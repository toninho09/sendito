//! Application state and input handling.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use sendito_core::complete::FieldNode;
use sendito_core::model::*;
use sendito_core::vars::{Scope, VarValue};
use sendito_pipeline::human_size;
use sendito_store::collection::{name_of, parent_rel};
use sendito_store::env::COLLECTION_ENV;
use sendito_store::history::HistoryEntry;
use sendito_store::session::{Layout, Session, SessionTab};
use tokio::sync::mpsc;

use crate::complete::{CompleteKind, Completion};
use crate::editor::{Clipboard, Outcome, Pos, TextEditor, VimMode};
use crate::form::TableOutcome;
use crate::keymap::{Action, Chord, Ctx, Keymap, Lookup};
use crate::overlay::*;
use crate::runner::{
    self, Control, GrpcControl, GrpcUpdate, HttpOutcome, Project, RunCtx, SchemaCache, WsControl,
    WsUpdate,
};
use crate::tab::{Field, Kind, QueryBar, QueryMode, Response, Running, Summary, Tab, Watch};
use crate::theme::Theme;
use crate::tree::{RowKind, TreeState};
use crate::viewer::{Doc, ViewMode, Viewer};

mod actions;
mod commands;
mod completion;
mod dialogs;
mod env;
mod examples;
mod folders;
mod hints;
mod history;
mod import_export;
mod ingest;
mod keys;
mod mcp;
mod mouse;
mod overlay_keys;
mod query;
mod schema;
mod send;
mod settings;
mod text;
mod tree_ops;

pub use folders::folder_label;
pub use settings::*;
pub use text::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Sidebar,
    Filter,
    /// URL bar part: HTTP 0 method, 1 URL, 2 send · gRPC 0 target, 1 method, 2 send.
    Bar(usize),
    ReqTabs,
    ReqBody,
    RespTabs,
    RespBody,
    /// The query bar (jq filter or search) above the response view.
    RespQuery,
}

impl Focus {
    /// The URL bar or the request pane has the focus.
    pub fn in_request(self) -> bool {
        matches!(self, Focus::Bar(_) | Focus::ReqTabs | Focus::ReqBody)
    }

    /// The response pane or its query bar has the focus.
    pub fn in_response(self) -> bool {
        matches!(self, Focus::RespTabs | Focus::RespBody | Focus::RespQuery)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarMode {
    Tree,
    History,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Info,
    Ok,
    Warn,
    Error,
}

/// Clickable regions registered while drawing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    Tab(usize),
    TabClose(usize),
    NewTab,
    EnvSelector,
    HelpButton,
    SidebarMode(SidebarMode),
    SidebarArea,
    TreeRow(usize),
    TreeFilter,
    HistoryRow(usize),
    /// A recent request on the start screen.
    RecentRow(usize),
    Bar(usize),
    ReqTab(usize),
    ReqField(usize),
    /// The JSON error in the body editor's status (0-based line and column).
    JsonError {
        line: usize,
        col: usize,
    },
    /// One option of a choice field drawn as chips (the body type).
    ChoiceOption {
        field: usize,
        index: usize,
    },
    TableCell {
        field: usize,
        row: usize,
        col: usize,
    },
    TableToggle {
        field: usize,
        row: usize,
    },
    TableDelete {
        field: usize,
        row: usize,
    },
    TextArea {
        field: usize,
    },
    RespTab(usize),
    RespView,
    /// The query bar above the response view.
    RespQuery,
    /// A row of the stream message list.
    StreamMsg(usize),
    ViewMode(ViewMode),
    RespAction(RespAction),
    SidebarBorder,
    SplitBorder,
    Overlay(OverlayHit),
    /// A row of the completion popup.
    CompleteItem(usize),
    /// A key of the Ctrl+W menu, or (`None`) the rest of its box.
    Menu(Option<crate::keymap::Action>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespAction {
    Copy,
    Save,
    Wrap,
    Cancel,
    SendMessage,
    EndStream,
    Template,
    Format,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayHit {
    Item(usize),
    Button(usize),
    Input,
    Choice(usize),
    /// The value chip of a settings row (clicking it cycles the value).
    Value(usize),
    /// An entry of an open prompt option list, by its position in the list.
    PromptOption(usize),
    EnvTab(usize),
    EnvCell {
        row: usize,
        col: usize,
    },
    EnvDelete(usize),
    /// A cell of the MCP servers editor.
    McpCell {
        row: usize,
        col: usize,
    },
    /// The ✕ of an MCP servers row.
    McpDelete(usize),
    /// A button of the MCP servers editor (0 = save, 1 = add, 2 = details,
    /// 3 = close).
    McpButton(usize),
    /// A tab of a config editor (MCP server details, OAuth, keymap).
    CfgTab(usize),
    /// A cell of a config editor.
    CfgCell {
        row: usize,
        col: usize,
    },
    /// The ✕ of a config editor row.
    CfgDelete(usize),
    /// A button of a config editor (0 = save, 1 = add, 2 = close).
    CfgButton(usize),
    /// Dead space inside a dialog frame; clicking it does nothing.
    Dialog,
    Outside,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drag {
    Sidebar,
    Split,
    Tree {
        rel: String,
        start: (u16, u16),
        active: bool,
    },
    Text {
        field: Option<usize>,
        bar: bool,
    },
}

pub enum AppMsg {
    Http {
        tab: u64,
        outcome: Box<HttpOutcome>,
    },
    Grpc {
        tab: u64,
        update: GrpcUpdate,
    },
    Ws {
        tab: u64,
        update: runner::WsUpdate,
    },
    Mcp {
        tab: u64,
        update: runner::McpUpdate,
    },
    OAuthUrl {
        job: u64,
        url: String,
    },
    OAuthDone {
        job: u64,
        result: Result<String, String>,
    },
    Cookies {
        job: u64,
        result: Result<CookiesOutcome, String>,
    },
    McpTools {
        job: u64,
        tab: u64,
        server: String,
        /// Open the picker; a false value only fills the tab's tool list
        /// (completion pulls it in the background).
        open_picker: bool,
        result: Result<Vec<sendito_pipeline::mcp::Tool>, String>,
        /// The server needs an OAuth sign-in.
        needs_login: bool,
    },
    Schema {
        tab: u64,
        job: u64,
        result: Result<Vec<sendito_pipeline::grpc::ServiceInfo>, String>,
        open_picker: bool,
        template: Option<String>,
    },
    History(Vec<HistoryEntry>),
    Scope {
        /// The scope, its errors and the names whose value did not load.
        result: Box<(Scope, Vec<String>, Vec<String>)>,
        then: ScopeThen,
        /// "Send anyway" was chosen in the "Secrets not loaded" dialog.
        secrets_ok: bool,
    },
}

/// Outcome of a `:cookies` command, produced off the UI thread (the jar is
/// guarded by a cross-process lock).
/// The last row of the cookie jar picker.
const CLEAR_COOKIES: &str = "Clear all cookies";

pub enum CookiesOutcome {
    List(Vec<String>),
    Done(&'static str),
}

/// What to do once a full scope (with secrets) is ready.
pub enum ScopeThen {
    Http {
        tab: u64,
        req: Box<HttpRequest>,
        name: String,
        path: Option<String>,
        from_watch: bool,
    },
    Grpc {
        tab: u64,
        req: Box<GrpcRequest>,
        name: String,
        path: Option<String>,
    },
    Ws {
        tab: u64,
        req: Box<WebSocketRequest>,
        name: String,
        path: Option<String>,
    },
    Mcp {
        tab: u64,
        req: Box<McpRequest>,
        name: String,
        path: Option<String>,
    },
}

impl ScopeThen {
    /// The tab and the request this send is for.
    fn request(&self) -> (u64, Request) {
        match self {
            ScopeThen::Http { tab, req, .. } => (*tab, Request::Http((**req).clone())),
            ScopeThen::Grpc { tab, req, .. } => (*tab, Request::Grpc((**req).clone())),
            ScopeThen::Ws { tab, req, .. } => (*tab, Request::Ws((**req).clone())),
            ScopeThen::Mcp { tab, req, .. } => (*tab, Request::Mcp((**req).clone())),
        }
    }
}

/// Rectangles from the last frame, used for mouse handling.
#[derive(Debug, Clone, Default)]
pub struct Areas {
    pub sidebar: Rect,
    pub tree: Rect,
    pub main: Rect,
    pub request: Rect,
    pub req_body: Rect,
    pub response: Rect,
    pub resp_body: Rect,
    pub field_rects: Vec<Rect>,
    pub bar_url: Rect,
    /// Where the focused editor's cursor was drawn last frame.
    pub caret: Option<(u16, u16)>,
}

pub struct App {
    pub project: Project,
    pub theme: Theme,
    /// Built-in and user themes.
    pub themes: crate::theme::Registry,
    pub icons: crate::icons::Icons,
    /// Advances while a request runs (spinners); stays 0 in tests.
    pub anim_frame: u64,
    /// Where the mouse pointer is, for hover highlights.
    pub hover: Option<(u16, u16)>,
    pub keymap: Keymap,
    pub vim: bool,
    pub tree: TreeState,
    pub sidebar_mode: SidebarMode,
    pub history: Option<Vec<HistoryEntry>>,
    pub history_sel: usize,
    pub history_scroll: usize,
    /// Whether the next draw should scroll `history_sel` into view; the wheel
    /// disarms it to allow free browsing (same idea as `TreeState::follow`).
    pub history_follow: bool,
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub focus: Focus,
    pub layout: Layout,
    pub zoom: bool,
    pub active_env: Option<String>,
    pub env_list: Vec<String>,
    pub runtime: BTreeMap<String, VarValue>,
    pub names: Scope,
    /// The settings of every folder that has a `folder.toml`, by folder path.
    pub folders: BTreeMap<String, FolderSettings>,
    pub clipboard: Clipboard,
    pub pending: Vec<Chord>,
    pub overlay: Option<Overlay>,
    /// Inline completion popup; not an overlay: it does not dim or block.
    pub complete: Option<crate::complete::Completion>,
    pub status: Option<(String, StatusKind, Instant)>,
    /// Home directory, shown as `~` in paths.
    pub home: Option<PathBuf>,
    /// Where the last frame put the terminal cursor (None: hidden).
    pub last_cursor: Option<(u16, u16)>,
    pub rt: tokio::runtime::Handle,
    pub tx: mpsc::UnboundedSender<AppMsg>,
    pub rx: mpsc::UnboundedReceiver<AppMsg>,
    pub hits: Vec<(Rect, Hit)>,
    pub areas: Areas,
    pub drag: Option<Drag>,
    pub drop_target: Option<String>,
    pub last_click: Option<(Instant, u16, u16)>,
    pub quit: bool,
    pub session_dirty: bool,
    pub last_session_save: Instant,
    pub next_id: u64,
    pub schemas: SchemaCache,
    schema_job: Option<(u64, tokio::task::JoinHandle<()>)>,
    schema_seq: u64,
    mcp_discovery: Option<(u64, tokio::task::JoinHandle<()>)>,
    mcp_discovery_seq: u64,
    oauth_login: Option<(u64, tokio::task::JoinHandle<()>)>,
    oauth_login_seq: u64,
    /// What to do when the running MCP sign-in succeeds.
    oauth_then: Option<McpLoginThen>,
    cookies_job: Option<(u64, tokio::task::JoinHandle<()>)>,
    cookies_job_seq: u64,
    pub filter: TextEditor,
    pub osc52: Vec<String>,
    pub size: (u16, u16),
    pub parked_env: Option<EnvEditor>,
    /// Set by "Send anyway" in the "Secrets not loaded" dialog, for the next
    /// send only.
    secrets_ok: bool,
    /// The MCP servers editor parked behind its save/discard confirmation.
    pub parked_mcp: Option<McpEditor>,
    /// A config editor kept behind its "unsaved changes" dialog.
    pub parked_cfg: Option<crate::config_editor::CfgEditor>,
    /// The config file settings are saved to (the settings dialog chooses).
    pub config_scope: ConfigScope,
    /// The prompt or import dialog parked behind its discard confirmation.
    pub parked_dialog: Option<Overlay>,
    /// The last report dialog (title, text), reopened by :report.
    pub last_report: Option<(String, String)>,
    pub cursor: Option<(u16, u16)>,
}

/// `NO_COLOR` is set (and not empty): no colors at all.
pub fn no_color() -> bool {
    std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty())
}

fn vim_label(e: &TextEditor) -> VimMode {
    e.vim
}

/// The follow-up of an MCP sign-in.
struct McpLoginThen {
    server: String,
    /// The server had no auth table: write `type = "oauth"` on success.
    write_auth: bool,
    /// The request to run again (tab id and what to run).
    retry: Option<(u64, McpAfterLogin)>,
}

impl App {
    pub fn new(project: Project, session: Session, rt: tokio::runtime::Handle) -> Self {
        let mut warnings = project.warnings.clone();
        let (keymap, kw) = Keymap::defaults().with_overrides(&project.config.keymap);
        warnings.extend(kw);
        let (files, file_warnings) = sendito_store::themes::discover(&project.theme_dirs);
        let themes = crate::theme::Registry::from_files(files, file_warnings);
        warnings.extend(themes.warnings.iter().cloned());
        let theme_name = &project.config.ui.theme;
        if !themes.is_known(theme_name) {
            warnings.push(format!("unknown theme \"{theme_name}\"; using auto"));
        } else if !themes.notes(theme_name).is_empty() {
            warnings.push(format!(
                "theme {theme_name}: {}",
                themes.notes(theme_name).join("; ")
            ));
        }
        let theme = if no_color() {
            Theme::mono()
        } else {
            themes.theme(theme_name)
        };
        let vim = project.config.ui.vim_mode;
        let root = project
            .coll
            .scan()
            .unwrap_or_else(|_| sendito_store::Folder {
                name: project.name.clone(),
                rel: String::new(),
                folders: vec![],
                requests: vec![],
            });
        let tree = TreeState::new(root, session.expanded.clone());
        let env_list = project.envs.list();
        let active_env = session.active_env.clone().filter(|e| env_list.contains(e));
        let runtime: BTreeMap<String, VarValue> = session
            .runtime_vars
            .iter()
            .map(|(k, v)| (k.clone(), VarValue::plain(v.clone())))
            .collect();
        let names = runner::names_scope(&project.envs, active_env.as_deref(), &runtime);
        let (tx, rx) = mpsc::unbounded_channel();
        let icons = crate::icons::Icons::by_name(&project.config.ui.icons);
        let mut app = App {
            project,
            theme,
            themes,
            icons,
            anim_frame: 0,
            hover: None,
            keymap,
            vim,
            tree,
            sidebar_mode: SidebarMode::Tree,
            history: None,
            history_sel: 0,
            history_scroll: 0,
            history_follow: true,
            tabs: Vec::new(),
            active: 0,
            focus: Focus::Sidebar,
            layout: session.layout.clone(),
            zoom: false,
            active_env,
            env_list,
            runtime,
            names,
            folders: BTreeMap::new(),
            clipboard: Clipboard::default(),
            pending: Vec::new(),
            overlay: None,
            status: None,
            home: std::env::var_os("HOME").map(PathBuf::from),
            last_cursor: None,
            rt,
            tx,
            rx,
            hits: Vec::new(),
            areas: Areas::default(),
            drag: None,
            drop_target: None,
            last_click: None,
            quit: false,
            session_dirty: false,
            last_session_save: Instant::now(),
            next_id: 1,
            schemas: SchemaCache::default(),
            schema_job: None,
            schema_seq: 0,
            mcp_discovery: None,
            mcp_discovery_seq: 0,
            oauth_login: None,
            oauth_login_seq: 0,
            oauth_then: None,
            cookies_job: None,
            cookies_job_seq: 0,
            filter: TextEditor::single("").with_placeholder("filter…"),
            osc52: Vec::new(),
            size: (80, 24),
            parked_env: None,
            secrets_ok: false,
            parked_mcp: None,
            parked_cfg: None,
            config_scope: ConfigScope::default(),
            parked_dialog: None,
            last_report: None,
            cursor: None,
            complete: None,
        };
        app.load_folders();
        app.restore_tabs(&session);
        if app.tabs.is_empty() {
            app.focus = Focus::Sidebar;
            // The start screen lists the most recent requests.
            if app.project.config.history.enabled {
                app.load_history();
            }
        }
        if !warnings.is_empty() {
            app.set_status(warnings.join(" · "), StatusKind::Warn);
        }
        app
    }

    fn restore_tabs(&mut self, s: &Session) {
        for t in &s.tabs {
            if let Some(rel) = &t.folder {
                self.restore_folder_tab(rel, t.draft.as_deref());
                continue;
            }
            let saved = t.path.as_ref().and_then(|p| self.project.coll.load(p).ok());
            let draft = t
                .draft
                .as_ref()
                .and_then(|d| sendito_store::request_file::parse(d).ok());
            let req = match (&draft, &saved) {
                (Some(d), _) => d.clone(),
                (None, Some(s)) => s.clone(),
                (None, None) => continue,
            };
            let name = t
                .path
                .as_deref()
                .map(|p| name_of(p).to_string())
                .unwrap_or_else(|| t.name.clone());
            let id = self.alloc_id();
            let mut tab = Tab::new(
                id,
                t.path.clone().filter(|_| saved.is_some()),
                name,
                &req,
                saved,
            );
            tab.ed.set_vim(self.vim);
            tab.refresh_model();
            self.tabs.push(tab);
        }
        self.active = s.active_tab.min(self.tabs.len().saturating_sub(1));
        if !self.tabs.is_empty() {
            self.focus = self.top_bar_focus();
        }
    }

    pub fn alloc_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    /// Drops a confirmation or note (not warnings or errors): a new action
    /// makes it stale.
    pub(crate) fn clear_transient_status(&mut self) {
        if matches!(self.status, Some((_, StatusKind::Ok | StatusKind::Info, _))) {
            self.status = None;
        }
    }

    pub fn set_status(&mut self, msg: impl Into<String>, kind: StatusKind) {
        self.status = Some((msg.into(), kind, Instant::now()));
    }

    pub fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    pub fn tab_mut(&mut self) -> Option<&mut Tab> {
        self.tabs.get_mut(self.active)
    }

    fn tab_by_id(&mut self, id: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn session(&self) -> Session {
        Session {
            active_env: self.active_env.clone(),
            active_tab: self.active,
            tabs: self
                .tabs
                .iter()
                .map(|t| SessionTab {
                    path: t.path.clone(),
                    name: t.name.clone(),
                    // Tabs without a file always keep their request here.
                    draft: if t.folder.is_some() {
                        t.dirty()
                            .then(|| sendito_store::folder::to_toml(&t.folder_settings()))
                    } else if t.dirty() || t.path.is_none() {
                        Some(sendito_store::request_file::to_toml(&t.model))
                    } else {
                        None
                    },
                    folder: t.folder.clone(),
                })
                .collect(),
            layout: self.layout.clone(),
            expanded: self.tree.expanded_list(),
            runtime_vars: self
                .runtime
                .iter()
                .filter(|(_, v)| !v.secret)
                .map(|(k, v)| (k.clone(), v.value.clone()))
                .collect(),
        }
    }

    pub fn save_session(&mut self) {
        let path = sendito_store::session::session_path(&self.project.root);
        if let Err(e) = sendito_store::session::save(&path, &self.session()) {
            self.set_status(format!("could not save session: {e}"), StatusKind::Warn);
        }
        self.session_dirty = false;
        self.last_session_save = Instant::now();
    }

    /// Periodic work: save the session a moment after changes, expire status
    /// messages and repeat a watched request. Returns whether anything changed
    /// that needs a redraw.
    pub fn tick(&mut self) -> bool {
        if self.session_dirty && self.last_session_save.elapsed() > Duration::from_secs(2) {
            self.save_session();
        }
        if let Some((_, kind, at)) = &self.status {
            let ttl = if *kind == StatusKind::Error { 12 } else { 6 };
            if at.elapsed() > Duration::from_secs(ttl) {
                self.status = None;
            }
        }
        // Skip missed intervals while a request is running. Each watched tab
        // can continue in the background; dialogs pause automatic sends.
        let now = Instant::now();
        for tab in &mut self.tabs {
            if tab.running.is_some()
                && let Some(w) = &mut tab.watch
                && now >= w.next
            {
                w.next = now + w.interval;
            }
        }
        if self.overlay.is_some() {
            return false;
        }
        let due: Vec<usize> = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(i, tab)| {
                (tab.ed.kind == Kind::Http
                    && tab.running.is_none()
                    && tab.watch.as_ref().is_some_and(|w| now >= w.next))
                .then_some(i)
            })
            .collect();
        let active = self.active;
        let mut changed = false;
        for i in due {
            self.tabs[i].refresh_model();
            let (request, names) = self.effective(&self.tabs[i]);
            let undefined = !runner::unresolved(&request, &names).is_empty()
                || !sendito_core::template::empty_placeholders(
                    &sendito_store::request_file::to_toml(&request),
                )
                .is_empty();
            if undefined {
                self.tabs[i].watch = None;
                self.set_status(
                    "watch stopped: request has unresolved variables",
                    StatusKind::Warn,
                );
            } else {
                // `send_opts` uses the active index for the duration of this
                // synchronous setup only; restore focus before processing events.
                self.active = i;
                self.send_opts(false, true);
                self.active = active;
            }
            changed = true;
        }
        changed
    }

    /// Starts or stops re-sending the active request every `secs` seconds
    /// (`:watch [s]`, the watch key).
    fn toggle_watch(&mut self, secs: Option<u64>) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        if tab.watch.take().is_some() {
            self.set_status("watch off", StatusKind::Info);
            return;
        }
        if tab.ed.kind != Kind::Http {
            self.set_status(
                "watch repeats HTTP requests; send gRPC and MCP calls yourself",
                StatusKind::Warn,
            );
            return;
        }
        let s = secs.unwrap_or_else(|| self.project.config.ui.watch_interval_s.max(1));
        if !(1..=86_400).contains(&s) {
            self.set_status(
                "watch interval must be between 1 and 86400 seconds",
                StatusKind::Error,
            );
            return;
        }
        tab.watch = Some(Watch::new(Duration::from_secs(s)));
        let key = self.keymap.hint(Action::WatchToggle, self.vim);
        self.set_status(
            format!(
                "watch on: sending every {s} s until {key}, cancel, close or change environment"
            ),
            StatusKind::Ok,
        );
    }

    /// The vim mode of the editor that has the keyboard, if one does.
    pub fn focused_vim_mode(&mut self) -> Option<VimMode> {
        let focus = self.focus;
        let tab = self.tabs.get_mut(self.active)?;
        match focus {
            Focus::Bar(p) => tab.ed.bar_text(p).map(|e| e.vim),
            Focus::ReqBody => match tab.current_field() {
                Some(f) if f.is_table() => tab
                    .ed
                    .table_mut(f)
                    .and_then(|t| t.editor_mut().map(|e| e.vim)),
                Some(f) => tab.ed.text_mut(f).map(|e| e.vim),
                None => None,
            },
            _ => None,
        }
    }

    /// The terminal window title: the open request and the collection.
    pub fn window_title(&self) -> String {
        match self.tab() {
            Some(t) => format!("sendito · {} · {}", t.title(), self.project.name),
            None => format!("sendito · {}", self.project.name),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.mcp_discovery.is_some()
            || self.schema_job.is_some()
            || self.oauth_login.is_some()
            || self.tabs.iter().any(|t| t.running.is_some())
    }
}
