//! Application state and input handling.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;
use sendito_core::model::*;
use sendito_core::vars::{Scope, VarValue};
use sendito_store::collection::{name_of, parent_rel};
use sendito_store::env::COLLECTION_ENV;
use sendito_store::history::HistoryEntry;
use sendito_store::session::{Layout, Session, SessionTab};
use tokio::sync::mpsc;

use crate::editor::{Clipboard, Outcome, TextEditor, VimMode};
use crate::form::TableOutcome;
use crate::keymap::{Action, Chord, Ctx, Keymap, Lookup};
use crate::overlay::*;
use crate::runner::{self, GrpcControl, GrpcUpdate, HttpOutcome, Project, RunCtx, SchemaCache};
use crate::tab::{Field, Kind, Response, Running, Summary, Tab, Watch};
use crate::theme::Theme;
use crate::tree::{RowKind, TreeState};
use crate::viewer::{Doc, ViewMode, Viewer};

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
    ViewMode(ViewMode),
    RespAction(RespAction),
    SidebarBorder,
    SplitBorder,
    Overlay(OverlayHit),
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
    Close,
    EnvTab(usize),
    EnvCell { row: usize, col: usize },
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
        result: Result<Vec<sendito_mcp::Tool>, String>,
    },
    Schema {
        tab: u64,
        result: Result<Vec<sendito_grpc::ServiceInfo>, String>,
        open_picker: bool,
        template: Option<String>,
    },
    History(Vec<HistoryEntry>),
    Scope {
        result: Box<(Scope, Vec<String>)>,
        then: ScopeThen,
    },
}

/// Outcome of a `:cookies` command, produced off the UI thread (the jar is
/// guarded by a cross-process lock).
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
    Mcp {
        tab: u64,
        req: Box<McpRequest>,
        name: String,
        path: Option<String>,
    },
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
    pub tabs: Vec<Tab>,
    pub active: usize,
    pub focus: Focus,
    pub layout: Layout,
    pub zoom: bool,
    pub active_env: Option<String>,
    pub env_list: Vec<String>,
    pub runtime: BTreeMap<String, VarValue>,
    pub names: Scope,
    pub clipboard: Clipboard,
    pub pending: Vec<Chord>,
    pub overlay: Option<Overlay>,
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
    mcp_discovery: Option<(u64, tokio::task::JoinHandle<()>)>,
    mcp_discovery_seq: u64,
    oauth_login: Option<(u64, tokio::task::JoinHandle<()>)>,
    oauth_login_seq: u64,
    cookies_job: Option<(u64, tokio::task::JoinHandle<()>)>,
    cookies_job_seq: u64,
    pub filter: TextEditor,
    pub osc52: Vec<String>,
    pub size: (u16, u16),
    pub parked_env: Option<EnvEditor>,
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
            tabs: Vec::new(),
            active: 0,
            focus: Focus::Sidebar,
            layout: session.layout.clone(),
            zoom: false,
            active_env,
            env_list,
            runtime,
            names,
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
            mcp_discovery: None,
            mcp_discovery_seq: 0,
            oauth_login: None,
            oauth_login_seq: 0,
            cookies_job: None,
            cookies_job_seq: 0,
            filter: TextEditor::single("").with_placeholder("filter…"),
            osc52: Vec::new(),
            size: (80, 24),
            parked_env: None,
            last_report: None,
            cursor: None,
        };
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
    fn clear_transient_status(&mut self) {
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
                    draft: if t.dirty() || t.path.is_none() {
                        Some(sendito_store::request_file::to_toml(&t.model))
                    } else {
                        None
                    },
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
            let request = &self.tabs[i].model;
            let undefined = !runner::unresolved(request, &self.names).is_empty()
                || !sendito_core::template::empty_placeholders(
                    &sendito_store::request_file::to_toml(request),
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
            Focus::Bar(1) if tab.ed.kind == Kind::Http => Some(tab.ed.url.vim),
            Focus::Bar(0) if tab.ed.kind == Kind::Grpc => Some(tab.ed.target.vim),
            Focus::Bar(0) if tab.ed.kind == Kind::Mcp => Some(tab.ed.mcp_server.vim),
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
            || self.oauth_login.is_some()
            || self.tabs.iter().any(|t| t.running.is_some())
    }

    // ----- events -------------------------------------------------------

    pub fn on_event(&mut self, ev: Event) {
        match ev {
            Event::Key(k) if k.kind != KeyEventKind::Release => self.on_key(k),
            Event::Mouse(m) if self.project.config.ui.mouse => self.on_mouse(m),
            Event::Paste(s) => self.on_paste(&s),
            Event::Resize(w, h) => self.size = (w, h),
            _ => {}
        }
    }

    pub fn on_paste(&mut self, s: &str) {
        if let Some(ov) = &mut self.overlay {
            match ov {
                Overlay::Import(i) => i.input.insert_str(s),
                Overlay::Prompt(p) => p.input.insert_str(s),
                Overlay::Picker(p) => p.filter.insert_str(s),
                Overlay::Palette(p) => {
                    p.input.insert_str(s);
                    p.sel = 0;
                    p.armed = true;
                }
                Overlay::Env(e) => e.table.paste(s),
                _ => {}
            }
            return;
        }
        // A pasted curl command in the URL bar becomes a request.
        if matches!(self.focus, Focus::Bar(1)) && s.trim_start().starts_with("curl ") {
            self.import_curl(s);
            return;
        }
        match self.focus {
            Focus::Filter => {
                self.filter.insert_str(s);
                let f = self.filter.text();
                self.tree.set_filter(&f);
            }
            Focus::Bar(_) | Focus::ReqBody => {
                if let Some(e) = self.focused_text() {
                    e.insert_str(s);
                } else if let Some(t) = self.focused_table() {
                    t.paste(s);
                }
                self.changed();
            }
            _ => {}
        }
    }

    fn changed(&mut self) {
        if let Some(t) = self.tab_mut() {
            t.refresh_model();
        }
        // An edit makes "created …" / "saved …" stale.
        self.clear_transient_status();
        self.session_dirty = true;
    }

    fn focused_text(&mut self) -> Option<&mut TextEditor> {
        let focus = self.focus;
        let tab = self.tabs.get_mut(self.active)?;
        match focus {
            Focus::Bar(1) if tab.ed.kind == Kind::Http => Some(&mut tab.ed.url),
            Focus::Bar(0) if tab.ed.kind == Kind::Grpc => Some(&mut tab.ed.target),
            Focus::Bar(0) if tab.ed.kind == Kind::Mcp => Some(&mut tab.ed.mcp_server),
            Focus::ReqBody => {
                let f = tab.current_field()?;
                tab.ed.text_mut(f)
            }
            _ => None,
        }
    }

    fn focused_table(&mut self) -> Option<&mut crate::form::Table> {
        let tab = self.tabs.get_mut(self.active)?;
        if self.focus != Focus::ReqBody {
            return None;
        }
        let f = tab.current_field()?;
        tab.ed.table_mut(f)
    }

    /// Whether the focused widget is a text editor in vim normal mode (or not text).
    fn in_vim_normal(&mut self) -> bool {
        if !self.vim {
            return false;
        }
        if let Some(e) = self.focused_text() {
            return vim_label(e) != VimMode::Insert;
        }
        if let Some(t) = self.focused_table() {
            return t.editor_mut().is_none_or(|e| e.vim != VimMode::Insert);
        }
        true
    }

    fn contexts(&mut self) -> Vec<Ctx> {
        let is_table = self.focused_table().is_some();
        match self.focus {
            Focus::Sidebar if self.sidebar_mode == SidebarMode::Tree => {
                vec![Ctx::Tree, Ctx::Global]
            }
            Focus::RespBody | Focus::RespTabs => vec![Ctx::Response, Ctx::Global],
            Focus::ReqBody if is_table => vec![Ctx::Table, Ctx::Global],
            _ => vec![Ctx::Global],
        }
    }

    /// Runs `f` on the focused text editor together with the clipboard.
    fn with_text<R>(&mut self, f: impl FnOnce(&mut TextEditor, &mut Clipboard) -> R) -> Option<R> {
        let focus = self.focus;
        let tab = self.tabs.get_mut(self.active)?;
        let cb = &mut self.clipboard;
        let ed = match focus {
            Focus::Bar(1) if tab.ed.kind == Kind::Http => &mut tab.ed.url,
            Focus::Bar(0) if tab.ed.kind == Kind::Grpc => &mut tab.ed.target,
            Focus::Bar(0) if tab.ed.kind == Kind::Mcp => &mut tab.ed.mcp_server,
            Focus::ReqBody => {
                let fld = tab.current_field()?;
                tab.ed.text_mut(fld)?
            }
            _ => return None,
        };
        Some(f(ed, cb))
    }

    /// Runs `f` on the focused table together with the clipboard.
    fn with_table<R>(
        &mut self,
        f: impl FnOnce(&mut crate::form::Table, &mut Clipboard) -> R,
    ) -> Option<R> {
        if self.focus != Focus::ReqBody {
            return None;
        }
        let tab = self.tabs.get_mut(self.active)?;
        let cb = &mut self.clipboard;
        let fld = tab.current_field()?;
        let t = tab.ed.table_mut(fld)?;
        Some(f(t, cb))
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // A confirmation or note lasts until the next key press; warnings and
        // errors stay until replaced.
        self.clear_transient_status();
        if self.overlay.is_some() {
            self.overlay_key(key);
            return;
        }
        let chord = Chord::from_event(&key);
        // Tab inside a multi-line editor indents (unless in vim normal mode).
        if chord == Chord::new(KeyCode::Tab, KeyModifiers::NONE)
            && self.pending.is_empty()
            && self.focus == Focus::ReqBody
        {
            let multiline = self
                .tab()
                .and_then(|t| t.current_field())
                .is_some_and(Field::is_multiline);
            if multiline && !self.in_vim_normal() {
                self.forward_to_widget(key);
                return;
            }
        }
        // Vim command mode.
        if self.vim
            && self.pending.is_empty()
            && chord == Chord::new(KeyCode::Char(':'), KeyModifiers::NONE)
            && self.in_vim_normal()
            && self.focus != Focus::Filter
        {
            self.open_palette(":");
            return;
        }
        let mut seq = self.pending.clone();
        seq.push(chord);
        let ctxs = self.contexts();
        // Text widgets get plain keys first; only global bindings apply there.
        let text_focus = matches!(self.focus, Focus::Filter) || self.focused_text().is_some();
        let ctxs: Vec<Ctx> = if text_focus { vec![Ctx::Global] } else { ctxs };
        // In vim normal mode inside tables, `dd` deletes rows like lines.
        match self.keymap.lookup(&ctxs, self.vim, &seq) {
            Lookup::Action(a) => {
                self.pending.clear();
                self.dispatch(a);
            }
            Lookup::Prefix => {
                self.pending = seq;
            }
            Lookup::None => {
                let had_pending = !self.pending.is_empty();
                self.pending.clear();
                if !had_pending {
                    self.forward_to_widget(key);
                }
            }
        }
    }

    fn forward_to_widget(&mut self, key: KeyEvent) {
        let vim = self.vim;
        match self.focus {
            Focus::Sidebar => self.sidebar_key(key),
            Focus::Filter => {
                let out = self.filter.handle_key(&key, false, &mut self.clipboard);
                match out {
                    Outcome::Changed => {
                        let f = self.filter.text();
                        self.tree.set_filter(&f);
                    }
                    Outcome::Submit => self.focus = Focus::Sidebar,
                    Outcome::Leave => {
                        self.filter.set_text("");
                        self.tree.set_filter("");
                        self.focus = Focus::Sidebar;
                    }
                    Outcome::Ignored if matches!(key.code, KeyCode::Down) => {
                        self.focus = Focus::Sidebar
                    }
                    _ => {}
                }
            }
            Focus::Bar(part) => self.bar_key(part, key),
            Focus::ReqTabs => match key.code {
                KeyCode::Left => self.subtab(-1),
                KeyCode::Right => self.subtab(1),
                KeyCode::Char('h') if vim => self.subtab(-1),
                KeyCode::Char('l') if vim => self.subtab(1),
                KeyCode::Enter | KeyCode::Down => self.enter_req_body(0),
                KeyCode::Char('j') if vim => self.enter_req_body(0),
                KeyCode::Up => self.focus = self.top_bar_focus(),
                KeyCode::Char('k') if vim => self.focus = self.top_bar_focus(),
                _ => {}
            },
            Focus::ReqBody => self.req_body_key(key),
            Focus::RespTabs => match key.code {
                KeyCode::Left => self.subtab(-1),
                KeyCode::Right => self.subtab(1),
                KeyCode::Enter | KeyCode::Down => self.focus = Focus::RespBody,
                _ => {}
            },
            Focus::RespBody => {}
        }
    }

    fn enter_req_body(&mut self, field: usize) {
        if let Some(t) = self.tab_mut() {
            if t.fields().is_empty() {
                return;
            }
            t.field = field.min(t.fields().len() - 1);
            self.focus = Focus::ReqBody;
        }
    }

    fn bar_key(&mut self, part: usize, key: KeyEvent) {
        let Some(kind) = self.tab().map(|t| t.ed.kind) else {
            return;
        };
        if kind == Kind::Mcp && part == 0 && key.code == KeyCode::F(2) {
            self.open_mcp_server_picker();
            return;
        }
        let vim = self.vim;
        let is_text = (kind == Kind::Http && part == 1)
            || (matches!(kind, Kind::Grpc | Kind::Mcp) && part == 0);
        if is_text {
            let Some(out) = self.with_text(|e, cb| e.handle_key(&key, vim, cb)) else {
                return;
            };
            match out {
                Outcome::Changed => self.changed(),
                Outcome::Submit => {
                    if kind == Kind::Http {
                        self.dispatch(Action::Send);
                    } else if kind == Kind::Mcp {
                        self.focus = Focus::ReqTabs;
                    } else {
                        self.focus = Focus::Bar(1);
                    }
                }
                Outcome::Leave => {}
                Outcome::Ignored => {
                    if matches!(key.code, KeyCode::Down) {
                        self.focus = Focus::ReqTabs;
                    }
                }
                Outcome::Handled => {}
            }
            return;
        }
        match (part, key.code) {
            (2, KeyCode::Enter | KeyCode::Char(' ')) => self.dispatch(Action::Send),
            (0, KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Down) if kind == Kind::Http => {
                self.open_method_picker(None)
            }
            (0, KeyCode::Char(c)) if kind == Kind::Http && c.is_ascii_alphabetic() => {
                self.open_method_picker(Some(c))
            }
            (0, KeyCode::Left | KeyCode::Right) if kind == Kind::Http => {
                let dir: isize = if key.code == KeyCode::Left { -1 } else { 1 };
                if let Some(t) = self.tab_mut() {
                    let i = HTTP_METHODS
                        .iter()
                        .position(|m| *m == t.ed.method)
                        .unwrap_or(0) as isize;
                    let n = HTTP_METHODS.len() as isize;
                    t.ed.method = HTTP_METHODS[((i + dir).rem_euclid(n)) as usize].to_string();
                }
                self.changed();
            }
            (1, KeyCode::Enter | KeyCode::Char(' ') | KeyCode::Down) if kind == Kind::Grpc => {
                self.open_grpc_method_picker()
            }
            (_, KeyCode::Down) => self.focus = Focus::ReqTabs,
            _ => {}
        }
    }

    fn req_body_key(&mut self, key: KeyEvent) {
        let vim = self.vim;
        let Some(field) = self.tab().and_then(|t| t.current_field()) else {
            self.focus = Focus::ReqTabs;
            return;
        };
        if key.code == KeyCode::F(2) {
            match field {
                Field::McpServer => {
                    self.open_mcp_server_picker();
                    return;
                }
                Field::McpTool => {
                    self.load_mcp_tools();
                    return;
                }
                _ => {}
            }
        }
        if field.is_table() {
            let Some(out) = self.with_table(|t, cb| t.handle_key(&key, vim, cb)) else {
                return;
            };
            match out {
                TableOutcome::Changed => self.changed(),
                TableOutcome::ExitUp => self.prev_field(),
                TableOutcome::ExitDown => {
                    self.next_field(false);
                }
                TableOutcome::Leave => self.focus = Focus::ReqTabs,
                TableOutcome::Handled | TableOutcome::Ignored => {
                    if vim && key.code == KeyCode::Char('d') {
                        // `dd` on a row in vim normal mode.
                    }
                }
            }
            return;
        }
        if field.is_choice() || field.is_toggle() {
            match key.code {
                KeyCode::Enter | KeyCode::Char(' ') if field.is_choice() => {
                    self.open_choice_picker(field)
                }
                KeyCode::Char(c)
                    if field.is_choice()
                        && c.is_alphanumeric()
                        && !(vim && matches!(c, 'j' | 'k')) =>
                {
                    self.open_choice_picker(field);
                    if let Some(Overlay::Picker(p)) = &mut self.overlay {
                        p.filter.set_text(&c.to_string());
                        p.filter.move_to_end();
                        p.sel = 0;
                    }
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    if let Some(t) = self.tab_mut() {
                        t.ed.cycle(field, 1);
                    }
                    self.changed();
                }
                KeyCode::Left | KeyCode::Right => {
                    let dir = if key.code == KeyCode::Left { -1 } else { 1 };
                    if let Some(t) = self.tab_mut() {
                        t.ed.cycle(field, dir);
                    }
                    self.changed();
                }
                KeyCode::Down => {
                    self.next_field(false);
                }
                KeyCode::Up => self.prev_field(),
                KeyCode::Char('j') if vim => {
                    self.next_field(false);
                }
                KeyCode::Char('k') if vim => self.prev_field(),
                KeyCode::Esc => self.focus = Focus::ReqTabs,
                _ => {}
            }
            return;
        }
        let Some(out) = self.with_text(|e, cb| e.handle_key(&key, vim, cb)) else {
            return;
        };
        match out {
            Outcome::Changed => self.changed(),
            Outcome::Submit => {
                self.next_field(false);
            }
            Outcome::Leave => self.focus = Focus::ReqTabs,
            Outcome::Ignored => match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.prev_field(),
                KeyCode::Down | KeyCode::Char('j') => {
                    self.next_field(false);
                }
                _ => {}
            },
            Outcome::Handled => {}
        }
    }

    fn prev_field(&mut self) {
        if let Some(t) = self.tab_mut()
            && t.field > 0
        {
            t.field -= 1;
            return;
        }
        self.focus = Focus::ReqTabs;
    }

    /// Moves to the next field; at the end, to the response (when `wrap`).
    fn next_field(&mut self, wrap: bool) -> bool {
        if let Some(t) = self.tab_mut()
            && t.field + 1 < t.fields().len()
        {
            t.field += 1;
            if let Some(f) = t.current_field()
                && let Some(tb) = t.ed.table_mut(f)
            {
                tb.sel_row = 0;
            }
            return true;
        }
        if wrap {
            self.focus = Focus::RespTabs;
        }
        false
    }

    fn sidebar_key(&mut self, key: KeyEvent) {
        if self.sidebar_mode == SidebarMode::History {
            let n = self.history.as_ref().map(|h| h.len()).unwrap_or(0);
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => {
                    self.history_sel = self.history_sel.saturating_sub(1)
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    self.history_sel = (self.history_sel + 1).min(n.saturating_sub(1))
                }
                KeyCode::Home => self.history_sel = 0,
                KeyCode::End => self.history_sel = n.saturating_sub(1),
                KeyCode::PageUp => self.history_sel = self.history_sel.saturating_sub(10),
                KeyCode::PageDown => {
                    self.history_sel = (self.history_sel + 10).min(n.saturating_sub(1))
                }
                KeyCode::Enter => self.open_history(self.history_sel, false),
                KeyCode::Char('s') => self.save_history_prompt(self.history_sel),
                KeyCode::Esc => self.toggle_history(),
                _ => {}
            }
        }
    }

    // ----- actions ------------------------------------------------------

    pub fn dispatch(&mut self, a: Action) {
        match a {
            Action::Send => {
                if self.focus == Focus::Sidebar && self.sidebar_mode == SidebarMode::History {
                    self.open_history(self.history_sel, true);
                } else {
                    self.send(false);
                }
            }
            Action::Cancel => self.cancel(),
            Action::WatchToggle => self.toggle_watch(None),
            Action::EndStream => self.end_stream(),
            Action::Save => self.save(),
            Action::Quit => self.request_quit(),
            Action::NewRequest => self.new_tab(Request::new_http()),
            Action::NewGrpcRequest => self.new_tab(Request::new_grpc()),
            Action::CloseTab => self.close_tab(false),
            Action::NextTab => self.switch_tab(1),
            Action::PrevTab => self.switch_tab(-1),
            Action::FocusLeft => self.focus_dir(-1, 0),
            Action::FocusRight => self.focus_dir(1, 0),
            Action::FocusUp => self.focus_dir(0, -1),
            Action::FocusDown => self.focus_dir(0, 1),
            Action::FocusNext => self.focus_cycle(1),
            Action::FocusPrev => self.focus_cycle(-1),
            Action::NextSubTab => self.subtab(1),
            Action::PrevSubTab => self.subtab(-1),
            Action::SubTab1
            | Action::SubTab2
            | Action::SubTab3
            | Action::SubTab4
            | Action::SubTab5
            | Action::SubTab6 => {
                let i = match a {
                    Action::SubTab1 => 0,
                    Action::SubTab2 => 1,
                    Action::SubTab3 => 2,
                    Action::SubTab4 => 3,
                    Action::SubTab5 => 4,
                    _ => 5,
                };
                self.set_subtab(i);
            }
            Action::Palette => self.open_palette(""),
            Action::Help => self.open_help(),
            Action::Settings => self.overlay = Some(Overlay::Settings { sel: 0 }),
            Action::SelectEnv => self.open_env_picker(),
            Action::ManageEnvs => self.open_env_editor(),
            Action::History => self.toggle_history(),
            Action::ToggleSidebar => {
                self.layout.sidebar_visible = !self.layout.sidebar_visible;
                if !self.layout.sidebar_visible
                    && matches!(self.focus, Focus::Sidebar | Focus::Filter)
                {
                    self.focus = self.top_bar_focus();
                }
                self.session_dirty = true;
            }
            Action::Import => self.open_import(""),
            Action::ExportCurl => self.export_curl(false),
            Action::FormatBody => self.format_body(),
            Action::Zoom => self.zoom = !self.zoom,
            Action::GrowSidebar => self.resize_sidebar(4),
            Action::ShrinkSidebar => self.resize_sidebar(-4),
            Action::GrowRequest => self.resize_split(5),
            Action::ShrinkRequest => self.resize_split(-5),
            Action::TreeOpen => self.tree_open(),
            Action::TreeExpand => {
                if !self.tree.expand() {
                    self.tree_open();
                }
                self.session_dirty = true;
            }
            Action::TreeCollapse => {
                self.tree.collapse();
                self.session_dirty = true;
            }
            Action::TreeNewRequest => self.prompt_new_request(),
            Action::TreeNewFolder => self.prompt_new_folder(),
            Action::TreeRename => self.prompt_rename(),
            Action::TreeDuplicate => self.tree_duplicate(),
            Action::TreeDelete => self.tree_delete(),
            Action::TreeMove => self.tree_move(),
            Action::TreeFilter => {
                self.focus = Focus::Filter;
                self.sidebar_mode = SidebarMode::Tree;
            }
            Action::CursorUp => self.cursor_move(-1),
            Action::CursorDown => self.cursor_move(1),
            Action::PageUp => self.cursor_page(-1),
            Action::PageDown => self.cursor_page(1),
            Action::Top => self.cursor_edge(false),
            Action::Bottom => self.cursor_edge(true),
            Action::ScrollLeft => self.with_viewer(|v| v.hscroll_by(-8)),
            Action::ScrollRight => self.with_viewer(|v| v.hscroll_by(8)),
            Action::ViewPretty => self.set_view(ViewMode::Pretty),
            Action::ViewRaw => self.set_view(ViewMode::Raw),
            Action::ViewHex => self.set_view(ViewMode::Hex),
            Action::Search => self.open_search(),
            Action::SearchNext => self.with_viewer(|v| {
                v.next_match(1);
            }),
            Action::SearchPrev => self.with_viewer(|v| {
                v.next_match(-1);
            }),
            Action::FoldToggle => self.with_viewer(|v| {
                v.toggle_fold();
            }),
            Action::FoldAll => self.with_viewer(Viewer::fold_all),
            Action::UnfoldAll => self.with_viewer(Viewer::unfold_all),
            Action::ToggleWrap => self.with_viewer(Viewer::toggle_wrap),
            Action::CopyBody => self.copy_body(),
            Action::SaveBody => self.prompt_save_body(),
            Action::NextCell => {
                if let Some(t) = self.focused_table() {
                    t.next_cell();
                }
            }
            Action::ToggleRow => {
                if let Some(t) = self.focused_table() {
                    t.toggle_row();
                }
                self.changed();
            }
            Action::DeleteRow => {
                if let Some(t) = self.focused_table() {
                    t.delete_row();
                }
                self.changed();
            }
            Action::Undo
            | Action::Redo
            | Action::SelectAll
            | Action::Copy
            | Action::Cut
            | Action::Paste => {}
        }
    }

    fn with_viewer(&mut self, f: impl FnOnce(&mut Viewer)) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let rt = tab.resp_tab;
        if let Some(r) = &mut tab.response {
            f(r.viewer_mut(rt));
        }
    }

    fn cursor_move(&mut self, d: isize) {
        match self.focus {
            Focus::Sidebar => {
                if self.sidebar_mode == SidebarMode::Tree {
                    self.tree.move_sel(d);
                }
            }
            _ => self.with_viewer(|v| v.move_cursor(d)),
        }
    }

    fn cursor_page(&mut self, d: isize) {
        match self.focus {
            Focus::Sidebar => {
                let h = self.areas.tree.height.max(2) as isize - 1;
                self.tree.move_sel(d * h);
            }
            _ => self.with_viewer(|v| v.page(d)),
        }
    }

    fn cursor_edge(&mut self, bottom: bool) {
        match self.focus {
            Focus::Sidebar => {
                let n = self.tree.rows.len() as isize;
                self.tree.move_sel(if bottom { n } else { -n });
            }
            _ => self.with_viewer(|v| if bottom { v.bottom() } else { v.top() }),
        }
    }

    fn set_view(&mut self, m: ViewMode) {
        if let Some(t) = self.tab_mut() {
            t.resp_tab = 0;
        }
        self.with_viewer(|v| v.set_mode(m));
    }

    fn resize_sidebar(&mut self, d: i32) {
        let w = (self.layout.sidebar_width as i32 + d).clamp(16, 80) as u16;
        self.layout.sidebar_width = w;
        self.session_dirty = true;
    }

    fn resize_split(&mut self, d: i32) {
        self.layout.request_pct = (self.layout.request_pct as i32 + d).clamp(20, 80) as u16;
        self.session_dirty = true;
    }

    /// The top bar part to focus for the active tab: HTTP has a method chip
    /// (part 0) and a URL (part 1); gRPC and MCP only have the address/server.
    fn top_bar_focus(&self) -> Focus {
        if self.tab().is_some_and(|t| t.ed.kind == Kind::Http) {
            Focus::Bar(1)
        } else {
            Focus::Bar(0)
        }
    }

    fn focus_order(&self) -> Vec<Focus> {
        let mut v = Vec::new();
        if self.layout.sidebar_visible {
            v.push(Focus::Sidebar);
        }
        if !self.tabs.is_empty() {
            v.push(Focus::Bar(0));
            if self.tab().is_some_and(|t| t.ed.kind != Kind::Mcp) {
                v.push(Focus::Bar(1));
            }
            v.extend([
                Focus::Bar(2),
                Focus::ReqTabs,
                Focus::ReqBody,
                Focus::RespTabs,
                Focus::RespBody,
            ]);
        }
        v
    }

    pub fn focus_cycle(&mut self, dir: isize) {
        // Inside the request body, Tab walks through the fields first.
        if self.focus == Focus::ReqBody {
            if dir > 0 && self.next_field(false) {
                return;
            }
            if dir < 0
                && let Some(t) = self.tab_mut()
                && t.field > 0
            {
                t.field -= 1;
                return;
            }
        }
        let order = self.focus_order();
        if order.is_empty() {
            return;
        }
        let cur = match self.focus {
            Focus::Filter => Focus::Sidebar,
            f => f,
        };
        let i = order.iter().position(|f| *f == cur).unwrap_or(0) as isize;
        let n = order.len() as isize;
        let next = order[((i + dir).rem_euclid(n)) as usize];
        if next == Focus::ReqBody {
            let count = self.tab().map(|t| t.fields().len()).unwrap_or(0);
            if count == 0 {
                self.focus = if dir > 0 {
                    Focus::RespTabs
                } else {
                    Focus::ReqTabs
                };
                return;
            }
            let start = if dir > 0 { 0 } else { count - 1 };
            self.enter_req_body(start);
            return;
        }
        self.focus = next;
    }

    fn focus_dir(&mut self, dx: i32, dy: i32) {
        let right_sidebar = self.project.config.ui.sidebar == "right";
        let in_sidebar = matches!(self.focus, Focus::Sidebar | Focus::Filter);
        if dx != 0 {
            let toward_sidebar = (dx < 0) != right_sidebar;
            if toward_sidebar && !in_sidebar && self.layout.sidebar_visible {
                self.focus = Focus::Sidebar;
            } else if !toward_sidebar && in_sidebar && !self.tabs.is_empty() {
                self.focus = Focus::Bar(if self.tab().is_some_and(|t| t.ed.kind != Kind::Http) {
                    0
                } else {
                    1
                });
            } else if let Focus::Bar(p) = self.focus {
                let mcp = self.tab().is_some_and(|t| t.ed.kind == Kind::Mcp);
                self.focus = Focus::Bar(if mcp {
                    if dx < 0 { 0 } else { 2 }
                } else {
                    (p as i32 + dx).clamp(0, 2) as usize
                });
            }
            return;
        }
        if in_sidebar || self.tabs.is_empty() {
            return;
        }
        let in_request = matches!(self.focus, Focus::Bar(_) | Focus::ReqTabs | Focus::ReqBody);
        if dy > 0 && in_request {
            self.focus = Focus::RespBody;
        } else if dy < 0 && !in_request {
            let non_http = self.tab().is_some_and(|t| t.ed.kind != Kind::Http);
            self.focus = Focus::Bar(if non_http { 0 } else { 1 });
        }
    }

    fn subtab(&mut self, dir: isize) {
        let resp = matches!(self.focus, Focus::RespTabs | Focus::RespBody);
        let Some(t) = self.tab_mut() else { return };
        if resp {
            let n = crate::tab::RESP_TABS.len() as isize;
            t.resp_tab = ((t.resp_tab as isize + dir).rem_euclid(n)) as usize;
        } else {
            let n = t.ed.tab_names().len() as isize;
            t.req_tab = ((t.req_tab as isize + dir).rem_euclid(n)) as usize;
            t.field = 0;
            let empty = t.fields().is_empty();
            if self.focus == Focus::ReqBody && empty {
                self.focus = Focus::ReqTabs;
            }
        }
    }

    fn set_subtab(&mut self, i: usize) {
        let resp = matches!(self.focus, Focus::RespTabs | Focus::RespBody);
        let Some(t) = self.tab_mut() else { return };
        if resp {
            if i < crate::tab::RESP_TABS.len() {
                t.resp_tab = i;
            }
        } else if i < t.ed.tab_names().len() {
            t.req_tab = i;
            t.field = 0;
            if !matches!(self.focus, Focus::ReqTabs | Focus::ReqBody) {
                self.focus = Focus::ReqTabs;
            }
            if self.focus == Focus::ReqBody && self.tab().is_some_and(|t| t.fields().is_empty()) {
                self.focus = Focus::ReqTabs;
            }
        }
    }

    fn switch_tab(&mut self, dir: isize) {
        if self.tabs.is_empty() {
            return;
        }
        let n = self.tabs.len() as isize;
        self.active = ((self.active as isize + dir).rem_euclid(n)) as usize;
        self.session_dirty = true;
    }

    pub fn new_tab(&mut self, req: Request) {
        self.clear_transient_status();
        let id = self.alloc_id();
        let n = self.tabs.iter().filter(|t| t.path.is_none()).count() + 1;
        let name = if n == 1 {
            "untitled".to_string()
        } else {
            format!("untitled {n}")
        };
        let blank =
            req == Request::new_http() || req == Request::new_grpc() || req == Request::new_mcp();
        let mut tab = Tab::new(id, None, name, &req, None);
        tab.ed.set_vim(self.vim);
        tab.refresh_model();
        // An empty new request has nothing to lose: it is not "unsaved" until
        // something is typed.
        if blank {
            tab.saved = Some(tab.model.clone());
        }
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        self.focus = if matches!(req, Request::Grpc(_) | Request::Mcp(_)) {
            Focus::Bar(0)
        } else {
            Focus::Bar(1)
        };
        self.session_dirty = true;
    }

    pub fn open_request(&mut self, rel: &str) {
        self.clear_transient_status();
        if let Some(i) = self
            .tabs
            .iter()
            .position(|t| t.path.as_deref() == Some(rel))
        {
            self.active = i;
            self.focus = Focus::Bar(if self.tabs[i].ed.kind == Kind::Http {
                1
            } else {
                0
            });
            return;
        }
        match self.project.coll.load(rel) {
            Ok(req) => {
                let id = self.alloc_id();
                let mut tab = Tab::new(
                    id,
                    Some(rel.to_string()),
                    name_of(rel).to_string(),
                    &req,
                    Some(req.clone()),
                );
                tab.ed.set_vim(self.vim);
                tab.refresh_model();
                let non_http = tab.ed.kind != Kind::Http;
                self.tabs.push(tab);
                self.active = self.tabs.len() - 1;
                self.focus = if non_http {
                    Focus::Bar(0)
                } else {
                    Focus::Bar(1)
                };
                self.session_dirty = true;
            }
            Err(e) => self.set_status(format!("cannot open: {e}"), StatusKind::Error),
        }
    }

    fn close_tab(&mut self, force: bool) {
        let Some(t) = self.tab() else { return };
        if t.dirty() && !force {
            let id = t.id;
            let name = t.title();
            let message = if t.path.is_none() {
                format!("\"{name}\" has no file yet. Save it? (Save asks for a name and a folder.)")
            } else {
                format!("\"{name}\" has unsaved changes.")
            };
            self.overlay = Some(Overlay::Confirm(Confirm {
                title: "Unsaved changes".into(),
                message,
                buttons: vec!["Save".into(), "Discard".into(), "Cancel".into()],
                sel: 0,
                action: ConfirmAction::CloseTab(id),
            }));
            return;
        }
        if let Some(mut t) = (self.active < self.tabs.len()).then(|| self.tabs.remove(self.active))
            && let Some(r) = t.running.take()
        {
            r.task.abort();
        }
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
        if self.tabs.is_empty() {
            self.focus = Focus::Sidebar;
        }
        self.session_dirty = true;
    }

    fn request_quit(&mut self) {
        let dirty = self.tabs.iter().filter(|t| t.dirty()).count();
        if dirty > 0 && self.project.config.ui.confirm_quit {
            self.overlay = Some(Overlay::Confirm(Confirm {
                title: "Quit Sendito?".into(),
                message: format!(
                    "{} unsaved changes. They are kept as drafts and restored next time.",
                    if dirty == 1 {
                        "1 tab has".to_string()
                    } else {
                        format!("{dirty} tabs have")
                    }
                ),
                buttons: vec!["Quit".into(), "Cancel".into()],
                sel: 0,
                action: ConfirmAction::Quit,
            }));
        } else {
            self.quit = true;
        }
    }

    pub fn save(&mut self) {
        let Some(t) = self.tab() else { return };
        let Some(path) = t.path.clone() else {
            let id = t.id;
            let name = t.name.clone();
            self.prompt_save_as(id, &name);
            return;
        };
        let model = t.model.clone();
        match self.project.coll.save(&path, &model) {
            Ok(_) => {
                if let Some(t) = self.tab_mut() {
                    t.saved = Some(model);
                }
                self.rescan();
                self.set_status(format!("saved {path}"), StatusKind::Ok);
                self.session_dirty = true;
            }
            Err(e) => self.set_status(format!("save failed: {e}"), StatusKind::Error),
        }
    }

    fn prompt_save_as(&mut self, tab: u64, name: &str) {
        let folders = self.tree.all_folders();
        // A request that has a file is saved next to it; a new one goes to the
        // collection root unless the user picks a folder (the tree's selection
        // is often just its first row, which says nothing about this request).
        let target = self
            .tab_by_id(tab)
            .and_then(|t| t.path.clone().or_else(|| t.origin_path.clone()))
            .map(|p| sendito_store::collection::parent_rel(&p).to_string())
            .unwrap_or_default();
        let idx = folders.iter().position(|f| *f == target).unwrap_or(0);
        let labels: Vec<String> = folders
            .iter()
            .map(|f| {
                if f.is_empty() {
                    "/ (collection root)".into()
                } else {
                    format!("/{f}")
                }
            })
            .collect();
        let mut input = TextEditor::single(if name.starts_with("untitled") {
            ""
        } else {
            name
        });
        input.move_to_end();
        self.overlay = Some(Overlay::Prompt(Prompt {
            title: "Save request".into(),
            label: "Name".into(),
            input,
            choices: vec![("In folder".into(), labels, idx)],
            focus: 0,
            action: PromptAction::SaveAs { tab },
            error: None,
            note: None,
            open: None,
        }));
    }

    pub fn rescan(&mut self) {
        match self.project.coll.scan() {
            Ok(root) => self.tree.set_root(root),
            Err(e) => self.set_status(format!("cannot read collection: {e}"), StatusKind::Error),
        }
    }

    // ----- sending ------------------------------------------------------

    pub fn send(&mut self, confirmed: bool) {
        self.send_opts(confirmed, false);
    }

    /// Sends the active tab's request. `from_watch` marks an automatic repeat:
    /// no note about unsaved edits, and the watch cycle restarts.
    fn send_opts(&mut self, confirmed: bool, from_watch: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            self.set_status("open or create a request first (Ctrl+N)", StatusKind::Warn);
            return;
        };
        tab.refresh_model();
        // On an open client/bidi stream, Send pushes the editor content as more messages.
        if let Some(r) = &tab.running {
            if r.streaming_input {
                let json = tab.ed.message.text();
                let compact = serde_json::from_str::<serde_json::Value>(&json)
                    .map(|v| v.to_string())
                    .unwrap_or_else(|_| json.split_whitespace().collect::<Vec<_>>().join(" "));
                let shown: String = if compact.chars().count() > 32 {
                    compact.chars().take(31).chain(['…']).collect()
                } else {
                    compact
                };
                if let Some(c) = &r.control {
                    let _ = c.send(GrpcControl::Send(json));
                }
                // Select the sent text so the next message can be typed over it.
                tab.ed.message.select_all();
                let sent = tab
                    .response
                    .as_ref()
                    .map(|r| {
                        r.grpc_events
                            .iter()
                            .filter(|e| matches!(e, sendito_grpc::Event::Sent { .. }))
                            .count()
                    })
                    .unwrap_or(0)
                    + 1;
                let end = self.keymap.hint(Action::EndStream, self.vim);
                self.set_status(
                    format!("message {sent} sent: {shown} · {end} ends the stream"),
                    StatusKind::Ok,
                );
                return;
            }
            self.set_status(
                "a request is already running (Ctrl+W x cancels it)",
                StatusKind::Warn,
            );
            return;
        }
        let req = tab.model.clone();
        if let Request::Grpc(g) = &req
            && (g.service.is_empty() || g.method.is_empty())
        {
            self.focus = Focus::Bar(1);
            self.set_status(
                "choose a method first: Enter on the method field lists the services",
                StatusKind::Warn,
            );
            return;
        }
        if let Request::Mcp(m) = &req
            && (m.server.trim().is_empty() || m.tool.trim().is_empty())
        {
            self.focus = Focus::ReqTabs;
            self.set_status(
                "choose a server and a tool first: F2 on each field lists them",
                StatusKind::Warn,
            );
            return;
        }
        if !confirmed {
            let un = runner::unresolved(&req, &self.names);
            let empty = !sendito_core::template::empty_placeholders(
                &sendito_store::request_file::to_toml(&req),
            )
            .is_empty();
            if !un.is_empty() || empty {
                let env = self
                    .active_env
                    .clone()
                    .unwrap_or_else(|| "no environment".into());
                let mut message = String::new();
                if !un.is_empty() {
                    let list = un
                        .iter()
                        .map(|u| format!("{{{{{u}}}}}"))
                        .collect::<Vec<_>>()
                        .join(", ");
                    message.push_str(&format!(
                        "{list} not defined in the collection variables or \"{env}\". "
                    ));
                }
                if empty {
                    message.push_str("The request contains {{}} with no variable name. ");
                }
                message.push_str(
                    "Send anyway sends them as typed, braces included. To define them, cancel and \
                     open the environment (Ctrl+W V).",
                );
                self.overlay = Some(Overlay::Confirm(Confirm {
                    title: "Undefined variables".into(),
                    message,
                    buttons: vec!["Send anyway".into(), "Cancel".into()],
                    sel: 1,
                    action: ConfirmAction::SendAnyway,
                }));
                return;
            }
        }
        let id = tab.id;
        let name = tab.name.clone();
        // A history copy is recorded under the request it came from.
        let path = tab.path.clone().or_else(|| tab.origin_path.clone());
        let edited = tab.path.is_some() && tab.dirty();
        let mut resp = Response::new();
        resp.summary.status = String::new();
        tab.response = Some(resp);
        // A new request makes earlier messages stale.
        self.status = None;
        if edited && !from_watch {
            let save = self.keymap.hint(Action::Save, self.vim);
            self.set_status(
                format!("sent with your unsaved edits ({save} saves them to the file)"),
                StatusKind::Info,
            );
        }
        let envs = self.project.envs.clone();
        let active = self.active_env.clone();
        let runtime = self.runtime.clone();
        let tx = self.tx.clone();
        let then = match req {
            Request::Http(h) => ScopeThen::Http {
                tab: id,
                req: Box::new(h),
                name,
                path,
                from_watch,
            },
            Request::Grpc(g) => ScopeThen::Grpc {
                tab: id,
                req: Box::new(g),
                name,
                path,
            },
            Request::Mcp(m) => ScopeThen::Mcp {
                tab: id,
                req: Box::new(m),
                name,
                path,
            },
        };
        let task = self.rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                runner::build_scope(&envs, active.as_deref(), &runtime)
            })
            .await
            .unwrap_or_else(|e| (Scope::new(), vec![e.to_string()]));
            let _ = tx.send(AppMsg::Scope {
                result: Box::new(result),
                then,
            });
        });
        let tab = &mut self.tabs[self.active];
        tab.running = Some(Running {
            task,
            control: None,
            started: Instant::now(),
            streaming_input: false,
        });
        // The watched request was just sent: the next repeat is a full interval
        // away, also after a manual send.
        if let Some(w) = &mut tab.watch {
            w.next = Instant::now() + w.interval;
        }
    }

    fn run_ctx(&self, name: String, path: Option<String>) -> RunCtx {
        RunCtx {
            root: self.project.root.clone(),
            send: self.project.send_options(),
            history: if self.project.config.history.enabled {
                Some((
                    sendito_store::history::history_path(&self.project.root),
                    self.project.config.history.max_entries,
                ))
            } else {
                None
            },
            name,
            path,
            env: self.active_env.clone(),
            schemas: self.schemas.clone(),
            grpc_deadline_ms: self.project.config.grpc.deadline_ms,
            mask_echoed_secrets: true,
            cookie_jar: self.project.config.http.cookie_jar,
            oauth: Some(runner::OAuthRunInfo::of(&self.project)),
        }
    }

    fn start_with_scope(&mut self, scope: Scope, errors: Vec<String>, then: ScopeThen) {
        let tx = self.tx.clone();
        match then {
            ScopeThen::Http {
                tab,
                req,
                name,
                path,
                from_watch,
            } => {
                if self.tabs.iter().all(|t| t.id != tab || t.running.is_none()) {
                    return; // cancelled meanwhile
                }
                let mut ctx = self.run_ctx(name, path);
                if from_watch {
                    ctx.history = None;
                }
                let task = self.rt.spawn(async move {
                    let outcome = runner::exec_http(&ctx, &req, scope, errors).await;
                    let _ = tx.send(AppMsg::Http {
                        tab,
                        outcome: Box::new(outcome),
                    });
                });
                if let Some(t) = self.tab_by_id(tab)
                    && let Some(r) = &mut t.running
                {
                    r.task = task;
                }
            }
            ScopeThen::Grpc {
                tab,
                req,
                name,
                path,
            } => {
                if self.tabs.iter().all(|t| t.id != tab || t.running.is_none()) {
                    return;
                }
                let ctx = self.run_ctx(name, path);
                let (utx, mut urx) = mpsc::unbounded_channel::<GrpcUpdate>();
                let (ctx_tx, ctx_rx) = mpsc::unbounded_channel::<GrpcControl>();
                let fwd = tx.clone();
                let task = self.rt.spawn(async move {
                    let run = runner::exec_grpc(&ctx, &req, scope, errors, utx, ctx_rx);
                    let forward = async {
                        while let Some(u) = urx.recv().await {
                            let _ = fwd.send(AppMsg::Grpc { tab, update: u });
                        }
                    };
                    tokio::join!(run, forward);
                });
                if let Some(t) = self.tab_by_id(tab)
                    && let Some(r) = &mut t.running
                {
                    r.task = task;
                    r.control = Some(ctx_tx);
                }
            }
            ScopeThen::Mcp {
                tab,
                req,
                name,
                path,
            } => {
                if self.tabs.iter().all(|t| t.id != tab || t.running.is_none()) {
                    return;
                }
                let ctx = self.run_ctx(name, path);
                let info = runner::McpRunInfo::of(&self.project);
                let (utx, mut urx) = mpsc::unbounded_channel::<runner::McpUpdate>();
                let fwd = tx.clone();
                let task = self.rt.spawn(async move {
                    let run = runner::exec_mcp(&ctx, &req, scope, errors, &info, utx);
                    let forward = async {
                        while let Some(u) = urx.recv().await {
                            let _ = fwd.send(AppMsg::Mcp { tab, update: u });
                        }
                    };
                    tokio::join!(run, forward);
                });
                if let Some(t) = self.tab_by_id(tab)
                    && let Some(r) = &mut t.running
                {
                    r.task = task;
                }
            }
        }
    }

    fn cancel(&mut self) {
        if let Some((_, task)) = self.cookies_job.take() {
            task.abort();
            self.set_status("cookie command cancelled", StatusKind::Info);
            return;
        }
        if let Some((_, task)) = self.oauth_login.take() {
            task.abort();
            self.set_status("OAuth login cancelled", StatusKind::Info);
            return;
        }
        if let Some((_, task)) = self.mcp_discovery.take() {
            task.abort();
            self.set_status("MCP tool discovery cancelled", StatusKind::Info);
            return;
        }
        let Some(t) = self.tab_mut() else { return };
        let Some(r) = &t.running else {
            let watched = t.watch.take().is_some();
            self.set_status(
                if watched {
                    "watch off"
                } else {
                    "nothing is running"
                },
                StatusKind::Info,
            );
            return;
        };
        if let Some(c) = &r.control {
            let _ = c.send(GrpcControl::Cancel);
            return;
        }
        if let Some(r) = t.running.take() {
            r.task.abort();
        }
        // Cancelling also stops the watch: it would just re-send what was
        // cancelled.
        let watch_off = t.watch.take().is_some();
        let resp = t.response.get_or_insert_with(Response::new);
        resp.summary.error = Some("cancelled".into());
        resp.summary.status = "Cancelled".into();
        // "Cancelled" in the response border and status line says it all.
        self.clear_transient_status();
        if watch_off {
            self.set_status("cancelled · watch off", StatusKind::Info);
        }
    }

    fn end_stream(&mut self) {
        let Some(t) = self.tab_mut() else { return };
        match t.running.as_mut() {
            Some(r) if r.streaming_input => {
                if let Some(c) = &r.control {
                    let _ = c.send(GrpcControl::EndStream);
                }
                r.streaming_input = false;
                // Nothing more can be sent: drop the "type over it" selection.
                t.ed.message.anchor = None;
            }
            _ => self.set_status("no open client stream", StatusKind::Info),
        }
    }

    pub fn on_msg(&mut self, msg: AppMsg) {
        match msg {
            AppMsg::Scope { result, then } => {
                let (scope, errors) = *result;
                self.start_with_scope(scope, errors, then);
            }
            AppMsg::Http { tab, outcome } => self.on_http(tab, *outcome),
            AppMsg::Grpc { tab, update } => self.on_grpc(tab, update),
            AppMsg::Mcp { tab, update } => self.on_mcp(tab, update),
            AppMsg::OAuthUrl { job, url } => {
                if self.oauth_login.as_ref().is_some_and(|(id, _)| *id == job) {
                    let text = format!(
                        "Open this URL in a browser if it did not open automatically:\n\n{url}\n\nWaiting for the callback. Close this dialog and use Ctrl+W x to cancel.\n"
                    );
                    if self.overlay.is_none() {
                        self.show_report("HTTP OAuth login", &text);
                    } else {
                        self.last_report = Some(("HTTP OAuth login".into(), text));
                        self.set_status("OAuth URL available with :report", StatusKind::Info);
                    }
                }
            }
            AppMsg::OAuthDone { job, result } => {
                if self.oauth_login.as_ref().is_some_and(|(id, _)| *id == job) {
                    self.oauth_login = None;
                    if matches!(&self.overlay, Some(Overlay::Report(r)) if r.title == "HTTP OAuth login")
                    {
                        self.overlay = None;
                    }
                    match result {
                        Ok(message) => self.set_status(message, StatusKind::Ok),
                        Err(e) => self.set_status(e, StatusKind::Error),
                    }
                }
            }
            AppMsg::Cookies { job, result } => {
                if self.cookies_job.as_ref().is_some_and(|(id, _)| *id == job) {
                    self.cookies_job = None;
                    self.status = None;
                    match result {
                        Ok(CookiesOutcome::List(rows)) => {
                            let text = if rows.is_empty() {
                                "No cookies in this environment.\n".to_string()
                            } else {
                                format!(
                                    "{}\n\n:cookies delete <number> removes one; :cookies clear removes all.\n",
                                    rows.join("\n")
                                )
                            };
                            if self.overlay.is_none() {
                                self.overlay = Some(Overlay::Report(Report {
                                    title: "Cookie jar".into(),
                                    viewer: Viewer::new(Doc::text(&text)),
                                    copy_text: text,
                                    copied: false,
                                    close_status: None,
                                }));
                            } else {
                                self.last_report = Some(("Cookie jar".into(), text));
                                self.set_status("cookies listed with :report", StatusKind::Info);
                            }
                        }
                        Ok(CookiesOutcome::Done(m)) => self.set_status(m, StatusKind::Ok),
                        Err(e) => self.set_status(format!("cookie jar: {e}"), StatusKind::Error),
                    }
                }
            }
            AppMsg::McpTools {
                job,
                tab,
                server,
                result,
            } => {
                if !self
                    .mcp_discovery
                    .as_ref()
                    .is_some_and(|(id, _)| *id == job)
                {
                    return;
                }
                self.mcp_discovery = None;
                let active = self.tab().is_some_and(|t| {
                    t.id == tab && t.ed.kind == Kind::Mcp && t.ed.mcp_server.text() == server
                });
                if active && self.overlay.is_none() {
                    match result {
                        Ok(tools) if tools.is_empty() => {
                            self.set_status("the server has no tools", StatusKind::Info)
                        }
                        Ok(tools) => {
                            let items: Vec<_> = tools.iter().map(|t| t.name.clone()).collect();
                            let details = tools
                                .iter()
                                .map(|t| t.description.clone().unwrap_or_default())
                                .collect();
                            let current = self.tab().and_then(|t| {
                                items.iter().position(|name| *name == t.ed.mcp_tool.text())
                            });
                            if let Some(t) = self.tab_mut() {
                                t.mcp_tools = tools;
                            }
                            self.overlay = Some(Overlay::Picker(Picker::new(
                                "Choose an MCP tool",
                                items,
                                details,
                                PickAction::McpTool,
                                current,
                            )));
                        }
                        Err(e) => self.set_status(format!("tools: {e}"), StatusKind::Error),
                    }
                }
            }
            AppMsg::Schema {
                tab,
                result,
                open_picker,
                template,
            } => {
                let mut status = None;
                let mut status_ok: Option<String> = None;
                if let Some(t) = self.tab_by_id(tab) {
                    match result {
                        Ok(svcs) => {
                            let n: usize = svcs.iter().map(|s| s.methods.len()).sum();
                            let count = |n: usize, one: &str| {
                                if n == 1 {
                                    format!("1 {one}")
                                } else {
                                    format!("{n} {one}s")
                                }
                            };
                            status_ok = Some(format!(
                                "schema loaded: {}, {}",
                                count(svcs.len(), "service"),
                                count(n, "method")
                            ));
                            t.services = Some(svcs);
                            t.schema_error = None;
                            if let Some(tpl) = template {
                                t.ed.message.replace_all(&tpl);
                                t.refresh_model();
                                status_ok = Some(
                                    "message replaced by a template with every field of the input type (Ctrl+Z undoes)"
                                        .into(),
                                );
                            }
                        }
                        Err(e) => {
                            status = Some(format!("schema: {e}"));
                            t.schema_error = Some(e);
                        }
                    }
                }
                if let Some(s) = status {
                    self.set_status(s, StatusKind::Error);
                } else {
                    if let Some(s) = status_ok {
                        self.set_status(s, StatusKind::Ok);
                    }
                    if open_picker {
                        self.open_grpc_method_picker();
                    }
                }
            }
            AppMsg::History(h) => {
                self.history_sel = h.len().saturating_sub(1).min(self.history_sel);
                self.history_sel = 0;
                let mut h = h;
                h.reverse();
                self.history = Some(h);
            }
        }
    }

    fn apply_updates(&mut self, updates: Vec<(String, VarValue)>) {
        if updates.is_empty() {
            return;
        }
        for (k, v) in updates {
            self.runtime.insert(k, v);
        }
        self.refresh_names();
        self.session_dirty = true;
    }

    pub fn refresh_names(&mut self) {
        self.names = runner::names_scope(
            &self.project.envs,
            self.active_env.as_deref(),
            &self.runtime,
        );
    }

    fn on_http(&mut self, tab_id: u64, mut o: HttpOutcome) {
        let updates = o.scripts.updates();
        let mut status: Option<(String, StatusKind)> = None;
        if let Some(t) = self.tab_by_id(tab_id) {
            let elapsed_ms = t
                .running
                .as_ref()
                .map(|r| r.started.elapsed().as_secs_f64() * 1000.0);
            t.running = None;
            let mut resp = Response::new();
            match &o.response {
                Ok(r) => {
                    resp.summary = Summary {
                        status: format!("{} {}", r.status, r.reason),
                        code: Some(r.status),
                        ok: r.status < 400,
                        time_ms: Some(r.timing.total.as_secs_f64() * 1000.0),
                        size: Some(o.body_size),
                        decoded: r
                            .decoded
                            .as_ref()
                            .map(|d| format!("{} {}", d.encoding, human_size(d.raw_len))),
                        error: None,
                    };
                    resp.body = Viewer::new(std::mem::take(&mut o.doc));
                    resp.headers = crate::tab::wrapped(Doc::text(&headers_text(r, &o)));
                    let mut timing = timing_text(r, o.body_size);
                    if let Some(w) = &mut t.watch {
                        w.record(r.status < 400, r.timing.total.as_secs_f64() * 1000.0);
                        if !w.samples.is_empty() {
                            timing.push_str(&watch_text(&w.samples));
                        }
                    }
                    resp.timing = crate::tab::wrapped(Doc::text(&timing));
                    let fails = o.scripts.failed_asserts();
                    if !o.scripts.asserts.is_empty() {
                        status = Some(if fails == 0 {
                            (
                                format!("{} passed", assertions(o.scripts.asserts.len())),
                                StatusKind::Ok,
                            )
                        } else {
                            (
                                format!(
                                    "{fails} of {} failed (see Results)",
                                    assertions(o.scripts.asserts.len())
                                ),
                                StatusKind::Error,
                            )
                        });
                    }
                    if r.truncated {
                        status = Some((
                            "response body truncated at the configured limit".into(),
                            StatusKind::Warn,
                        ));
                    }
                }
                Err(e) => {
                    if let Some(w) = &mut t.watch {
                        w.record(false, elapsed_ms.unwrap_or(0.0));
                        resp.timing = crate::tab::wrapped(Doc::text(&watch_text(&w.samples)));
                    }
                    resp.summary = Summary {
                        status: "Error".into(),
                        error: Some(e.clone()),
                        ..Default::default()
                    };
                    resp.body = crate::tab::wrapped(Doc::text(&format!(
                        "Request failed: {}\n\nDetails:\n{e}\n",
                        explain_error(e)
                    )));
                    resp.headers = crate::tab::wrapped(Doc::text(&request_headers_text(&o)));
                    // The reason is in the response pane; the status line names the
                    // outcome only (it would be cut short anyway).
                    status = Some((
                        format!("{} · details in the response pane", short_error(e)),
                        StatusKind::Error,
                    ));
                }
            }
            if !o.scripts.asserts.is_empty() {
                let pass = o.scripts.asserts.iter().filter(|a| a.pass).count();
                let total = o.scripts.asserts.len();
                resp.results_badge = Some(format!(
                    "{pass}/{total} {}",
                    if pass == total { "✓" } else { "✗" }
                ));
            }
            resp.messages = crate::tab::wrapped(Doc::text(&messages_text(
                &o.scripts,
                &o.scope_errors,
                &o.resolved.notes,
            )));
            t.response = Some(resp);
        }
        self.apply_updates(updates);
        if let Some((s, k)) = status {
            self.set_status(s, k);
        }
        if self.history.is_some() {
            self.load_history();
        }
    }

    fn on_grpc(&mut self, tab_id: u64, u: GrpcUpdate) {
        let mut status: Option<(String, StatusKind)> = None;
        let mut updates = Vec::new();
        let end_key = self.keymap.hint(Action::EndStream, self.vim);
        let Some(t) = self.tab_by_id(tab_id) else {
            return;
        };
        let resp = t.response.get_or_insert_with(Response::new);
        // Follow the log while the cursor is at its end.
        let before = resp.messages.row_count();
        // A unary call's log is short and reads from the top.
        let follow = resp.messages.cursor + 2 >= before
            && resp.grpc_kind != Some(sendito_grpc::schema::CallKind::Unary);
        match u {
            GrpcUpdate::Started { resolved, kind } => {
                resp.grpc_kind = Some(kind);
                // The running badge in the response border says what runs; the
                // status stays empty until the call finishes.
                resp.summary.status = String::new();
                if let Some(r) = &mut t.running {
                    r.streaming_input = kind.client_streams();
                }
                // The editor's message went out first: confirm it, as for later ones.
                if kind.client_streams() {
                    t.ed.message.select_all();
                    status = Some((
                        format!(
                            "message 1 sent · type the next one and send again, or {end_key} ends the stream"
                        ),
                        StatusKind::Ok,
                    ));
                }
                // Two short lines, so a narrow pane does not orphan a word.
                let line = format!(
                    "{}: /{}/{}\n  at {}\n",
                    kind.label(),
                    resolved.service,
                    resolved.method,
                    resolved.uri
                );
                resp.messages.doc.append_text(&line);
            }
            GrpcUpdate::Event(ev) => {
                resp.messages.doc.append_text(&event_text(&ev));
                match &ev {
                    sendito_grpc::Event::Received { json, .. } => {
                        resp.body = Viewer::new(Doc::from_bytes(
                            json.as_bytes().to_vec(),
                            Some("application/json"),
                        ));
                    }
                    sendito_grpc::Event::Finished { code, message, .. } => {
                        let name = sendito_grpc::code_name(*code);
                        resp.summary.status = if message.is_empty() {
                            format!("{name} ({code})")
                        } else {
                            format!("{name} ({code}): {message}")
                        };
                        resp.summary.ok = *code == 0;
                        resp.summary.code = None;
                        resp.summary.time_ms = Some(match &ev {
                            sendito_grpc::Event::Finished { at, .. } => at.as_secs_f64() * 1000.0,
                            _ => 0.0,
                        });
                        if *code != 0 {
                            resp.summary.error = Some(format!("{name}: {message}"));
                        }
                    }
                    _ => {}
                }
                resp.grpc_events.push(ev);
                let size: usize = resp
                    .grpc_events
                    .iter()
                    .map(|e| {
                        if let sendito_grpc::Event::Received { size, .. } = e {
                            *size
                        } else {
                            0
                        }
                    })
                    .sum();
                resp.summary.size = Some(size as u64);
                resp.headers =
                    crate::tab::wrapped(Doc::text(&grpc_headers_text(&resp.grpc_events)));
                resp.timing = crate::tab::wrapped(Doc::text(&grpc_timing_text(&resp.grpc_events)));
            }
            GrpcUpdate::Error(e) => {
                resp.messages.doc.append_text(&format!("error: {e}\n"));
                if resp.grpc_events.is_empty() {
                    resp.summary.status = "Error".into();
                    resp.summary.error = Some(e.clone());
                    resp.body = crate::tab::wrapped(Doc::text(&format!("Call failed:\n\n{e}\n")));
                }
                status = Some((e, StatusKind::Error));
            }
            GrpcUpdate::Done {
                scripts,
                scope_errors,
            } => {
                t.running = None;
                let text = messages_text(&scripts, &scope_errors, &[]);
                if !text.trim().is_empty() {
                    resp.messages.doc.append_text(&format!("\n{text}"));
                }
                updates = scripts.updates();
                let fails = scripts.failed_asserts();
                // Replaces the "stream open" note: the call is over.
                let result = if resp.summary.status.is_empty() {
                    "request finished".to_string()
                } else {
                    format!("request finished: {}", resp.summary.status)
                };
                let ok = resp.summary.ok;
                status = Some(if scripts.asserts.is_empty() {
                    (
                        result,
                        if ok {
                            StatusKind::Ok
                        } else {
                            StatusKind::Error
                        },
                    )
                } else if fails == 0 {
                    (
                        format!("{result} · {} passed", assertions(scripts.asserts.len())),
                        if ok {
                            StatusKind::Ok
                        } else {
                            StatusKind::Error
                        },
                    )
                } else {
                    (
                        format!(
                            "{result} · {fails} of {} failed (see Log)",
                            assertions(scripts.asserts.len())
                        ),
                        StatusKind::Error,
                    )
                });
            }
        }
        if follow {
            resp.messages.bottom();
        }
        self.apply_updates(updates);
        if let Some((s, k)) = status {
            self.set_status(s, k);
        }
    }

    fn on_mcp(&mut self, tab_id: u64, u: runner::McpUpdate) {
        let mut status: Option<(String, StatusKind)> = None;
        let mut updates = Vec::new();
        let Some(t) = self.tab_by_id(tab_id) else {
            return;
        };
        let resp = t.response.get_or_insert_with(Response::new);
        match u {
            runner::McpUpdate::Event(ev) => {
                if let Some(json) = mcp_sent_line(&ev) {
                    resp.headers.doc.append_text(&format!("{json}\n"));
                }
                let line = mcp_event_text(&ev);
                resp.messages.doc.append_text(&line);
                resp.messages.bottom();
            }
            runner::McpUpdate::Done {
                error,
                is_error,
                result,
                scripts,
                scope_errors,
            } => {
                let time_ms = t
                    .running
                    .as_ref()
                    .map(|r| r.started.elapsed().as_secs_f64() * 1000.0);
                t.running = None;
                let size = result.as_ref().map(|b| b.len() as u64);
                if let Some(ms) = time_ms {
                    resp.timing = crate::tab::wrapped(Doc::text(&format!(
                        "Total: {:.1} ms\nIncludes scope preparation, handshake and tool call.\n",
                        ms
                    )));
                }
                // MCP has no HTTP status: `code` stays `None` so the border and
                // the chrome colour by `ok`/`error` instead of a status class.
                resp.summary = match (&error, is_error) {
                    (Some(e), _) => Summary {
                        status: "error".into(),
                        code: None,
                        ok: false,
                        time_ms,
                        size,
                        decoded: None,
                        error: Some(e.clone()),
                    },
                    (None, true) => Summary {
                        status: "TOOL ERROR".into(),
                        code: None,
                        ok: false,
                        time_ms,
                        size,
                        decoded: None,
                        error: None,
                    },
                    (None, false) => Summary {
                        status: "OK".into(),
                        code: None,
                        ok: true,
                        time_ms,
                        size,
                        decoded: None,
                        error: None,
                    },
                };
                if let Some(body) = result {
                    resp.body = Viewer::new(Doc::from_bytes(body, Some("application/json")));
                } else if let Some(e) = &error {
                    resp.body = crate::tab::wrapped(Doc::text(&format!("Call failed:\n\n{e}\n")));
                }
                let text = messages_text(&scripts, &scope_errors, &[]);
                if !text.trim().is_empty() {
                    resp.messages.doc.append_text(&format!("\n{text}"));
                }
                updates = scripts.updates();
                let fails = scripts.failed_asserts();
                let result_line = if let Some(e) = &error {
                    format!("call failed: {e}")
                } else if is_error {
                    "tool reported an error (isError)".to_string()
                } else {
                    "call finished".to_string()
                };
                status = Some(if scripts.asserts.is_empty() {
                    (
                        result_line,
                        if error.is_none() && !is_error {
                            StatusKind::Ok
                        } else {
                            StatusKind::Error
                        },
                    )
                } else if fails == 0 {
                    (
                        format!(
                            "{result_line} · {} passed",
                            assertions(scripts.asserts.len())
                        ),
                        if error.is_none() && !is_error {
                            StatusKind::Ok
                        } else {
                            StatusKind::Error
                        },
                    )
                } else {
                    (
                        format!(
                            "{result_line} · {fails} of {} failed (see Log)",
                            assertions(scripts.asserts.len())
                        ),
                        StatusKind::Error,
                    )
                });
            }
        }
        self.apply_updates(updates);
        if let Some((s, k)) = status {
            self.set_status(s, k);
        }
    }

    // ----- MCP discovery ------------------------------------------------

    fn open_mcp_server_picker(&mut self) {
        let Some(t) = self.tab().filter(|t| t.ed.kind == Kind::Mcp) else {
            return;
        };
        let current = t.ed.mcp_server.text();
        let items: Vec<String> = self.project.config.mcp.servers.keys().cloned().collect();
        if items.is_empty() {
            self.set_status("no servers configured in [mcp.servers]", StatusKind::Warn);
            return;
        }
        let selected = items.iter().position(|name| *name == current);
        self.overlay = Some(Overlay::Picker(Picker::new(
            "Choose an MCP server",
            items,
            vec![],
            PickAction::McpServer,
            selected,
        )));
    }

    fn load_mcp_tools(&mut self) {
        let Some(t) = self.tab().filter(|t| t.ed.kind == Kind::Mcp) else {
            return;
        };
        let tab = t.id;
        let server = t.ed.mcp_server.text();
        if server.trim().is_empty() {
            self.set_status("choose an MCP server first", StatusKind::Warn);
            return;
        }
        let info = runner::McpRunInfo::of(&self.project);
        let envs = self.project.envs.clone();
        let env = self.active_env.clone();
        let runtime = self.runtime.clone();
        let tx = self.tx.clone();
        let server2 = server.clone();
        if let Some((_, task)) = self.mcp_discovery.take() {
            task.abort();
        }
        self.mcp_discovery_seq = self.mcp_discovery_seq.wrapping_add(1);
        let job = self.mcp_discovery_seq;
        self.set_status("loading MCP tools… (Ctrl+W x cancels)", StatusKind::Info);
        let task = self.rt.spawn(async move {
            let result = async {
                let (scope, errors) = tokio::task::spawn_blocking(move || {
                    runner::build_scope(&envs, env.as_deref(), &runtime)
                })
                .await
                .map_err(|e| e.to_string())?;
                if !errors.is_empty() {
                    return Err(errors.join("; "));
                }
                let name = scope.render(&server2).text;
                let transport = runner::mcp_transport(&info, &name, &scope)?;
                let opts = sendito_mcp::Options {
                    timeout: std::time::Duration::from_millis(info.timeout_ms.max(1)),
                };
                let (events, _rx) = mpsc::unbounded_channel();
                sendito_mcp::list_tools(&transport, &opts, events)
                    .await
                    .map_err(|e| sendito_core::vars::scrub(&e.to_string(), &scope.secret_values()))
            }
            .await;
            let _ = tx.send(AppMsg::McpTools {
                job,
                tab,
                server,
                result,
            });
        });
        self.mcp_discovery = Some((job, task));
    }

    // ----- gRPC schema --------------------------------------------------

    pub fn load_schema(&mut self, open_picker: bool, template: bool, refresh: bool) {
        let Some(t) = self.tab_mut() else { return };
        if t.ed.kind != Kind::Grpc {
            return;
        }
        t.refresh_model();
        let Request::Grpc(g) = t.model.clone() else {
            return;
        };
        let id = t.id;
        let resolved = sendito_core::resolve::resolve_grpc(&g, &self.names);
        let root = self.project.root.clone();
        let cache = self.schemas.clone();
        let tx = self.tx.clone();
        let svc = g.service.clone();
        let method = g.method.clone();
        self.set_status(
            format!(
                "loading schema from {}…",
                if matches!(g.schema, SchemaSource::Reflection) {
                    resolved.uri.clone()
                } else {
                    "proto files".into()
                }
            ),
            StatusKind::Info,
        );
        self.rt.spawn(async move {
            let r = runner::load_schema(&root, &resolved, &g.schema, &cache, refresh).await;
            let template = match (&r, template) {
                (Ok(s), true) => s.template(&svc, &method).ok(),
                _ => None,
            };
            let _ = tx.send(AppMsg::Schema {
                tab: id,
                result: r.map(|s| s.services()),
                open_picker,
                template,
            });
        });
    }

    fn open_grpc_method_picker(&mut self) {
        let Some(t) = self.tab() else { return };
        let Some(svcs) = t.services.clone() else {
            self.load_schema(true, false, false);
            return;
        };
        let current = format!("{}/{}", t.ed.service, t.ed.grpc_method);
        let mut items = Vec::new();
        let mut details = Vec::new();
        for s in &svcs {
            for m in &s.methods {
                items.push(format!("{}/{}", s.name, m.name));
                details.push(format!("{} · {} → {}", m.kind.label(), m.input, m.output));
            }
        }
        if items.is_empty() {
            self.set_status("the schema has no services", StatusKind::Warn);
            return;
        }
        let cur = items.iter().position(|i| *i == current);
        self.overlay = Some(Overlay::Picker(Picker::new(
            "Choose a gRPC method",
            items,
            details,
            PickAction::GrpcMethod,
            cur,
        )));
    }

    // ----- environment --------------------------------------------------

    fn open_env_picker(&mut self) {
        self.env_list = self.project.envs.list();
        let mut items = vec!["(no environment)".to_string()];
        items.extend(self.env_list.iter().cloned());
        items.push("Manage environments…".into());
        let cur = match &self.active_env {
            Some(e) => self.env_list.iter().position(|x| x == e).map(|i| i + 1),
            None => Some(0),
        };
        self.overlay = Some(Overlay::Picker(Picker::new(
            "Environment",
            items,
            vec![],
            PickAction::Env,
            cur,
        )));
    }

    pub fn set_env(&mut self, env: Option<String>) {
        self.active_env = env;
        // A watch re-resolves variables at every send; a new environment makes
        // that a different request, so watches stop.
        let watched = self.tabs.iter_mut().filter_map(|t| t.watch.take()).count();
        self.refresh_names();
        self.session_dirty = true;
        // The status line and the tab bar show the new environment; an older
        // confirmation would now be stale.
        self.clear_transient_status();
        if watched > 0 {
            self.set_status("watch off (environment changed)", StatusKind::Info);
        }
    }

    pub fn open_env_editor(&mut self) {
        self.clear_transient_status();
        self.env_list = self.project.envs.list();
        let mut envs = vec![COLLECTION_ENV.to_string()];
        envs.extend(self.env_list.iter().cloned());
        let current = self
            .active_env
            .as_ref()
            .and_then(|e| envs.iter().position(|x| x == e))
            .unwrap_or(0);
        let mut ed = EnvEditor {
            envs,
            current,
            table: EnvEditor::table(),
            dirty: false,
            error: None,
        };
        self.load_env_into(&mut ed);
        self.overlay = Some(Overlay::Env(ed));
    }

    fn load_env_into(&self, ed: &mut EnvEditor) {
        let name = ed.envs.get(ed.current).cloned().unwrap_or_default();
        match self.project.envs.load(&name) {
            Ok(env) => ed.load(&env, |v| self.project.envs.is_set(&name, v)),
            Err(e) => ed.error = Some(e.to_string()),
        }
    }

    fn save_env_editor(&mut self, ed: &mut EnvEditor) -> bool {
        let name = ed.envs.get(ed.current).cloned().unwrap_or_default();
        let (env, secrets) = ed.to_env(&name);
        if let Err(e) = self.project.envs.save(&env) {
            ed.error = Some(e.to_string());
            return false;
        }
        for (var, value) in secrets {
            if let Some(v) = env.get(&var)
                && let Err(e) = self.project.envs.store_secret(&name, v, &value)
            {
                ed.error = Some(format!("{var}: {e}"));
                return false;
            }
        }
        self.load_env_into(ed);
        self.refresh_names();
        let label = if name == COLLECTION_ENV {
            "collection variables".to_string()
        } else {
            name
        };
        self.set_status(format!("saved {label}"), StatusKind::Ok);
        true
    }

    // ----- history ------------------------------------------------------

    fn toggle_history(&mut self) {
        if self.sidebar_mode == SidebarMode::History && matches!(self.focus, Focus::Sidebar) {
            self.sidebar_mode = SidebarMode::Tree;
            return;
        }
        self.sidebar_mode = SidebarMode::History;
        self.layout.sidebar_visible = true;
        self.focus = Focus::Sidebar;
        self.load_history();
    }

    pub fn load_history(&mut self) {
        let path = sendito_store::history::history_path(&self.project.root);
        let max = self.project.config.history.max_entries.max(1);
        let tx = self.tx.clone();
        self.rt.spawn(async move {
            let h = tokio::task::spawn_blocking(move || {
                sendito_store::history::load(&path, max).unwrap_or_default()
            })
            .await
            .unwrap_or_default();
            let _ = tx.send(AppMsg::History(h));
        });
    }

    fn open_history(&mut self, i: usize, resend: bool) {
        let Some(e) = self.history.as_ref().and_then(|h| h.get(i)).cloned() else {
            return;
        };
        match sendito_store::request_file::parse(&e.request) {
            Ok(req) => {
                self.new_tab(req);
                if let Some(t) = self.tab_mut() {
                    t.name = e.name.clone();
                    let when = e.ts.get(11..19).unwrap_or(&e.ts);
                    t.origin = Some(format!("from history ({when})"));
                    t.origin_path = e.path.clone();
                    // Unchanged from the history entry: not an unsaved edit.
                    t.saved = Some(t.model.clone());
                }
                if resend {
                    self.status = None;
                    self.send(false);
                    if self.status.is_none() {
                        let when = e.ts.get(11..19).unwrap_or(&e.ts);
                        self.set_status(
                            format!("resent the history entry of {when} ({})", e.name),
                            StatusKind::Info,
                        );
                    }
                }
            }
            Err(err) => self.set_status(
                format!("cannot read history entry: {err}"),
                StatusKind::Error,
            ),
        }
    }

    fn save_history_prompt(&mut self, i: usize) {
        let Some(e) = self.history.as_ref().and_then(|h| h.get(i)).cloned() else {
            return;
        };
        let folders = self.tree.all_folders();
        let labels: Vec<String> = folders
            .iter()
            .map(|f| {
                if f.is_empty() {
                    "/ (collection root)".into()
                } else {
                    format!("/{f}")
                }
            })
            .collect();
        // The entry's own folder, else the folder selected in the tree (as for
        // new requests and folders).
        let target = self.tree.target_folder();
        let idx = e
            .path
            .as_deref()
            .map(parent_rel)
            .and_then(|p| folders.iter().position(|f| f == p))
            .or_else(|| folders.iter().position(|f| *f == target))
            .unwrap_or(0);
        // The original request usually still exists: offer a free name, and
        // say why it differs.
        let folder = folders.get(idx).cloned().unwrap_or_default();
        let exists = |n: &str| {
            self.project
                .root
                .join(sendito_store::collection::join_rel(
                    &folder,
                    &format!("{n}.toml"),
                ))
                .exists()
        };
        let mut name = e.name.clone();
        let mut note = None;
        if exists(&name) {
            let mut k = 2;
            while exists(&format!("{} {k}", e.name)) {
                k += 1;
            }
            name = format!("{} {k}", e.name);
            // No-break spaces keep each name on one line when the note wraps.
            let nb = |t: &str| t.replace(' ', "\u{a0}");
            note = Some(format!(
                "\"{}\" exists in /{}; suggested name \"{}\" is selected: typing replaces it.",
                nb(&e.name),
                nb(&folder),
                nb(&name)
            ));
        }
        let mut input = TextEditor::single(&name);
        input.move_to_end();
        if note.is_some() {
            // A suggested name: selected, so typing replaces it.
            input.select_all();
        }
        self.overlay = Some(Overlay::Prompt(Prompt {
            title: "Save request (from history)".into(),
            label: "Name".into(),
            input,
            choices: vec![("In folder".into(), labels, idx)],
            focus: 0,
            action: PromptAction::SaveHistory(i),
            error: None,
            note,
            open: None,
        }));
    }

    // ----- tree operations ----------------------------------------------

    fn tree_open(&mut self) {
        let Some(row) = self.tree.selected_row().cloned() else {
            return;
        };
        match row.kind {
            RowKind::Folder { .. } => {
                if self.tree.filter.is_empty() {
                    self.tree.toggle(&row.rel);
                    self.session_dirty = true;
                }
            }
            RowKind::Request { .. } => self.open_request(&row.rel),
        }
    }

    /// Folder choice row: all folders, the tree's target folder selected.
    fn folder_choice(&self) -> (String, Vec<String>, usize) {
        let folders = self.tree.all_folders();
        let target = self.tree.target_folder();
        let idx = folders.iter().position(|f| *f == target).unwrap_or(0);
        let labels = folders
            .iter()
            .map(|f| {
                if f.is_empty() {
                    "/ (collection root)".to_string()
                } else {
                    format!("/{f}")
                }
            })
            .collect();
        ("In folder".into(), labels, idx)
    }

    fn prompt_new_request(&mut self) {
        let folder = self.tree.target_folder();
        self.overlay = Some(Overlay::Prompt(Prompt {
            title: "New request".into(),
            label: "Name".into(),
            input: TextEditor::single(""),
            choices: vec![
                (
                    "Type".into(),
                    vec!["HTTP".into(), "gRPC".into(), "MCP".into()],
                    0,
                ),
                self.folder_choice(),
            ],
            focus: 0,
            action: PromptAction::NewRequest { folder },
            error: None,
            note: None,
            open: None,
        }));
    }

    fn prompt_new_folder(&mut self) {
        let folder = self.tree.target_folder();
        self.overlay = Some(Overlay::Prompt(Prompt {
            title: "New folder".into(),
            label: "Name".into(),
            input: TextEditor::single(""),
            choices: vec![self.folder_choice()],
            focus: 0,
            action: PromptAction::NewFolder { folder },
            error: None,
            note: None,
            open: None,
        }));
    }

    fn prompt_rename(&mut self) {
        let Some(row) = self.tree.selected_row().cloned() else {
            return;
        };
        let mut input = TextEditor::single(&row.name);
        input.select_all();
        self.overlay = Some(Overlay::Prompt(Prompt {
            title: format!(
                "Rename {}",
                if row.is_folder() { "folder" } else { "request" }
            ),
            label: "New name".into(),
            input,
            choices: vec![],
            focus: 0,
            action: PromptAction::Rename(row.rel),
            error: None,
            note: None,
            open: None,
        }));
    }

    fn tree_duplicate(&mut self) {
        let Some(rel) = self.tree.selected_rel() else {
            return;
        };
        match self.project.coll.duplicate(&rel) {
            Ok(new_rel) => {
                self.rescan();
                self.tree.select_rel(&new_rel);
                self.set_status(
                    format!("duplicated as {}", name_of(&new_rel)),
                    StatusKind::Ok,
                );
            }
            Err(e) => self.set_status(e.to_string(), StatusKind::Error),
        }
    }

    fn tree_delete(&mut self) {
        let Some(row) = self.tree.selected_row().cloned() else {
            return;
        };
        let what = if row.is_folder() {
            "folder and everything in it"
        } else {
            "request"
        };
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: "Delete".into(),
            message: format!(
                "Delete the {what} \"{}\"? This removes it from disk.",
                row.name
            ),
            buttons: vec!["Delete".into(), "Cancel".into()],
            sel: 1,
            action: ConfirmAction::Delete(row.rel),
        }));
    }

    fn tree_move(&mut self) {
        let Some(rel) = self.tree.selected_rel() else {
            return;
        };
        let folders: Vec<String> = self
            .tree
            .all_folders()
            .into_iter()
            .filter(|f| *f != rel && !f.starts_with(&format!("{rel}/")) && f != parent_rel(&rel))
            .collect();
        if folders.is_empty() {
            self.set_status(
                "there is no other folder to move to (create one with N)",
                StatusKind::Info,
            );
            return;
        }
        let labels = folders
            .iter()
            .map(|f| {
                if f.is_empty() {
                    "/ (collection root)".to_string()
                } else {
                    format!("/{f}")
                }
            })
            .collect();
        self.overlay = Some(Overlay::Picker(Picker {
            details: folders,
            ..Picker::new(
                &format!("Move \"{}\" to", name_of(&rel)),
                labels,
                vec![],
                PickAction::MoveTo(rel),
                None,
            )
        }));
    }

    /// Moves a node, updating open tabs that point into it.
    pub fn move_node(&mut self, rel: &str, folder: &str) {
        match self.project.coll.move_to(rel, folder) {
            Ok(new_rel) => {
                self.repath_tabs(rel, &new_rel);
                self.rescan();
                self.tree.select_rel(&new_rel);
                self.set_status(format!("moved to {new_rel}"), StatusKind::Ok);
            }
            Err(e) => self.set_status(e.to_string(), StatusKind::Error),
        }
    }

    fn repath_tabs(&mut self, old: &str, new: &str) {
        for t in &mut self.tabs {
            if let Some(p) = &t.path {
                if p == old {
                    t.path = Some(new.to_string());
                    t.name = name_of(new).to_string();
                } else if let Some(rest) = p.strip_prefix(&format!("{old}/")) {
                    t.path = Some(format!("{new}/{rest}"));
                }
            }
        }
        self.session_dirty = true;
    }

    // ----- misc actions -------------------------------------------------

    fn open_method_picker(&mut self, prefix: Option<char>) {
        let Some(t) = self.tab() else { return };
        let mut items: Vec<String> = HTTP_METHODS.iter().map(|s| s.to_string()).collect();
        items.push("Custom…".into());
        let cur = items.iter().position(|m| *m == t.ed.method);
        let mut p = Picker::new("HTTP method", items, vec![], PickAction::Method, cur);
        if let Some(c) = prefix {
            p.filter.set_text(&c.to_string());
            p.filter.move_to_end();
            p.sel = 0;
        }
        self.overlay = Some(Overlay::Picker(p));
    }

    fn open_choice_picker(&mut self, f: Field) {
        let Some(t) = self.tab() else { return };
        let items = t.ed.choice_options(f);
        let cur = items.iter().position(|i| *i == t.ed.choice_label(f));
        self.overlay = Some(Overlay::Picker(Picker::new(
            f.label(),
            items,
            vec![],
            PickAction::Choice(f),
            cur,
        )));
    }

    pub fn open_palette(&mut self, prefill: &str) {
        self.clear_transient_status();
        let mut items: Vec<(PaletteItem, String, String)> = Vec::new();
        for (c, d) in COMMANDS {
            items.push((PaletteItem::Command(c, d), c.to_string(), d.to_string()));
        }
        for a in Action::ALL {
            if matches!(
                a,
                Action::CursorUp
                    | Action::CursorDown
                    | Action::PageUp
                    | Action::PageDown
                    | Action::Top
                    | Action::Bottom
                    | Action::ScrollLeft
                    | Action::ScrollRight
                    | Action::Undo
                    | Action::Redo
                    | Action::SelectAll
                    | Action::Copy
                    | Action::Cut
                    | Action::Paste
                    | Action::NextCell
                    | Action::FocusNext
                    | Action::FocusPrev
            ) {
                continue;
            }
            // Up to two keys, e.g. "Ctrl+Enter / F5" (F1 lists them all).
            let keys = self.keymap.keys_for(*a, self.vim);
            let keys = if keys.is_empty() {
                "(unbound)".to_string()
            } else {
                keys.iter().take(2).cloned().collect::<Vec<_>>().join(" / ")
            };
            items.push((PaletteItem::Action(*a), a.description().to_string(), keys));
        }
        for r in self.tree.root.all_requests() {
            items.push((
                PaletteItem::Open(r.rel.clone()),
                format!("Open {} {}", r.badge, r.name),
                r.rel.clone(),
            ));
        }
        items.push((
            PaletteItem::Env(None),
            "Environment: (no environment)".into(),
            String::new(),
        ));
        for e in self.project.envs.list() {
            items.push((
                PaletteItem::Env(Some(e.clone())),
                format!("Environment: {e}"),
                String::new(),
            ));
        }
        let mut input = TextEditor::single(prefill)
            .with_placeholder("type to search actions and requests, or : for commands");
        input.move_to_end();
        let armed = true;
        self.overlay = Some(Overlay::Palette(Palette {
            input,
            items,
            sel: 0,
            scroll: 0,
            armed,
        }));
    }

    pub fn open_help(&mut self) {
        self.clear_transient_status();
        self.overlay = Some(Overlay::Help {
            viewer: crate::tab::wrapped(Doc::text(&help_text(&self.keymap, self.vim))),
            filter: TextEditor::single("").with_placeholder("(type to filter the keys)"),
        });
    }

    fn open_search(&mut self) {
        if self.tab().and_then(|t| t.response.as_ref()).is_none() {
            self.set_status("no response to search yet", StatusKind::Info);
            return;
        }
        self.overlay = Some(Overlay::Prompt(Prompt {
            title: "Search response".into(),
            label: "Find".into(),
            input: TextEditor::single(""),
            choices: vec![],
            focus: 0,
            action: PromptAction::Search,
            error: None,
            note: None,
            open: None,
        }));
    }

    pub fn open_import(&mut self, prefill: &str) {
        let mut input = TextEditor::multi(prefill)
            .with_placeholder("curl https://…   or   path/to/collection.json or Bruno directory");
        input.move_to_end();
        self.overlay = Some(Overlay::Import(Import { input, error: None }));
    }

    fn import_curl(&mut self, text: &str) {
        match sendito_import::curl::import(text) {
            Ok(r) => {
                // An empty, never-saved tab is reused instead of left behind.
                let blank = {
                    let mut b = Tab::new(0, None, String::new(), &Request::new_http(), None);
                    b.refresh_model();
                    b.model
                };
                let reuse = self.tab().is_some_and(|t| {
                    t.path.is_none()
                        && t.running.is_none()
                        && t.response.is_none()
                        && t.model == blank
                });
                let req = Request::Http(r.request);
                let where_ = if reuse {
                    let (id, name) = self
                        .tab()
                        .map(|t| (t.id, t.name.clone()))
                        .unwrap_or_default();
                    let mut tab = Tab::new(id, None, name, &req, None);
                    tab.ed.set_vim(self.vim);
                    tab.refresh_model();
                    self.tabs[self.active] = tab;
                    self.focus = Focus::Bar(1);
                    self.session_dirty = true;
                    "this tab"
                } else {
                    self.new_tab(req);
                    "a new tab"
                };
                if r.notes.is_empty() {
                    self.set_status(
                        format!("imported the curl command into {where_}"),
                        StatusKind::Ok,
                    );
                } else {
                    self.show_report(
                        "curl import notes",
                        &r.notes
                            .iter()
                            .map(|n| format!("• {n}"))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    );
                }
            }
            Err(e) => self.set_status(format!("curl: {e}"), StatusKind::Error),
        }
    }

    fn do_import(&mut self, text: &str) -> Result<(), String> {
        let t = text.trim();
        if t.is_empty() {
            return Err("paste a curl command or enter a file path".into());
        }
        if t.starts_with("curl ") || t == "curl" {
            self.overlay = None;
            self.import_curl(t);
            return Ok(());
        }
        let (format, result) = if t.starts_with('{') {
            sendito_import::import_json(t)?
        } else {
            // A relative path is looked up in the working directory, then in
            // the collection folder.
            let cwd = std::env::current_dir().unwrap_or_default();
            let mut p = sendito_store::env::expand(&cwd, t);
            if !p.exists() {
                let in_root = sendito_store::env::expand(&self.project.root, t);
                if in_root.exists() {
                    p = in_root;
                }
            }
            sendito_import::import_path(&p)?
        };
        let rep = sendito_import::write(&result, &self.project.coll, Some(&self.project.envs), "")
            .map_err(|e| e.to_string())?;
        self.overlay = None;
        self.rescan();
        self.tree.select_rel(&rep.folder);
        self.refresh_names();
        let here = self.project.name.clone();
        let mut text = format!(
            "{} collection \"{}\" imported as the folder {}/ in this collection ({here}).\n\n  requests:  {}\n  folders:   {}\n  variables: {} (collection variables)\n",
            format.label(),
            result.root.name,
            rep.folder,
            rep.requests,
            rep.folders,
            rep.variables
        );
        if rep.environments > 0 {
            text.push_str(&format!("  environments: {}\n", rep.environments));
        }
        for name in &rep.variable_conflicts {
            text.push_str(&format!(
                "  kept the existing collection variable {name:?}\n"
            ));
        }
        for (original, renamed) in &rep.renamed_environments {
            text.push_str(&format!(
                "  environment {original:?} imported as {renamed:?}\n"
            ));
        }
        if result.skipped.is_empty()
            && rep.variable_conflicts.is_empty()
            && rep.renamed_environments.is_empty()
        {
            text.push_str("\nEverything was imported as it was.\n");
        } else {
            // Three kinds of note, each with what to do about it.
            let (mut lost, mut todo, mut changed) = (Vec::new(), Vec::new(), Vec::new());
            for sk in &result.skipped {
                let line = format!("• {}: {}\n", sk.item, sk.reason);
                if sk.reason.contains("does not define it") {
                    todo.push(line);
                } else if sk.reason.contains("not imported")
                    || sk.reason.contains("not supported")
                    || sk.reason.contains("no file selected")
                    || sk.reason.contains("neither a request")
                {
                    lost.push(line);
                } else {
                    changed.push(line);
                }
            }
            text.push_str(&format!(
                "  report:    {} entries ({} need action, {} skipped, {} imported with changes)\n",
                result.skipped.len(),
                todo.len(),
                lost.len(),
                changed.len()
            ));
            for (title, help, lines) in [
                (
                    "Needs action",
                    "variables the requests use but nothing defines: set them in an environment (F4, Ctrl+W V)",
                    &todo,
                ),
                (
                    "Skipped",
                    "not imported: Sendito has no equivalent; recreate by hand in that request if you need it",
                    &lost,
                ),
                (
                    "Imported with changes",
                    "works as imported; check the request if the change matters to you",
                    &changed,
                ),
            ] {
                if lines.is_empty() {
                    continue;
                }
                text.push_str(&format!("\n{title} ({}) — {help}:\n\n", lines.len()));
                for l in lines {
                    text.push_str(l);
                }
            }
        }
        self.show_report(&format!("{} import", format.label()), &text);
        let notes = result.skipped.len();
        if let Some(Overlay::Report(r)) = &mut self.overlay {
            r.close_status = Some(format!(
                "imported {} requests into {}/{}",
                rep.requests,
                rep.folder,
                if notes == 0 {
                    String::new()
                } else {
                    format!(" · {notes} entries to review (:report shows them again)")
                }
            ));
        }
        Ok(())
    }

    pub fn show_report(&mut self, title: &str, text: &str) {
        self.last_report = Some((title.to_string(), text.to_string()));
        self.overlay = Some(Overlay::Report(Report {
            title: title.into(),
            viewer: Viewer::new(Doc::text(text)),
            copy_text: text.to_string(),
            copied: false,
            close_status: None,
        }));
    }

    pub fn export_curl(&mut self, include_secrets: bool) {
        let Some(t) = self.tab() else { return };
        let Request::Http(h) = &t.model else {
            self.set_status("curl export is for HTTP requests", StatusKind::Warn);
            return;
        };
        let h = h.clone();
        let scope = if include_secrets {
            runner::build_scope(
                &self.project.envs,
                self.active_env.as_deref(),
                &self.runtime,
            )
            .0
        } else {
            self.names.clone()
        };
        let cmd = sendito_import::curl::export(&h, &scope, include_secrets, &self.project.root);
        self.clipboard.set(cmd.clone(), false);
        self.osc52.push(cmd.clone());
        let note = if include_secrets {
            "Sent to the clipboard through the terminal (OSC 52). Some terminals block it: if pasting \
             gives nothing, select the command above. It CONTAINS SECRET VALUES."
        } else {
            "Copied to the clipboard through the terminal (OSC 52). Some terminals block it: if pasting \
             gives nothing, select the command above. Secret variables stay \
             {{placeholders}}; :curl! includes their values."
        };
        // Options that come from the request's settings, explained.
        let mut flags = Vec::new();
        if cmd.contains("\n  -L") || cmd.ends_with(" -L") {
            flags.push("-L follows redirects (Options: follow redirects is on)");
        }
        if cmd.contains("\n  -k") {
            flags.push("-k skips TLS verification (Options: verify TLS is off)");
        }
        if cmd.contains("--max-time") {
            flags.push("--max-time is the request timeout (Options)");
        }
        let flags = if flags.is_empty() {
            String::new()
        } else {
            format!("{}\n\n", flags.join("; "))
        };
        self.overlay = Some(Overlay::Report(Report {
            title: "Copy as curl".into(),
            viewer: Viewer::new(Doc::text(&format!(
                "{cmd}\n\n── notes (not part of the command) ──\n{flags}{note}\n"
            ))),
            copy_text: cmd,
            copied: true,
            close_status: Some("the curl command was sent to the terminal clipboard".into()),
        }));
        self.set_status(
            "curl command sent to the terminal clipboard",
            StatusKind::Ok,
        );
    }

    /// Copies the active request as an .http file (JetBrains HTTP client /
    /// VS Code REST Client).
    fn export_http_file(&mut self, include_secrets: bool) {
        let Some(t) = self.tab() else { return };
        let Request::Http(h) = &t.model else {
            self.set_status(".http export is for HTTP requests", StatusKind::Warn);
            return;
        };
        let name = t.name.clone();
        let h = h.clone();
        let scope = if include_secrets {
            runner::build_scope(
                &self.project.envs,
                self.active_env.as_deref(),
                &self.runtime,
            )
            .0
        } else {
            self.names.clone()
        };
        let file = sendito_import::http_file::export(
            &name,
            &h,
            &scope,
            include_secrets,
            &self.project.root,
        );
        self.clipboard.set(file.clone(), false);
        self.osc52.push(file.clone());
        let note = if include_secrets {
            "Sent to the terminal clipboard (OSC 52); some terminals block it. It CONTAINS SECRET VALUES."
        } else {
            "Copied to the terminal clipboard (OSC 52); some terminals block it. Secret variables stay \u{7b}\u{7b}placeholders\u{7d}\u{7d}; :httpfile! includes their values."
        };
        self.overlay = Some(Overlay::Report(Report {
            title: "Copy as .http".into(),
            viewer: Viewer::new(Doc::text(&format!(
                "{file}\n\n── notes (not part of the file) ──\n{note}\n"
            ))),
            copy_text: file,
            copied: true,
            close_status: Some("the .http file was sent to the terminal clipboard".into()),
        }));
        self.set_status(".http file sent to the terminal clipboard", StatusKind::Ok);
    }

    fn format_body(&mut self) {
        let Some(t) = self.tab_mut() else { return };
        let ed = if matches!(t.ed.kind, Kind::Grpc | Kind::Mcp) {
            &mut t.ed.message
        } else {
            &mut t.ed.body_text
        };
        match serde_json::from_str::<serde_json::Value>(&ed.text()) {
            Ok(_) => {
                let (pretty, _) = crate::viewer::pretty_json(ed.text().as_bytes());
                ed.replace_all(&pretty);
                self.changed();
            }
            Err(e) => self.set_status(format!("not valid JSON: {e}"), StatusKind::Warn),
        }
    }

    fn copy_body(&mut self) {
        let Some(t) = self.tab() else { return };
        let Some(r) = &t.response else { return };
        let v = match t.resp_tab {
            0 => &r.body,
            1 => &r.headers,
            2 => &r.timing,
            _ => &r.messages,
        };
        let text = match v.mode {
            ViewMode::Pretty if v.doc.pretty.is_some() => v
                .doc
                .pretty
                .as_ref()
                .map(|p| p.text.clone())
                .unwrap_or_default(),
            _ => v.doc.raw.text.clone(),
        };
        if text.len() > 1_000_000 {
            self.set_status(
                format!(
                    "body is {} — too large for the terminal clipboard; save it with s",
                    human_size(text.len() as u64)
                ),
                StatusKind::Warn,
            );
            return;
        }
        let n = text.len();
        self.clipboard.set(text.clone(), false);
        self.osc52.push(text);
        self.set_status(
            format!("copied {} to the clipboard", human_size(n as u64)),
            StatusKind::Ok,
        );
    }

    fn prompt_save_body(&mut self) {
        if self.tab().and_then(|t| t.response.as_ref()).is_none() {
            self.set_status("no response yet", StatusKind::Info);
            return;
        }
        let suggestion = self
            .tab()
            .map(|t| format!("{}.response", t.name.replace(' ', "_")))
            .unwrap_or_default();
        let mut input = TextEditor::single(&suggestion);
        input.move_to_end();
        self.overlay = Some(Overlay::Prompt(Prompt {
            title: "Save response body".into(),
            label: "File".into(),
            input,
            choices: vec![],
            focus: 0,
            action: PromptAction::SaveBody,
            error: None,
            note: None,
            open: None,
        }));
    }

    pub fn save_body(&mut self, path: &str) -> Result<String, String> {
        let t = self.tab().ok_or("no tab")?;
        let r = t.response.as_ref().ok_or("no response")?;
        let p =
            sendito_store::env::expand(&std::env::current_dir().unwrap_or_default(), path.trim());
        std::fs::write(&p, r.body.doc.bytes()).map_err(|e| format!("{}: {e}", p.display()))?;
        Ok(format!(
            "saved {} to {}",
            human_size(r.body.doc.len() as u64),
            p.display()
        ))
    }

    // ----- palette commands ---------------------------------------------

    /// `:cookies [clear | delete <number>]`. The jar is guarded by a
    /// cross-process lock, so the work runs off the UI thread and the result
    /// arrives as an [`AppMsg::Cookies`] message.
    fn cookies_command(&mut self, arg: &str) {
        enum Op {
            List,
            Clear,
            Delete(usize),
        }
        let op = if arg == "clear" {
            Ok(Op::Clear)
        } else if let Some(number) = arg.strip_prefix("delete ") {
            number
                .trim()
                .parse::<usize>()
                .map(Op::Delete)
                .map_err(|_| ())
        } else if arg.is_empty() {
            Ok(Op::List)
        } else {
            Err(())
        };
        let Ok(op) = op else {
            self.set_status(
                "usage: :cookies [clear | delete <number>]",
                StatusKind::Error,
            );
            return;
        };
        if self.cookies_job.is_some() {
            self.set_status("a cookie command is already running", StatusKind::Warn);
            return;
        }
        let root = self.project.root.clone();
        let env = self
            .active_env
            .clone()
            .unwrap_or_else(|| COLLECTION_ENV.into());
        self.cookies_job_seq = self.cookies_job_seq.wrapping_add(1);
        let job = self.cookies_job_seq;
        let tx = self.tx.clone();
        let task = self.rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let r = match op {
                    Op::List => {
                        sendito_store::cookies::describe(&root, &env).map(CookiesOutcome::List)
                    }
                    Op::Clear => sendito_store::cookies::clear(&root, &env).map(|()| {
                        CookiesOutcome::Done("cookies cleared for the active environment")
                    }),
                    Op::Delete(n) => sendito_store::cookies::delete(&root, &env, n)
                        .map(|()| CookiesOutcome::Done("cookie deleted")),
                };
                r.map_err(|e| e.to_string())
            })
            .await
            .map_err(|_| "cookie jar task failed".to_string())
            .and_then(|r| r);
            let _ = tx.send(AppMsg::Cookies { job, result });
        });
        self.cookies_job = Some((job, task));
        self.set_status("working on the cookie jar…", StatusKind::Info);
    }

    fn http_oauth_command(&mut self, arg: &str) {
        let Some((action, name)) = arg.split_once(char::is_whitespace) else {
            self.set_status("usage: :auth login|logout <identity>", StatusKind::Error);
            return;
        };
        let name = name.trim();
        if name.is_empty() || !matches!(action, "login" | "logout") {
            self.set_status("usage: :auth login|logout <identity>", StatusKind::Error);
            return;
        }
        if self.oauth_login.is_some() {
            self.set_status("an OAuth operation is already running", StatusKind::Warn);
            return;
        }
        let identity = self.project.config.oauth.get(name).cloned();
        if action == "login" && identity.is_none() {
            self.set_status(
                "OAuth identity is not defined in [oauth]",
                StatusKind::Error,
            );
            return;
        }
        let account =
            runner::oauth_token_account(&self.project.name, self.active_env.as_deref(), name);
        let keychain = self.project.envs.keychain.clone();
        let envs = self.project.envs.clone();
        let env = self.active_env.clone();
        let runtime = self.runtime.clone();
        let login = action == "login";
        self.oauth_login_seq = self.oauth_login_seq.wrapping_add(1);
        let job = self.oauth_login_seq;
        let tx = self.tx.clone();
        let task = self.rt.spawn(async move {
            let result: Result<String, String> = async {
                if !login {
                    tokio::task::spawn_blocking(move || keychain.delete(&account))
                        .await
                        .map_err(|_| "keychain task failed".to_string())?
                        .map_err(|_| "could not delete OAuth tokens".to_string())?;
                    return Ok("HTTP OAuth tokens removed".into());
                }
                let (scope, errors) = tokio::task::spawn_blocking(move || {
                    runner::build_scope(&envs, env.as_deref(), &runtime)
                })
                .await
                .map_err(|_| "environment resolution failed".to_string())?;
                if !errors.is_empty() {
                    return Err("cannot resolve environment secrets".into());
                }
                let identity = identity.ok_or("OAuth identity is not defined")?;
                let settings = runner::oauth_settings(&identity, &scope)?;
                let opener = |url: &str| {
                    let program = if cfg!(target_os = "macos") {
                        "open"
                    } else {
                        "xdg-open"
                    };
                    std::process::Command::new(program)
                        .arg(url)
                        .stdin(std::process::Stdio::null())
                        .stdout(std::process::Stdio::null())
                        .stderr(std::process::Stdio::null())
                        .spawn()
                        .is_ok()
                };
                let note = |text: String| {
                    if let Some(url) = text.strip_prefix("authorization URL:\n  ") {
                        let _ = tx.send(AppMsg::OAuthUrl {
                            job,
                            url: url.into(),
                        });
                    }
                };
                let tokens = sendito_mcp::oauth::login_http(
                    &settings,
                    Duration::from_secs(300),
                    &opener,
                    &note,
                )
                .await
                .map_err(|_| {
                    "OAuth login failed; check the endpoints and authorization response".to_string()
                })?;
                tokio::task::spawn_blocking(move || keychain.set(&account, &tokens.to_json()))
                    .await
                    .map_err(|_| "keychain task failed".to_string())?
                    .map_err(|_| "could not store OAuth tokens".to_string())?;
                Ok("HTTP OAuth login complete".into())
            }
            .await;
            let _ = tx.send(AppMsg::OAuthDone { job, result });
        });
        self.oauth_login = Some((job, task));
        self.set_status(
            if login {
                "starting HTTP OAuth login"
            } else {
                "removing OAuth tokens"
            },
            StatusKind::Info,
        );
    }

    pub fn run_command(&mut self, cmd: &str) {
        let cmd = cmd.trim().trim_start_matches(':');
        let (word, arg) = cmd
            .split_once(char::is_whitespace)
            .map(|(w, a)| (w, a.trim()))
            .unwrap_or((cmd, ""));
        match word {
            "send" => self.send(false),
            "cancel" => self.cancel(),
            "auth" => self.http_oauth_command(arg),
            "watch" => {
                let secs = if arg.is_empty() {
                    Ok(None)
                } else {
                    arg.parse::<u64>().map(Some)
                };
                match secs {
                    Ok(s) => self.toggle_watch(s),
                    Err(_) => {
                        self.set_status("usage: :watch [seconds]".to_string(), StatusKind::Error)
                    }
                }
            }
            "end" => self.end_stream(),
            "env" => {
                let list = self.project.envs.list();
                let prefixed: Vec<&String> = list.iter().filter(|e| e.starts_with(arg)).collect();
                if arg.is_empty() {
                    self.set_env(None);
                } else if list.iter().any(|e| e == arg) {
                    self.set_env(Some(arg.to_string()));
                } else if let [one] = prefixed.as_slice() {
                    self.set_env(Some((*one).clone()));
                } else {
                    self.set_status(
                        format!(
                            "no environment named \"{arg}\" (available: {})",
                            self.project.envs.list().join(", ")
                        ),
                        StatusKind::Error,
                    );
                }
            }
            "envs" => self.open_env_editor(),
            "theme" => self.theme_command(arg),
            "w" | "write" | "save" => self.save(),
            "saveas" => {
                if let Some(t) = self.tab() {
                    let id = t.id;
                    self.prompt_save_as(id, arg);
                }
            }
            "wq" | "x" => {
                self.save();
                self.request_quit();
            }
            "q" | "quit" => self.request_quit(),
            "q!" | "quit!" => self.quit = true,
            "new" => self.new_tab(Request::new_http()),
            "newgrpc" => self.new_tab(Request::new_grpc()),
            "newmcp" => self.new_tab(Request::new_mcp()),
            "mcpservers" => self.open_mcp_server_picker(),
            "mcptools" => self.load_mcp_tools(),
            "import" => {
                if arg.is_empty() {
                    self.open_import("");
                } else if let Err(e) = self.do_import(arg) {
                    self.set_status(e, StatusKind::Error);
                }
            }
            "curl" => self.export_curl(false),
            "curl!" => self.export_curl(true),
            "httpfile" => self.export_http_file(false),
            "httpfile!" => self.export_http_file(true),
            "history" => self.toggle_history(),
            "cookies" => self.cookies_command(arg),
            "clearhistory" => {
                self.overlay = Some(Overlay::Confirm(Confirm {
                    title: "Clear history".into(),
                    message: "Delete every history entry of this collection?".into(),
                    buttons: vec!["Delete".into(), "Cancel".into()],
                    sel: 1,
                    action: ConfirmAction::ClearHistory,
                }))
            }
            "format" => self.format_body(),
            "template" => self.load_schema(false, true, false),
            "reload" => self.load_schema(false, false, true),
            "settings" => self.overlay = Some(Overlay::Settings { sel: 0 }),
            "help" | "h" => self.open_help(),
            "report" => match self.last_report.clone() {
                Some((title, text)) => self.show_report(&title, &text),
                None => self.set_status("no report yet in this session", StatusKind::Info),
            },
            "savebody" => {
                if arg.is_empty() {
                    self.prompt_save_body();
                } else {
                    match self.save_body(arg) {
                        Ok(m) => self.set_status(m, StatusKind::Ok),
                        Err(e) => self.set_status(e, StatusKind::Error),
                    }
                }
            }
            "" => {}
            other => self.set_status(
                format!("unknown command :{other} (Ctrl+P lists commands)"),
                StatusKind::Error,
            ),
        }
    }

    // ----- settings -----------------------------------------------------

    pub fn setting_value(&self, id: SettingId) -> String {
        let ui = &self.project.config.ui;
        match id {
            SettingId::Vim => {
                if self.vim {
                    "On".into()
                } else {
                    "Off".into()
                }
            }
            SettingId::Theme => ui.theme.clone(),
            SettingId::Icons => ui.icons.clone(),
            SettingId::Sidebar => ui.sidebar.clone(),
            SettingId::Mouse => {
                if ui.mouse {
                    "On".into()
                } else {
                    "Off".into()
                }
            }
            SettingId::ConfirmQuit => {
                if ui.confirm_quit {
                    "On".into()
                } else {
                    "Off".into()
                }
            }
            SettingId::History => {
                if self.project.config.history.enabled {
                    "On".into()
                } else {
                    "Off".into()
                }
            }
            SettingId::WatchInterval => format!("{} s", self.project.config.ui.watch_interval_s),
        }
    }

    /// Switches to theme `name` (not saved to the config).
    pub fn apply_theme(&mut self, name: &str) {
        self.project.config.ui.theme = name.to_string();
        if !no_color() {
            self.theme = self.themes.theme(name);
        }
    }

    /// Reads the theme files again (`:theme reload`).
    pub fn reload_themes(&mut self) {
        let (files, warnings) = sendito_store::themes::discover(&self.project.theme_dirs);
        self.themes = crate::theme::Registry::from_files(files, warnings);
        let name = self.project.config.ui.theme.clone();
        self.apply_theme(&name);
        let mut problems: Vec<String> = self.themes.warnings.clone();
        problems.extend(self.themes.notes(&name).iter().cloned());
        let n = self.themes.names().len() - crate::theme::BUILTIN.len();
        if problems.is_empty() {
            self.set_status(
                format!("themes reloaded: {n} of your own, using {name}"),
                StatusKind::Ok,
            );
        } else {
            self.set_status(
                format!("themes reloaded: {}", problems.join("; ")),
                StatusKind::Warn,
            );
        }
    }

    /// `:theme [name|reload]`.
    fn theme_command(&mut self, arg: &str) {
        let names = self.themes.names();
        if arg.is_empty() {
            self.set_status(
                format!(
                    "theme: {} (available: {})",
                    self.project.config.ui.theme,
                    names.join(", ")
                ),
                StatusKind::Info,
            );
        } else if arg == "reload" {
            self.reload_themes();
        } else if let Some(n) = names.iter().find(|n| n.eq_ignore_ascii_case(arg)).cloned() {
            self.apply_theme(&n);
            if let Some(p) = self.project.global_config.clone()
                && let Err(e) = sendito_store::config::set_value(
                    &p,
                    "ui",
                    "theme",
                    toml::Value::String(n.clone()),
                )
            {
                self.set_status(format!("could not write settings: {e}"), StatusKind::Error);
                return;
            }
            let notes = self.themes.notes(&n);
            if notes.is_empty() {
                self.set_status(format!("theme: {n}"), StatusKind::Ok);
            } else {
                self.set_status(format!("theme {n}: {}", notes.join("; ")), StatusKind::Warn);
            }
        } else {
            self.set_status(
                format!("no theme named \"{arg}\" (available: {})", names.join(", ")),
                StatusKind::Error,
            );
        }
    }

    pub fn change_setting(&mut self, id: SettingId, dir: isize) {
        let (section, key, value) = match id {
            SettingId::Vim => {
                self.vim = !self.vim;
                self.project.config.ui.vim_mode = self.vim;
                let vim = self.vim;
                for t in &mut self.tabs {
                    t.ed.set_vim(vim);
                }
                ("ui", "vim_mode", toml::Value::Boolean(self.vim))
            }
            SettingId::Theme => {
                let names = self.themes.names();
                let i = names
                    .iter()
                    .position(|n| *n == self.project.config.ui.theme)
                    .unwrap_or(0) as isize;
                let next = names[((i + dir).rem_euclid(names.len() as isize)) as usize].clone();
                self.apply_theme(&next);
                ("ui", "theme", toml::Value::String(next))
            }
            SettingId::Icons => {
                let v = if self.icons.nerd { "unicode" } else { "nerd" };
                self.project.config.ui.icons = v.into();
                self.icons = crate::icons::Icons::by_name(v);
                ("ui", "icons", toml::Value::String(v.into()))
            }
            SettingId::Sidebar => {
                let v = if self.project.config.ui.sidebar == "right" {
                    "left"
                } else {
                    "right"
                };
                self.project.config.ui.sidebar = v.into();
                ("ui", "sidebar", toml::Value::String(v.into()))
            }
            SettingId::Mouse => {
                self.project.config.ui.mouse = !self.project.config.ui.mouse;
                (
                    "ui",
                    "mouse",
                    toml::Value::Boolean(self.project.config.ui.mouse),
                )
            }
            SettingId::ConfirmQuit => {
                self.project.config.ui.confirm_quit = !self.project.config.ui.confirm_quit;
                (
                    "ui",
                    "confirm_quit",
                    toml::Value::Boolean(self.project.config.ui.confirm_quit),
                )
            }
            SettingId::History => {
                self.project.config.history.enabled = !self.project.config.history.enabled;
                (
                    "history",
                    "enabled",
                    toml::Value::Boolean(self.project.config.history.enabled),
                )
            }
            SettingId::WatchInterval => {
                const STEPS: [u64; 4] = [5, 10, 30, 60];
                let i = STEPS
                    .iter()
                    .position(|s| *s == self.project.config.ui.watch_interval_s)
                    .unwrap_or(0) as isize;
                let next = STEPS[((i + dir).rem_euclid(STEPS.len() as isize)) as usize];
                self.project.config.ui.watch_interval_s = next;
                ("ui", "watch_interval_s", toml::Value::Integer(next as i64))
            }
        };
        if let Some(p) = self.project.global_config.clone()
            && let Err(e) = sendito_store::config::set_value(&p, section, key, value)
        {
            self.set_status(format!("could not write settings: {e}"), StatusKind::Error);
        }
    }

    // ----- hints --------------------------------------------------------

    /// Context-sensitive hints for the hint row: (keys, label).
    /// The send keys for hints: the first two bindings ("Ctrl+Enter/F5"), as
    /// Ctrl+Enter only works in terminals that report it.
    pub fn send_hint(&self) -> String {
        let keys = self.keymap.keys_for(Action::Send, self.vim);
        match keys.as_slice() {
            [] => "(unbound)".into(),
            [one] => one.clone(),
            [a, b, ..] => format!("{a}/{b}"),
        }
    }

    /// The verb for running a request: "send" for HTTP and gRPC alike,
    /// matching the Send button.
    pub fn send_verb(&self) -> &'static str {
        "send"
    }

    pub fn hints(&mut self) -> Vec<(String, String)> {
        let k = |s: &Self, a: Action| s.keymap.hint(a, s.vim);
        let mut v: Vec<(String, String)> = Vec::new();
        if let Some(ov) = &self.overlay {
            match ov {
                Overlay::Palette(_) => v.extend([
                    ("↑↓".into(), "move".into()),
                    ("Enter".into(), "run".into()),
                    (":".into(), "commands".into()),
                    ("Esc".into(), "close".into()),
                ]),
                Overlay::Picker(_) => v.extend([
                    ("type".into(), "filter".into()),
                    ("↑↓".into(), "move".into()),
                    ("Enter".into(), "choose".into()),
                    ("Esc".into(), "close".into()),
                ]),
                Overlay::Prompt(p) if p.open.is_some() => v.extend([
                    ("↑↓".into(), "move".into()),
                    ("Enter".into(), "choose".into()),
                    ("Esc".into(), "close list".into()),
                ]),
                Overlay::Prompt(p) if p.focus > 0 => v.extend([
                    ("Enter".into(), prompt_verb(&p.action).into()),
                    ("←→".into(), "change".into()),
                    ("Space".into(), "list".into()),
                    ("Tab".into(), "next field".into()),
                    ("Esc".into(), "cancel".into()),
                ]),
                Overlay::Prompt(p) => v.extend([
                    ("Enter".into(), prompt_verb(&p.action).into()),
                    ("Tab".into(), "next field".into()),
                    ("Esc".into(), "cancel".into()),
                ]),
                // The dialog's own keys, then Enter on the focused button.
                Overlay::Confirm(c) => {
                    let b = |i: usize| c.buttons.get(i).cloned().unwrap_or_default().to_lowercase();
                    if c.buttons.len() == 3 {
                        v.extend([("s".into(), b(0)), ("d".into(), b(1))]);
                    } else {
                        v.push(("y".into(), b(0)));
                    }
                    v.extend([
                        (
                            "Enter".into(),
                            c.buttons
                                .get(c.sel)
                                .cloned()
                                .unwrap_or_default()
                                .to_lowercase(),
                        ),
                        ("←→".into(), "choose".into()),
                        ("Esc".into(), "cancel".into()),
                    ]);
                }
                // Save, New env and Close are on the dialog's buttons.
                Overlay::Env(e) => v.extend([
                    ("Ctrl+S".into(), "save".into()),
                    ("Ctrl+D".into(), "delete row".into()),
                    ("Space".into(), "source".into()),
                    ("Ctrl+PgUp/Dn".into(), "env".into()),
                    if e.table.sel_col == 3 {
                        ("type".into(), "set secret".into())
                    } else {
                        ("Ctrl+N".into(), "new env".into())
                    },
                    (
                        "Esc".into(),
                        if e.dirty {
                            "close (asks to save)"
                        } else {
                            "close"
                        }
                        .into(),
                    ),
                ]),
                Overlay::Import(_) => v.extend([
                    ("F5 / Ctrl+S".into(), "import".into()),
                    ("Esc".into(), "cancel".into()),
                ]),
                Overlay::Report(r) => v.extend([
                    (
                        "y".into(),
                        if r.copied { "copy again" } else { "copy" }.into(),
                    ),
                    ("↑↓".into(), "scroll".into()),
                    ("Esc".into(), "close".into()),
                    (":report".into(), "shows this report again".into()),
                ]),
                Overlay::Help { .. } => v.extend([
                    ("type".into(), "search".into()),
                    ("↑↓ PgUp PgDn".into(), "scroll".into()),
                    ("Esc".into(), "close".into()),
                ]),
                Overlay::Settings { .. } => v.extend([
                    ("↑↓".into(), "move".into()),
                    ("←→ Space".into(), "change".into()),
                    ("Esc".into(), "close".into()),
                ]),
            }
            return v;
        }
        // While a tree item is dragged, say what releasing it does.
        if let Some(Drag::Tree {
            rel, active: true, ..
        }) = &self.drag
        {
            let name = sendito_store::collection::name_of(rel);
            let what = match self.drop_target.as_deref() {
                Some("") => format!("to move {name} into / (collection root)"),
                Some(t) => format!("to move {name} into {t}/"),
                None => "outside a folder: nothing moves".to_string(),
            };
            v.push(("release".into(), what));
            return v;
        }
        if !self.pending.is_empty() {
            let p = self
                .pending
                .iter()
                .map(|c| c.to_string())
                .collect::<Vec<_>>()
                .join(" ");
            v.push((
                format!("{p} …"),
                "h/j/k/l focus · n/p tab · e send · x cancel · s save · c close · q quit · ? help"
                    .into(),
            ));
            return v;
        }
        let running = self.tab().is_some_and(|t| t.running.is_some());
        let streaming = self
            .tab()
            .and_then(|t| t.running.as_ref())
            .is_some_and(|r| r.streaming_input);
        // Keys of a running call come first, whichever pane has focus.
        if streaming {
            v.extend([
                (self.send_hint(), "send message".into()),
                (k(self, Action::EndStream), "end stream".into()),
                (k(self, Action::Cancel), "cancel".into()),
            ]);
        } else if running {
            v.push((k(self, Action::Cancel), "cancel".into()));
        }
        match self.focus {
            Focus::Sidebar if self.sidebar_mode == SidebarMode::History => {
                v.extend([
                    ("Enter".into(), "open".into()),
                    (self.send_hint(), "resend entry".into()),
                    ("s".into(), "save to tree".into()),
                    (k(self, Action::History), "back to tree".into()),
                ]);
            }
            Focus::Sidebar => {
                // Most used and destructive keys first: narrow screens drop the tail.
                if self.vim {
                    v.push(("j/k".into(), "up/down".into()));
                    v.push(("h/l".into(), "collapse/expand".into()));
                }
                v.extend([
                    ("Enter".into(), "open / fold".into()),
                    ("n".into(), "new request".into()),
                    ("Del/x".into(), "delete".into()),
                    ("/".into(), "filter".into()),
                    ("r".into(), "rename".into()),
                    ("N".into(), "new folder".into()),
                    ("m / drag".into(), "move".into()),
                    ("d".into(), "duplicate".into()),
                ]);
            }
            Focus::Filter => v.extend([
                ("type".into(), "filter".into()),
                ("Enter/↓".into(), "to results".into()),
                ("Esc".into(), "clear".into()),
            ]),
            Focus::Bar(_) | Focus::ReqTabs | Focus::ReqBody => {
                let no_method = self.tab().is_some_and(|t| {
                    matches!(&t.model, Request::Grpc(g) if g.service.is_empty() || g.method.is_empty())
                });
                if no_method {
                    v.push(("method ▾".into(), "pick one to enable Send".into()));
                } else if !running {
                    v.push((self.send_hint(), self.send_verb().into()));
                }
                // Save comes right after send so narrow terminals keep it.
                v.push((k(self, Action::Save), "save".into()));
                // In vim insert or visual mode, how to get back out.
                let mode = self.focused_text().map(|e| e.vim);
                if self.vim
                    && matches!(
                        mode,
                        Some(
                            crate::editor::VimMode::Insert
                                | crate::editor::VimMode::Visual
                                | crate::editor::VimMode::VisualLine
                        )
                    )
                {
                    v.push(("Esc".into(), "normal mode".into()));
                } else if self.vim && mode == Some(crate::editor::VimMode::Normal) {
                    v.push(("i".into(), "insert".into()));
                }
                let field = self.tab().and_then(|t| t.current_field());
                match (self.focus, field) {
                    (Focus::ReqTabs, _) => v.extend([
                        ("Enter/↓".into(), "edit".into()),
                        ("←→".into(), "switch tab".into()),
                    ]),
                    (Focus::ReqBody, Some(Field::McpServer)) => {
                        v.push(("F2".into(), "choose server".into()))
                    }
                    (Focus::ReqBody, Some(Field::McpTool)) => {
                        v.push(("F2".into(), "choose tool".into()))
                    }
                    (Focus::Bar(0), _) if self.tab().is_some_and(|t| t.ed.kind == Kind::Mcp) => {
                        v.push(("F2".into(), "choose server".into()))
                    }
                    (Focus::ReqBody, Some(f)) if f.is_table() => {
                        // A choice column (e.g. a multipart part's kind).
                        let choice = self
                            .tab_mut()
                            .and_then(|t| t.ed.table_mut(f))
                            .and_then(|t| t.columns.get(t.sel_col).map(|c| (c.title, c.kind)))
                            .and_then(|(title, kind)| {
                                matches!(kind, crate::form::ColKind::Choice(_)).then_some(title)
                            });
                        if let Some(title) = choice {
                            v.push(("Space".into(), format!("change {}", title.to_lowercase())));
                        }
                        v.extend([
                            ("Enter".into(), "next cell".into()),
                            (k(self, Action::DeleteRow), "delete row".into()),
                            (k(self, Action::ToggleRow), "enable/disable".into()),
                        ]);
                    }
                    (Focus::ReqBody, Some(f)) if f.is_choice() => {
                        v.push(("Enter/Space".into(), "choose".into()))
                    }
                    (Focus::ReqBody, Some(f)) if f.is_toggle() => {
                        v.push(("Space".into(), "toggle".into()))
                    }
                    (Focus::ReqBody, Some(f)) if f.is_multiline() => {
                        // The way out of the editor comes right after Send, so
                        // it survives on narrow screens.
                        let at = v
                            .iter()
                            .position(|(_, l)| l == "send" || l == "send message")
                            .map_or(v.len(), |i| i + 1);
                        v.insert(at, ("Esc".into(), "leave editor".into()));
                        if !self.vim {
                            v.push((k(self, Action::SelectAll), "select all".into()));
                        }
                        v.push((k(self, Action::FormatBody), "format JSON".into()));
                    }
                    (Focus::Bar(0), _) if self.tab().is_some_and(|t| t.ed.kind == Kind::Http) => {
                        v.push(("Enter".into(), "choose method".into()))
                    }
                    (Focus::Bar(1), _) if self.tab().is_some_and(|t| t.ed.kind == Kind::Grpc) => {
                        v.push(("Enter".into(), "choose method".into()))
                    }
                    _ => {}
                }
            }
            Focus::RespTabs | Focus::RespBody => {
                v.push(("Alt+1-4".into(), "tabs".into()));
                // View modes, folding and saving only apply to the body.
                if self.tab().is_some_and(|t| t.resp_tab == 0) {
                    v.extend([
                        ("y".into(), "copy".into()),
                        ("p/r/x".into(), "view".into()),
                        ("/".into(), "search".into()),
                        ("s".into(), "save body".into()),
                        ("Enter".into(), "fold".into()),
                        ("w".into(), "wrap".into()),
                    ]);
                } else {
                    v.push(("y".into(), "copy".into()));
                    // Timing is a short table: nothing to search or wrap.
                    if self.tab().is_some_and(|t| t.resp_tab != 2) {
                        v.push(("/".into(), "search".into()));
                        v.push(("w".into(), "wrap".into()));
                    }
                }
            }
        }
        v.push((k(self, Action::Palette), "commands".into()));
        v.push((k(self, Action::Help), "all keys".into()));
        v
    }
}

/// What confirming a prompt does, for its button and the hint row.
pub fn prompt_verb(a: &PromptAction) -> &'static str {
    match a {
        PromptAction::SaveAs { .. } | PromptAction::SaveHistory(_) | PromptAction::SaveBody => {
            "save"
        }
        PromptAction::NewFolder { .. } | PromptAction::NewRequest { .. } | PromptAction::NewEnv => {
            "create"
        }
        PromptAction::Rename(_) => "rename",
        PromptAction::Search => "search",
        PromptAction::CustomMethod => "use",
    }
}

/// A short, plain reason for a transport error (the full text follows it).
/// A few words for the status line.
fn short_error(e: &str) -> &'static str {
    let l = e.to_lowercase();
    if l.contains("connection refused") || l.contains("os error 111") || l.contains("os error 61") {
        "connection refused"
    } else if l.contains("dns error")
        || l.contains("failed to lookup")
        || l.contains("name or service not known")
    {
        "host name not found (DNS)"
    } else if l.contains("timed out") || l.contains("timeout") {
        "timed out"
    } else if l.contains("certificate") || l.contains("tls") || l.contains("handshake") {
        "TLS failure"
    } else if l.contains("connection reset") || l.contains("broken pipe") {
        "connection closed by the server"
    } else if l.contains("no such file") || l.contains("os error 2") {
        "file not found"
    } else {
        "request failed"
    }
}

fn explain_error(e: &str) -> String {
    let l = e.to_lowercase();
    let host = l
        .split("for url (")
        .nth(1)
        .and_then(|r| r.split(')').next())
        .unwrap_or("");
    let target = if host.is_empty() {
        "the server".to_string()
    } else {
        host.to_string()
    };
    if l.contains("connection refused") || l.contains("os error 111") || l.contains("os error 61") {
        format!("nothing is listening at {target} (connection refused).")
    } else if l.contains("dns error")
        || l.contains("failed to lookup")
        || l.contains("name or service not known")
    {
        "the host name could not be resolved (DNS).".into()
    } else if l.contains("timed out") || l.contains("timeout") {
        "the server did not answer in time (timeout).".into()
    } else if l.contains("certificate") || l.contains("tls") || l.contains("handshake") {
        "the TLS connection failed (certificate or handshake); Options can turn off TLS verification.".into()
    } else if l.contains("connection reset") || l.contains("broken pipe") {
        "the server closed the connection.".into()
    } else if l.contains("no such file") || l.contains("os error 2") {
        "a file the request sends was not found.".into()
    } else {
        "see the details below.".into()
    }
}

/// "1 assertion", "3 assertions".
fn assertions(n: usize) -> String {
    if n == 1 {
        "1 assertion".into()
    } else {
        format!("{n} assertions")
    }
}

/// `path` with the home directory written as `~`.
pub fn display_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

pub fn human_size(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

fn ms(d: Option<Duration>) -> String {
    d.map(|d| format!("{:.1} ms", d.as_secs_f64() * 1000.0))
        .unwrap_or_else(|| "—".into())
}

/// Width of the timing waterfall, in cells.
const TRACK: usize = 32;

/// One phase on the waterfall: dots for the timeline, blocks from `start`
/// for `len`, with eighth-of-a-cell precision at the end.
fn waterfall(start: Duration, len: Duration, total: Duration) -> String {
    let t = total.as_secs_f64();
    if t <= 0.0 {
        return "·".repeat(TRACK);
    }
    let scale = (TRACK * 8) as f64 / t;
    let s8 = ((start.as_secs_f64() * scale).round() as usize).min(TRACK * 8 - 1);
    let mut e8 = (((start + len).as_secs_f64() * scale).round() as usize).min(TRACK * 8);
    if !len.is_zero() && e8 <= s8 {
        e8 = s8 + 1;
    }
    const LEFT: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    (0..TRACK)
        .map(|cell| {
            let (c0, c1) = (cell * 8, cell * 8 + 8);
            let (from, to) = (s8.max(c0), e8.min(c1));
            if to <= from {
                '·'
            } else if from == c0 && to == c1 {
                '█'
            } else if from == c0 {
                LEFT[to - c0 - 1]
            } else if to == c1 {
                // Starts inside the cell: right-aligned blocks only come in halves.
                if c1 - from >= 6 {
                    '█'
                } else if c1 - from >= 3 {
                    '▐'
                } else {
                    '▕'
                }
            } else {
                '▎'
            }
        })
        .collect()
}

pub fn timing_text(r: &sendito_http::HttpResponse, body_size: u64) -> String {
    let t = r.timing;
    let total = t.total;
    // A phase that did not happen says why, on its own row. Phases follow
    // each other, so each starts where the previous one ended.
    let rows = [
        ("DNS lookup", t.dns, "skipped (IP address)"),
        ("TCP connect", t.connect, "skipped (connection reused)"),
        ("TLS handshake", t.tls, "skipped (not HTTPS)"),
        ("Waiting for first byte", Some(t.ttfb), ""),
        ("Download", Some(t.download), ""),
    ];
    let total_label = ms(Some(total));
    let scale = format!(
        "0{}{}",
        " ".repeat(TRACK.saturating_sub(1 + total_label.trim().len())),
        total_label.trim()
    );
    let mut s = format!("{:<22} {:>9}  {scale}\n", "Phase", "Time");
    let mut at = Duration::ZERO;
    for (label, d, why) in rows {
        match d {
            Some(d) => {
                s.push_str(&format!(
                    "{label:<22} {:>9}  {}\n",
                    ms(Some(d)),
                    waterfall(at, d, total)
                ));
                at += d;
            }
            None => s.push_str(&format!("{label:<22} {:>9}  {why}\n", "—")),
        }
    }
    s.push_str(&format!("{:<22} {:>9}\n\n", "Total", total_label));
    s.push_str(&format!("Protocol        {}\n", r.version));
    if let Some(a) = r.remote_addr {
        s.push_str(&format!("Remote address  {a}\n"));
    }
    s.push_str(&format!("Final URL       {}\n", r.url));
    s.push_str(&format!(
        "Body size       {} ({} bytes)\n",
        human_size(body_size),
        body_size
    ));

    s
}

/// The watch section of the Timing tab: one mark per send while watching
/// (✗ failed, blocks show duration, tallest = slowest).
fn watch_text(samples: &[(bool, f64)]) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = samples.iter().map(|(_, ms)| *ms).fold(0.0_f64, f64::max);
    let marks: String = samples
        .iter()
        .map(|&(ok, ms)| {
            if !ok {
                '✗'
            } else if max <= 0.0 {
                BLOCKS[0]
            } else {
                BLOCKS[(((ms / max) * 7.0).clamp(0.0, 7.0)) as usize]
            }
        })
        .collect();
    let fails = samples.iter().filter(|(ok, _)| !ok).count();
    let last = samples
        .last()
        .map(|&(ok, ms)| {
            if ok {
                format!("{:.0} ms", ms)
            } else {
                "failed".into()
            }
        })
        .unwrap_or_else(|| "-".into());
    format!(
        "\nWatch\n\n  {marks}\n\n  {} sends · {fails} failed · last {last}\n  ✗ failed send (error or status 400+) · blocks show duration\n",
        samples.len()
    )
}

fn request_headers_text(o: &HttpOutcome) -> String {
    let mut s = format!(
        "Request sent\n{} {}\n",
        o.resolved.method,
        mask_url(&o.resolved.url)
    );
    for (n, v) in &o.resolved.headers {
        s.push_str(&format!("{n}: {}\n", mask_header(n, v)));
    }
    s
}

fn mask_url(u: &str) -> String {
    u.to_string()
}

/// Authorization values are shown masked in the UI.
fn mask_header(name: &str, value: &str) -> String {
    let n = name.to_ascii_lowercase();
    if n == "authorization"
        || n == "proxy-authorization"
        || n.contains("api-key")
        || n.contains("token")
        || n == "cookie"
    {
        let scheme = value
            .split_whitespace()
            .next()
            .filter(|_| value.contains(' '))
            .unwrap_or("");
        if scheme.is_empty() {
            crate::MASK.to_string()
        } else {
            format!("{scheme} {}", crate::MASK)
        }
    } else {
        value.to_string()
    }
}

fn headers_text(r: &sendito_http::HttpResponse, o: &HttpOutcome) -> String {
    let mut s = format!("{} {} {}\n", r.version, r.status, r.reason);
    for (n, v) in &r.headers {
        s.push_str(&format!("{n}: {v}\n"));
    }
    s.push('\n');
    s.push_str(&request_headers_text(o));
    s
}

fn messages_text(sc: &runner::ScriptResults, scope_errors: &[String], notes: &[String]) -> String {
    let mut s = String::new();
    if !sc.asserts.is_empty() {
        let pass = sc.asserts.iter().filter(|a| a.pass).count();
        s.push_str(&format!("Assertions: {pass}/{} passed\n", sc.asserts.len()));
        for a in &sc.asserts {
            if a.pass {
                s.push_str(&format!("  ✓ PASS  {}\n", a.label));
            } else {
                s.push_str(&format!("  ✗ FAIL  {}   (actual: {})\n", a.label, a.actual));
            }
        }
        s.push('\n');
    }
    if !sc.pre.is_empty() {
        s.push_str("Pre-request variables:\n");
        for (k, v) in &sc.pre {
            s.push_str(&format!(
                "  {k} = {}\n",
                if v.secret {
                    crate::MASK.to_string()
                } else {
                    v.value.clone()
                }
            ));
        }
        s.push('\n');
    }
    if !sc.extracts.is_empty() {
        s.push_str("Extracted variables:\n");
        for e in &sc.extracts {
            match &e.value {
                Ok(v) => s.push_str(&format!(
                    "  {} = {}\n",
                    e.var,
                    if v.secret {
                        crate::MASK.to_string()
                    } else {
                        v.value.clone()
                    }
                )),
                Err(err) => s.push_str(&format!("  {}: {err}\n", e.var)),
            }
        }
        s.push('\n');
    }
    for e in scope_errors {
        s.push_str(&format!("warning: {e}\n"));
    }
    for n in notes {
        s.push_str(&format!("note: {n}\n"));
    }
    if s.is_empty() {
        s.push_str("No scripts or assertions for this request. Add them in the Scripts tab.\n");
    }
    s
}

fn clock(t: std::time::SystemTime) -> String {
    let dt: time::OffsetDateTime = t.into();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.millisecond()
    )
}

fn mcp_argument_template(schema: &serde_json::Value) -> String {
    fn sample(schema: &serde_json::Value, depth: usize) -> serde_json::Value {
        if let Some(value) = schema.get("default").or_else(|| {
            schema
                .get("enum")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
        }) {
            return value.clone();
        }
        if depth >= 8 {
            return serde_json::Value::Null;
        }
        match schema.get("type").and_then(|v| v.as_str()) {
            Some("object") => {
                let mut out = serde_json::Map::new();
                if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
                    for (name, value) in properties {
                        out.insert(name.clone(), sample(value, depth + 1));
                    }
                }
                serde_json::Value::Object(out)
            }
            Some("array") => serde_json::json!([]),
            Some("string") => serde_json::json!(""),
            Some("integer" | "number") => serde_json::json!(0),
            Some("boolean") => serde_json::json!(false),
            _ => serde_json::Value::Null,
        }
    }
    let value = sample(schema, 0);
    if value.is_object() {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())
    } else {
        "{}".into()
    }
}

/// One MCP event's log line (the Response "Log" tab).
pub fn mcp_event_text(e: &runner::McpEvent) -> String {
    use runner::McpEvent::*;
    match e {
        Sent { json, at, wall } => format!(
            "{} +{:>8.3}s  \u{2192} sent      {json}\n",
            clock(*wall),
            at.as_secs_f64()
        ),
        Received { json, at, wall } => format!(
            "{} +{:>8.3}s  \u{2190} received  {json}\n",
            clock(*wall),
            at.as_secs_f64()
        ),
        Log { line, at } => format!("{:12} +{:>8.3}s  \u{2502} {line}\n", "", at.as_secs_f64()),
        Finished { ok, message, at } => {
            let tail = if *ok {
                String::new()
            } else if message.is_empty() {
                "failed".into()
            } else {
                format!("failed: {message}")
            };
            format!("{:12} +{:>8.3}s  \u{25a0} {tail}\n", "", at.as_secs_f64())
        }
    }
}

/// The JSON-RPC line `on_mcp` keeps on the "Request" tab, if this event sent one.
pub fn mcp_sent_line(e: &runner::McpEvent) -> Option<&str> {
    match e {
        runner::McpEvent::Sent { json, .. } => Some(json),
        _ => None,
    }
}

pub fn event_text(e: &sendito_grpc::Event) -> String {
    use sendito_grpc::Event::*;
    match e {
        Sent { json, at, wall } => format!(
            "{} +{:>8.3}s  → sent      {json}\n",
            clock(*wall),
            at.as_secs_f64()
        ),
        SendClosed { at } => format!(
            "{:12} +{:>8.3}s  → end of request stream\n",
            "",
            at.as_secs_f64()
        ),
        Headers { metadata, at } => {
            let md: Vec<String> = metadata.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!(
                "{:12} +{:>8.3}s  ← headers   {}\n",
                "",
                at.as_secs_f64(),
                md.join(" ")
            )
        }
        Received {
            compact: c,
            size,
            at,
            wall,
            ..
        } => format!(
            "{} +{:>8.3}s  ← received  ({size} B) {c}\n",
            clock(*wall),
            at.as_secs_f64()
        ),
        Finished {
            code, message, at, ..
        } => {
            let name = sendito_grpc::code_name(*code);
            format!(
                "{:12} +{:>8.3}s  ■ status    {name} ({code}){}{message}\n",
                "",
                at.as_secs_f64(),
                if message.is_empty() { "" } else { ": " }
            )
        }
    }
}

fn grpc_headers_text(events: &[sendito_grpc::Event]) -> String {
    let mut s = String::new();
    for e in events {
        match e {
            sendito_grpc::Event::Headers { metadata, .. } => {
                s.push_str("Response headers\n");
                for (k, v) in metadata {
                    s.push_str(&format!("{k}: {v}\n"));
                }
                s.push('\n');
            }
            sendito_grpc::Event::Finished {
                code,
                message,
                metadata,
                ..
            } => {
                s.push_str("Trailers\n");
                s.push_str(&format!(
                    "grpc-status: {code} ({})\n",
                    sendito_grpc::code_name(*code)
                ));
                if !message.is_empty() {
                    s.push_str(&format!("grpc-message: {message}\n"));
                }
                for (k, v) in metadata {
                    s.push_str(&format!("{k}: {v}\n"));
                }
            }
            _ => {}
        }
    }
    s
}

fn grpc_timing_text(events: &[sendito_grpc::Event]) -> String {
    let mut s = String::new();
    let mut first = None;
    let mut count = 0;
    let mut sent = 0;
    for e in events {
        match e {
            sendito_grpc::Event::Headers { at, .. } => s.push_str(&format!(
                "Headers received  {:>10.1} ms\n",
                at.as_secs_f64() * 1000.0
            )),
            sendito_grpc::Event::Received { at, .. } => {
                count += 1;
                if first.is_none() {
                    first = Some(*at);
                }
            }
            sendito_grpc::Event::Sent { .. } => sent += 1,
            sendito_grpc::Event::Finished { at, .. } => s.push_str(&format!(
                "Finished          {:>10.1} ms\n",
                at.as_secs_f64() * 1000.0
            )),
            _ => {}
        }
    }
    if let Some(f) = first {
        s.push_str(&format!(
            "First message     {:>10.1} ms\n",
            f.as_secs_f64() * 1000.0
        ));
    }
    s.push_str(&format!(
        "\nMessages sent     {sent}\nMessages received {count}\n"
    ));
    s
}

fn help_text(km: &Keymap, vim: bool) -> String {
    let mut s = String::from("All keys, including any you changed in the config.\n");
    s.push_str(&format!(
        "Current mode: {} (switch in Settings, {}).\n",
        if vim { "vim" } else { "conventional" },
        km.hint(Action::Settings, vim)
    ));
    let mut cur = None;
    for (ctx, a, conv, vimk) in km.rows() {
        if cur != Some(ctx) {
            cur = Some(ctx);
            s.push_str(&format!("\n── {} ──\n", ctx.title()));
        }
        let keys = if vim { vimk } else { conv };
        if keys.is_empty() {
            continue;
        }
        s.push_str(&format!("  {:<28} {}\n", keys, a.description()));
    }
    // Fixed keys, one section per place they work in.
    let mut place = "";
    let kw = crate::keymap::FIXED_KEYS
        .iter()
        .map(|(_, k, _)| k.chars().count())
        .max()
        .unwrap_or(28)
        .max(28);
    for (w, k, d) in crate::keymap::FIXED_KEYS {
        if *w != place {
            place = w;
            s.push_str(&format!("\n── Fixed keys · {w} ──\n"));
        }
        s.push_str(&format!("  {k:<kw$}  {d}\n"));
    }
    s.push_str("\n── Symbols ──\n");
    for (sym, what) in crate::keymap::SYMBOLS {
        s.push_str(&format!("  {sym:<28} {what}\n"));
    }
    s.push_str("\n── Commands (command palette) ──\n");
    for (c, d) in crate::overlay::COMMANDS {
        s.push_str(&format!("  {c:<28} {d}\n"));
    }
    s
}

impl App {
    // ----- overlay input ------------------------------------------------

    fn overlay_key(&mut self, key: KeyEvent) {
        let Some(mut ov) = self.overlay.take() else {
            return;
        };
        let keep = self.overlay_key_inner(&mut ov, key);
        if keep && self.overlay.is_none() {
            self.overlay = Some(ov);
        }
    }

    /// Returns whether the overlay stays open.
    fn overlay_key_inner(&mut self, ov: &mut Overlay, key: KeyEvent) -> bool {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match ov {
            Overlay::Report(r) => match key.code {
                KeyCode::Esc | KeyCode::Enter | KeyCode::Char('q') => {
                    if let Some(m) = r.close_status.clone() {
                        self.set_status(m, StatusKind::Ok);
                    }
                    false
                }
                KeyCode::Char('y') => {
                    self.clipboard.set(r.copy_text.clone(), false);
                    self.osc52.push(r.copy_text.clone());
                    self.set_status("copied (sent to the terminal clipboard)", StatusKind::Ok);
                    true
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    r.viewer.move_cursor(-1);
                    true
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    r.viewer.move_cursor(1);
                    true
                }
                KeyCode::PageUp => {
                    r.viewer.page(-1);
                    true
                }
                KeyCode::PageDown => {
                    r.viewer.page(1);
                    true
                }
                _ => true,
            },
            Overlay::Help { viewer, filter } => match key.code {
                KeyCode::Esc => false,
                KeyCode::Up => {
                    viewer.move_cursor(-1);
                    true
                }
                KeyCode::Down => {
                    viewer.move_cursor(1);
                    true
                }
                KeyCode::PageUp => {
                    viewer.page(-1);
                    true
                }
                KeyCode::PageDown => {
                    viewer.page(1);
                    true
                }
                KeyCode::F(1) => false,
                _ => {
                    if filter.handle_key(&key, false, &mut self.clipboard) == Outcome::Changed {
                        let q = filter.text();
                        viewer.search(&q);
                    }
                    if key.code == KeyCode::Enter {
                        viewer.next_match(1);
                    }
                    true
                }
            },
            Overlay::Settings { sel } => match key.code {
                KeyCode::Esc | KeyCode::Char('q') => false,
                KeyCode::Up | KeyCode::Char('k') => {
                    *sel = sel.saturating_sub(1);
                    true
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    *sel = (*sel + 1).min(SETTINGS.len() - 1);
                    true
                }
                KeyCode::Left | KeyCode::Char('h') => {
                    self.change_setting(SETTINGS[*sel].0, -1);
                    true
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Enter | KeyCode::Char(' ') => {
                    self.change_setting(SETTINGS[*sel].0, 1);
                    true
                }
                _ => true,
            },
            Overlay::Confirm(c) => match key.code {
                KeyCode::Esc => {
                    // Cancelling is an answer too: say what was kept.
                    let action = c.action.clone();
                    let cancel = c.buttons.len() - 1;
                    if matches!(action, ConfirmAction::Delete(_)) {
                        self.confirm(action, cancel);
                    }
                    false
                }
                KeyCode::Left | KeyCode::Char('h') | KeyCode::BackTab => {
                    c.sel = c.sel.saturating_sub(1);
                    true
                }
                KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => {
                    c.sel = (c.sel + 1).min(c.buttons.len() - 1);
                    true
                }
                KeyCode::Enter | KeyCode::Char(' ') => {
                    let (action, sel) = (c.action.clone(), c.sel);
                    self.confirm(action, sel);
                    false
                }
                KeyCode::Char('y') if c.buttons.len() == 2 => {
                    let action = c.action.clone();
                    self.confirm(action, 0);
                    false
                }
                KeyCode::Char('s') if c.buttons.len() == 3 => {
                    let action = c.action.clone();
                    self.confirm(action, 0);
                    false
                }
                KeyCode::Char('d') if c.buttons.len() == 3 => {
                    let action = c.action.clone();
                    self.confirm(action, 1);
                    false
                }
                KeyCode::Char('n') if c.buttons.len() == 2 => {
                    let action = c.action.clone();
                    if matches!(action, ConfirmAction::Delete(_)) {
                        self.confirm(action, 1);
                    }
                    false
                }
                _ => true,
            },
            Overlay::Picker(p) => {
                let vis = p.visible();
                match key.code {
                    KeyCode::Esc => false,
                    KeyCode::Up => {
                        p.sel = p.sel.saturating_sub(1);
                        true
                    }
                    KeyCode::Down => {
                        p.sel = (p.sel + 1).min(vis.len().saturating_sub(1));
                        true
                    }
                    KeyCode::PageUp => {
                        p.sel = p.sel.saturating_sub(10);
                        true
                    }
                    KeyCode::PageDown => {
                        p.sel = (p.sel + 10).min(vis.len().saturating_sub(1));
                        true
                    }
                    KeyCode::Enter => {
                        if let Some(i) = p.selected() {
                            let action = p.action.clone();
                            let item = p.items[i].clone();
                            let detail = p.details.get(i).cloned();
                            self.pick(action, i, &item, detail);
                        }
                        false
                    }
                    _ => {
                        if p.filter.handle_key(&key, false, &mut self.clipboard) == Outcome::Changed
                        {
                            p.sel = 0;
                        }
                        true
                    }
                }
            }
            Overlay::Palette(p) => {
                let vis = p.visible();
                match key.code {
                    KeyCode::Esc => false,
                    KeyCode::Up => {
                        p.sel = p.sel.saturating_sub(1);
                        p.armed = true;
                        true
                    }
                    KeyCode::Down | KeyCode::Tab => {
                        // The first move highlights the first row.
                        if p.armed {
                            p.sel = (p.sel + 1).min(vis.len().saturating_sub(1));
                        }
                        p.armed = true;
                        true
                    }
                    KeyCode::Enter if !p.armed => true,
                    KeyCode::Enter => {
                        let text = p.input.text();
                        let t = text.trim();
                        if t.starts_with(':')
                            && let Some(i) = vis.get(p.sel)
                            && let PaletteItem::Env(e) = &p.items[*i].0
                        {
                            let e = e.clone();
                            self.set_env(e);
                            return false;
                        }
                        if t.starts_with(':') {
                            // A full command with arguments runs as typed; otherwise the selection.
                            let typed_word = t
                                .trim_start_matches(':')
                                .split_whitespace()
                                .next()
                                .unwrap_or("");
                            let exact = COMMANDS.iter().any(|(c, _)| {
                                c.trim_start_matches(':').split_whitespace().next()
                                    == Some(typed_word)
                            });
                            if exact || vis.is_empty() {
                                self.run_command(t);
                            } else if let Some(i) = vis.get(p.sel)
                                && let PaletteItem::Command(c, _) = p.items[*i].0
                            {
                                let word = c.split_whitespace().next().unwrap_or(c);
                                if c.contains('<') || c.contains('[') {
                                    let mut input = TextEditor::single(&format!("{word} "));
                                    input.move_to_end();
                                    p.input = input;
                                    return true;
                                }
                                self.run_command(word);
                            }
                            return false;
                        }
                        if let Some(i) = vis.get(p.sel) {
                            let item = p.items[*i].0.clone();
                            match item {
                                PaletteItem::Action(a) => self.dispatch(a),
                                PaletteItem::Open(rel) => {
                                    self.tree.select_rel(&rel);
                                    self.open_request(&rel);
                                }
                                PaletteItem::Env(e) => self.set_env(e),
                                PaletteItem::Command(c, _) => self.run_command(c),
                            }
                        }
                        false
                    }
                    _ => {
                        let out = p.input.handle_key(&key, false, &mut self.clipboard);
                        if out == Outcome::Changed {
                            p.sel = 0;
                            p.armed = !p.input.text().trim().is_empty();
                        }
                        // Backspace on an empty input closes the palette.
                        !(out == Outcome::Handled
                            && key.code == KeyCode::Backspace
                            && p.input.text().is_empty())
                    }
                }
            }
            Overlay::Prompt(p) => {
                let search = p.action == PromptAction::Search;
                // The open option list of a choice row takes the keys first.
                if let Some(hl) = p.open {
                    let n = p
                        .choices
                        .get(p.focus.wrapping_sub(1))
                        .map_or(0, |c| c.1.len());
                    match key.code {
                        KeyCode::Up => p.open = Some((hl + n.max(1) - 1) % n.max(1)),
                        KeyCode::Down | KeyCode::Tab => p.open = Some((hl + 1) % n.max(1)),
                        KeyCode::Enter | KeyCode::Char(' ') => {
                            if let Some(c) = p.choices.get_mut(p.focus - 1) {
                                c.2 = hl.min(n.saturating_sub(1));
                            }
                            p.open = None;
                            // Back to the name, so the next Enter confirms.
                            p.focus = 0;
                        }
                        KeyCode::Esc => p.open = None,
                        _ => {}
                    }
                    return true;
                }
                // Ctrl+S / F5 confirm from any row (Enter on a choice row opens its list).
                let confirm_any =
                    matches!(key.code, KeyCode::F(5)) || (ctrl && key.code == KeyCode::Char('s'));
                if confirm_any && !search {
                    return match self.prompt_ok(p) {
                        Ok(()) => false,
                        Err(e) => {
                            p.error = Some(e);
                            true
                        }
                    };
                }
                match key.code {
                    KeyCode::Esc => false,
                    // Space opens a choice row's list; Enter always confirms.
                    KeyCode::Char(' ') if p.focus > 0 => {
                        p.open = p.choices.get(p.focus - 1).map(|c| c.2);
                        true
                    }
                    KeyCode::Tab | KeyCode::Down if !p.choices.is_empty() => {
                        p.focus = (p.focus + 1) % (p.choices.len() + 1);
                        true
                    }
                    KeyCode::BackTab | KeyCode::Up if !p.choices.is_empty() => {
                        p.focus = (p.focus + p.choices.len()) % (p.choices.len() + 1);
                        true
                    }
                    KeyCode::Left | KeyCode::Right if p.focus > 0 => {
                        if let Some((_, opts, i)) = p.choices.get_mut(p.focus - 1) {
                            let dir: isize = if key.code == KeyCode::Left { -1 } else { 1 };
                            *i = ((*i as isize + dir).rem_euclid(opts.len().max(1) as isize))
                                as usize;
                        }
                        true
                    }
                    KeyCode::Enter => {
                        if search {
                            let q = p.input.text();
                            let mut n = 0;
                            self.with_viewer(|v| n = v.search(&q));
                            self.focus = Focus::RespBody;
                            if n == 0 && !q.is_empty() {
                                self.set_status(format!("no match for \"{q}\""), StatusKind::Warn);
                            } else if n > 0 {
                                self.set_status(
                                    format!("{n} match(es) · n/N next/previous"),
                                    StatusKind::Info,
                                );
                            }
                            return false;
                        }
                        match self.prompt_ok(p) {
                            Ok(()) => false,
                            Err(e) => {
                                p.error = Some(e);
                                true
                            }
                        }
                    }
                    _ => {
                        if p.focus == 0 {
                            p.input.handle_key(&key, false, &mut self.clipboard);
                            p.error = None;
                            p.note = None;
                        }
                        true
                    }
                }
            }
            Overlay::Import(i) => {
                let submit = matches!(key.code, KeyCode::F(5))
                    || (ctrl && matches!(key.code, KeyCode::Char('s') | KeyCode::Enter));
                if key.code == KeyCode::Esc {
                    return false;
                }
                if submit {
                    let text = i.input.text();
                    return match self.do_import(&text) {
                        Ok(()) => false,
                        Err(e) => {
                            i.error = Some(e);
                            true
                        }
                    };
                }
                i.input.handle_key(&key, false, &mut self.clipboard);
                i.error = None;
                true
            }
            Overlay::Env(e) => {
                if key.code == KeyCode::Esc {
                    if e.dirty {
                        // Keep the editor open behind the confirm dialog.
                        let ed = std::mem::replace(
                            e,
                            EnvEditor {
                                envs: vec![],
                                current: 0,
                                table: EnvEditor::table(),
                                dirty: false,
                                error: None,
                            },
                        );
                        self.parked_env = Some(ed);
                        self.overlay = Some(Overlay::Confirm(Confirm {
                            title: "Unsaved environment changes".into(),
                            message: "Save the changes to this environment?".into(),
                            buttons: vec!["Save".into(), "Discard".into(), "Cancel".into()],
                            sel: 0,
                            action: ConfirmAction::EnvClose,
                        }));
                        return false;
                    }
                    return false;
                }
                if ctrl && key.code == KeyCode::Char('s') {
                    let mut ed = std::mem::replace(
                        e,
                        EnvEditor {
                            envs: vec![],
                            current: 0,
                            table: EnvEditor::table(),
                            dirty: false,
                            error: None,
                        },
                    );
                    self.save_env_editor(&mut ed);
                    *e = ed;
                    return true;
                }
                if ctrl && key.code == KeyCode::Char('n') {
                    let ed = std::mem::replace(
                        e,
                        EnvEditor {
                            envs: vec![],
                            current: 0,
                            table: EnvEditor::table(),
                            dirty: false,
                            error: None,
                        },
                    );
                    self.parked_env = Some(ed);
                    self.overlay = Some(Overlay::Prompt(Prompt {
                        title: "New environment".into(),
                        label: "Name".into(),
                        input: TextEditor::single(""),
                        choices: vec![],
                        focus: 0,
                        action: PromptAction::NewEnv,
                        error: None,
                        note: None,
                        open: None,
                    }));
                    return false;
                }
                if ctrl && matches!(key.code, KeyCode::PageUp | KeyCode::PageDown) {
                    if e.dirty {
                        e.error = Some("save (Ctrl+S) or discard changes before switching".into());
                        return true;
                    }
                    let n = e.envs.len() as isize;
                    let d: isize = if key.code == KeyCode::PageUp { -1 } else { 1 };
                    e.current = ((e.current as isize + d).rem_euclid(n)) as usize;
                    let mut ed = std::mem::replace(
                        e,
                        EnvEditor {
                            envs: vec![],
                            current: 0,
                            table: EnvEditor::table(),
                            dirty: false,
                            error: None,
                        },
                    );
                    self.load_env_into(&mut ed);
                    *e = ed;
                    return true;
                }
                if ctrl && key.code == KeyCode::Char('d') {
                    e.table.delete_row();
                    e.dirty = true;
                    return true;
                }
                if key.code == KeyCode::Enter {
                    e.table.next_cell();
                    return true;
                }
                match e.table.handle_key(&key, false, &mut self.clipboard) {
                    TableOutcome::Changed => e.dirty = true,
                    TableOutcome::Leave => return false,
                    _ => {}
                }
                true
            }
        }
    }

    fn confirm(&mut self, action: ConfirmAction, sel: usize) {
        match action {
            ConfirmAction::SendAnyway => {
                if sel == 0 {
                    self.send(true);
                }
            }
            ConfirmAction::Delete(rel) => {
                if sel != 0 {
                    self.set_status(format!("kept {rel}: nothing was deleted"), StatusKind::Info);
                    return;
                }
                match self.project.coll.delete(&rel) {
                    Ok(()) => {
                        for t in &mut self.tabs {
                            if t.path
                                .as_deref()
                                .is_some_and(|p| p == rel || p.starts_with(&format!("{rel}/")))
                            {
                                t.path = None;
                                t.saved = None;
                                t.name = format!("{} (deleted)", t.name);
                            }
                        }
                        self.rescan();
                        self.set_status(format!("deleted {}", name_of(&rel)), StatusKind::Ok);
                    }
                    Err(e) => self.set_status(e.to_string(), StatusKind::Error),
                }
            }
            ConfirmAction::Quit => {
                if sel == 0 {
                    self.quit = true;
                } else {
                    self.set_status("staying: the unsaved tabs are still open", StatusKind::Info);
                }
            }
            ConfirmAction::CloseTab(id) => {
                let Some(i) = self.tabs.iter().position(|t| t.id == id) else {
                    return;
                };
                self.active = i;
                match sel {
                    0 => {
                        let had_path = self.tabs[i].path.is_some();
                        self.save();
                        if had_path && !self.tabs.get(i).is_some_and(|t| t.dirty()) {
                            self.close_tab(true);
                        }
                    }
                    1 => self.close_tab(true),
                    _ => {
                        let name = self.tabs[i].title();
                        self.set_status(
                            format!("\"{name}\" kept open: nothing was saved or discarded"),
                            StatusKind::Info,
                        );
                    }
                }
            }
            ConfirmAction::EnvClose => {
                let Some(mut ed) = self.parked_env.take() else {
                    return;
                };
                match sel {
                    0 => {
                        if !self.save_env_editor(&mut ed) {
                            self.overlay = Some(Overlay::Env(ed));
                        }
                    }
                    1 => {}
                    _ => self.overlay = Some(Overlay::Env(ed)),
                }
            }
            ConfirmAction::ClearHistory => {
                if sel == 0 {
                    let _ = sendito_store::history::clear(&sendito_store::history::history_path(
                        &self.project.root,
                    ));
                    self.history = Some(Vec::new());
                    self.set_status("history cleared", StatusKind::Ok);
                }
            }
        }
    }

    fn pick(&mut self, action: PickAction, index: usize, item: &str, detail: Option<String>) {
        match action {
            PickAction::Env => {
                let n = self.env_list.len();
                if index == 0 {
                    self.set_env(None);
                } else if index <= n {
                    self.set_env(Some(item.to_string()));
                } else {
                    self.open_env_editor();
                }
            }
            PickAction::Method => {
                if item == "Custom…" {
                    self.overlay = Some(Overlay::Prompt(Prompt {
                        title: "Custom HTTP method".into(),
                        label: "Method".into(),
                        input: TextEditor::single(""),
                        choices: vec![],
                        focus: 0,
                        action: PromptAction::CustomMethod,
                        error: None,
                        note: None,
                        open: None,
                    }));
                    return;
                }
                if let Some(t) = self.tab_mut() {
                    t.ed.method = item.to_string();
                }
                self.changed();
            }
            PickAction::GrpcMethod => {
                if let Some((svc, m)) = item.rsplit_once('/') {
                    let mut fill = false;
                    if let Some(t) = self.tab_mut() {
                        let changed = t.ed.service != svc || t.ed.grpc_method != m;
                        t.ed.service = svc.to_string();
                        t.ed.grpc_method = m.to_string();
                        let msg = t.ed.message.text();
                        fill = changed && (msg.trim().is_empty() || msg.trim() == "{}");
                    }
                    self.changed();
                    if fill {
                        self.load_schema(false, true, false);
                    }
                }
            }
            PickAction::McpServer => {
                if let Some(t) = self.tab_mut() {
                    t.ed.mcp_server.set_text(item);
                }
                self.changed();
            }
            PickAction::McpTool => {
                if let Some(t) = self.tab_mut() {
                    t.ed.mcp_tool.set_text(item);
                    let args = t.ed.message.text();
                    let unchanged =
                        matches!(&t.saved, Some(Request::Mcp(saved)) if saved.arguments == args);
                    if unchanged
                        && (args.trim().is_empty() || args.trim() == "{}")
                        && let Some(tool) = t.mcp_tools.get(index)
                    {
                        let template = mcp_argument_template(&tool.input_schema);
                        t.ed.message.replace_all(&template);
                    }
                }
                self.changed();
            }
            PickAction::MoveTo(rel) => {
                let folder = detail.unwrap_or_default();
                self.move_node(&rel, &folder);
            }
            PickAction::Choice(f) => {
                if let Some(t) = self.tab_mut() {
                    t.ed.set_choice(f, index);
                }
                self.changed();
            }
            PickAction::RequestKind => {}
        }
    }

    fn prompt_ok(&mut self, p: &mut Prompt) -> Result<(), String> {
        let value = p.input.text();
        let choice_at = |k: usize| p.choices.get(k).map(|(_, _, i)| *i).unwrap_or(0);
        // Folder choice is the last row when present.
        let folder_choice = p
            .choices
            .iter()
            .position(|(l, _, _)| l == "In folder")
            .map(choice_at);
        let choice = choice_at(0);
        match p.action.clone() {
            PromptAction::NewRequest { folder } => {
                let folder = folder_choice
                    .and_then(|i| self.tree.all_folders().get(i).cloned())
                    .unwrap_or(folder);
                let req = match choice {
                    1 => Request::new_grpc(),
                    2 => Request::new_mcp(),
                    _ => Request::new_http(),
                };
                let rel = self
                    .project
                    .coll
                    .create_request(&folder, value.trim(), &req)
                    .map_err(|e| e.to_string())?;
                self.rescan();
                self.tree.select_rel(&rel);
                self.open_request(&rel);
                self.set_status(
                    format!(
                        "created {rel} (an empty file) · your edits show ● until Ctrl+S saves them"
                    ),
                    StatusKind::Ok,
                );
            }
            PromptAction::NewFolder { folder } => {
                let folder = folder_choice
                    .and_then(|i| self.tree.all_folders().get(i).cloned())
                    .unwrap_or(folder);
                let rel = self
                    .project
                    .coll
                    .create_folder(&folder, value.trim())
                    .map_err(|e| e.to_string())?;
                self.tree.expanded.insert(rel.clone());
                self.rescan();
                self.tree.select_rel(&rel);
                self.set_status(format!("created folder {rel}"), StatusKind::Ok);
            }
            PromptAction::Rename(rel) => {
                let new_rel = self
                    .project
                    .coll
                    .rename(&rel, value.trim())
                    .map_err(|e| e.to_string())?;
                if self.tree.expanded.remove(&rel) {
                    self.tree.expanded.insert(new_rel.clone());
                }
                self.repath_tabs(&rel, &new_rel);
                self.rescan();
                self.tree.select_rel(&new_rel);
            }
            PromptAction::SaveAs { tab } => {
                let folders = self.tree.all_folders();
                let folder = folders.get(choice).cloned().unwrap_or_default();
                let model = self
                    .tabs
                    .iter()
                    .find(|t| t.id == tab)
                    .map(|t| t.model.clone())
                    .ok_or("tab closed")?;
                let rel = self
                    .project
                    .coll
                    .create_request(&folder, value.trim(), &model)
                    .map_err(|e| e.to_string())?;
                if let Some(t) = self.tab_by_id(tab) {
                    t.path = Some(rel.clone());
                    t.name = name_of(&rel).to_string();
                    t.saved = Some(model);
                }
                self.rescan();
                self.tree.select_rel(&rel);
                self.set_status(format!("saved {rel}"), StatusKind::Ok);
                self.session_dirty = true;
            }
            PromptAction::SaveHistory(i) => {
                let folders = self.tree.all_folders();
                let folder = folders.get(choice).cloned().unwrap_or_default();
                let e = self
                    .history
                    .as_ref()
                    .and_then(|h| h.get(i))
                    .cloned()
                    .ok_or("entry gone")?;
                let req = sendito_store::request_file::parse(&e.request)?;
                let rel = self
                    .project
                    .coll
                    .create_request(&folder, value.trim(), &req)
                    .map_err(|e| match e {
                        sendito_store::StoreError::Exists(_) => format!(
                            "\"{}\" exists in /{folder}: choose another name",
                            value.trim()
                        ),
                        e => e.to_string(),
                    })?;
                self.rescan();
                self.tree.select_rel(&rel);
                self.set_status(format!("saved history entry as {rel}"), StatusKind::Ok);
            }
            PromptAction::SaveBody => {
                let m = self.save_body(&value)?;
                self.set_status(m, StatusKind::Ok);
            }
            PromptAction::NewEnv => {
                let name = value.trim().to_string();
                let env = sendito_store::env::EnvFile::new(&name);
                if self.project.envs.list().contains(&name) {
                    return Err(format!("environment \"{name}\" already exists"));
                }
                self.project.envs.save(&env).map_err(|e| e.to_string())?;
                self.env_list = self.project.envs.list();
                let mut envs = vec![COLLECTION_ENV.to_string()];
                envs.extend(self.env_list.iter().cloned());
                let current = envs.iter().position(|e| *e == name).unwrap_or(0);
                let mut ed = self.parked_env.take().unwrap_or(EnvEditor {
                    envs: vec![],
                    current: 0,
                    table: EnvEditor::table(),
                    dirty: false,
                    error: None,
                });
                ed.envs = envs;
                ed.current = current;
                self.load_env_into(&mut ed);
                self.overlay = Some(Overlay::Env(ed));
            }
            PromptAction::Search => {}
            PromptAction::CustomMethod => {
                let m = value.trim().to_ascii_uppercase();
                if m.is_empty()
                    || !m
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
                {
                    return Err("a method is a single token such as PURGE".into());
                }
                if let Some(t) = self.tab_mut() {
                    t.ed.method = m;
                }
                self.changed();
            }
        }
        Ok(())
    }

    // ----- mouse --------------------------------------------------------

    fn hit_at(&self, x: u16, y: u16) -> Option<(Rect, Hit)> {
        self.hits
            .iter()
            .rev()
            .find(|(r, _)| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height)
            .cloned()
    }

    pub fn on_mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        self.hover = Some((x, y));
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                // A click starts a new action: an old message no longer applies.
                self.clear_transient_status();
                let double = self.last_click.is_some_and(|(t, lx, ly)| {
                    t.elapsed() < Duration::from_millis(400) && lx == x && ly == y
                });
                self.last_click = Some((Instant::now(), x, y));
                if let Some((rect, hit)) = self.hit_at(x, y) {
                    self.click(
                        hit,
                        rect,
                        x,
                        y,
                        double,
                        m.modifiers.contains(KeyModifiers::SHIFT),
                    );
                } else if self.overlay.is_some() {
                    // Click outside a dialog closes it (except confirmations).
                    if !matches!(
                        self.overlay,
                        Some(Overlay::Confirm(_)) | Some(Overlay::Env(_))
                    ) {
                        self.overlay = None;
                    }
                }
            }
            MouseEventKind::Down(MouseButton::Middle) => {
                if let Some((_, Hit::Tab(i))) = self.hit_at(x, y) {
                    self.active = i;
                    self.close_tab(false);
                }
            }
            MouseEventKind::Drag(MouseButton::Left) => self.on_drag(x, y),
            MouseEventKind::Up(MouseButton::Left) => {
                if let Some(Drag::Tree {
                    rel, active: true, ..
                }) = self.drag.take()
                    && let Some(target) = self.drop_target.take()
                    && target != parent_rel(&rel)
                {
                    self.move_node(&rel, &target);
                }
                self.drag = None;
                self.drop_target = None;
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let d: isize = if m.kind == MouseEventKind::ScrollUp {
                    -3
                } else {
                    3
                };
                self.scroll_at(x, y, d);
            }
            _ => {}
        }
    }

    fn scroll_at(&mut self, x: u16, y: u16, d: isize) {
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        if let Some(ov) = &mut self.overlay {
            match ov {
                Overlay::Report(r) => r.viewer.scroll_by(d),
                Overlay::Help { viewer, .. } => viewer.scroll_by(d),
                Overlay::Picker(p) => {
                    let n = p.visible().len();
                    p.sel = ((p.sel as isize + d.signum()).clamp(0, n.saturating_sub(1) as isize))
                        as usize;
                }
                Overlay::Palette(p) => {
                    p.armed = true;
                    let n = p.visible().len();
                    p.sel = ((p.sel as isize + d.signum()).clamp(0, n.saturating_sub(1) as isize))
                        as usize;
                }
                _ => {}
            }
            return;
        }
        if inside(self.areas.sidebar) {
            if self.sidebar_mode == SidebarMode::Tree {
                let max = self
                    .tree
                    .rows
                    .len()
                    .saturating_sub(self.areas.tree.height as usize);
                self.tree.scroll =
                    ((self.tree.scroll as isize + d).clamp(0, max as isize)) as usize;
            } else {
                let n = self.history.as_ref().map(|h| h.len()).unwrap_or(0);
                self.history_scroll = ((self.history_scroll as isize + d)
                    .clamp(0, n.saturating_sub(1) as isize))
                    as usize;
            }
        } else if inside(self.areas.resp_body) {
            self.with_viewer(|v| v.scroll_by(d));
        } else if inside(self.areas.req_body) {
            let rects = self.areas.field_rects.clone();
            if let Some(t) = self.tab_mut() {
                let fields = t.fields();
                for (i, r) in rects.iter().enumerate() {
                    if inside(*r)
                        && let Some(f) = fields.get(i)
                    {
                        if let Some(e) = t.ed.text_mut(*f) {
                            e.scroll_by(d);
                        } else if let Some(tb) = t.ed.table_mut(*f) {
                            let max = tb.rows.len() as isize;
                            tb.scroll = ((tb.scroll as isize + d).clamp(0, max)) as usize;
                        }
                    }
                }
            }
        }
    }

    fn on_drag(&mut self, x: u16, y: u16) {
        match self.drag.clone() {
            Some(Drag::Sidebar) => {
                let w = if self.project.config.ui.sidebar == "right" {
                    self.size.0.saturating_sub(x)
                } else {
                    x.saturating_sub(self.areas.sidebar.x) + 1
                };
                self.layout.sidebar_width = w.clamp(16, self.size.0.saturating_sub(30).max(16));
                self.session_dirty = true;
            }
            Some(Drag::Split) => {
                let main = self.areas.main;
                if main.height > 4 {
                    let rel = y.saturating_sub(main.y) as u32 * 100 / main.height as u32;
                    self.layout.request_pct = (rel as u16).clamp(20, 80);
                    self.session_dirty = true;
                }
            }
            Some(Drag::Tree { rel, start, active }) => {
                let moved = start.0.abs_diff(x) > 1 || start.1 != y;
                let active = active || moved;
                self.drag = Some(Drag::Tree {
                    rel: rel.clone(),
                    start,
                    active,
                });
                if active {
                    self.drop_target = match self.hit_at(x, y) {
                        Some((_, Hit::TreeRow(i))) => self.tree.rows.get(i).map(|r| {
                            if r.is_folder() {
                                r.rel.clone()
                            } else {
                                parent_rel(&r.rel).to_string()
                            }
                        }),
                        Some((_, Hit::SidebarArea)) => Some(String::new()),
                        _ => None,
                    }
                    .filter(|t| *t != rel && !t.starts_with(&format!("{rel}/")));
                }
            }
            Some(Drag::Text { field, bar }) => {
                let rect = if bar {
                    self.areas.bar_url
                } else {
                    field
                        .and_then(|f| self.areas.field_rects.get(f).copied())
                        .unwrap_or_default()
                };
                let rx = x.saturating_sub(rect.x);
                let ry = y.saturating_sub(rect.y).min(rect.height.saturating_sub(1));
                if let Some(e) = self.focused_text() {
                    e.click(rx, ry, true);
                }
            }
            None => {}
        }
    }

    fn click(&mut self, hit: Hit, rect: Rect, x: u16, y: u16, double: bool, shift: bool) {
        if self.overlay.is_some() {
            if let Hit::Overlay(oh) = hit {
                self.overlay_click(oh, rect, x, y, double);
            }
            return;
        }
        match hit {
            Hit::Tab(i) => {
                self.active = i;
                self.session_dirty = true;
            }
            Hit::TabClose(i) => {
                self.active = i;
                self.close_tab(false);
            }
            Hit::NewTab => self.new_tab(Request::new_http()),
            Hit::EnvSelector => self.open_env_picker(),
            Hit::HelpButton => self.open_help(),
            Hit::SidebarMode(m) => {
                self.sidebar_mode = m;
                self.focus = Focus::Sidebar;
                if m == SidebarMode::History {
                    self.load_history();
                }
            }
            Hit::SidebarArea => self.focus = Focus::Sidebar,
            Hit::TreeRow(i) => {
                self.focus = Focus::Sidebar;
                self.tree.selected = i;
                let row = self.tree.rows.get(i).cloned();
                if let Some(row) = row {
                    // Click on the folder arrow toggles, double-click opens requests.
                    let arrow_x = rect.x + (row.depth as u16) * 2;
                    if row.is_folder() && (double || x <= arrow_x + 1) {
                        self.tree.toggle(&row.rel);
                    } else if !row.is_folder() && double {
                        self.open_request(&row.rel);
                    }
                    self.drag = Some(Drag::Tree {
                        rel: row.rel,
                        start: (x, y),
                        active: false,
                    });
                }
            }
            Hit::TreeFilter => {
                self.focus = Focus::Filter;
                self.sidebar_mode = SidebarMode::Tree;
            }
            Hit::HistoryRow(i) => {
                self.focus = Focus::Sidebar;
                self.history_sel = i;
                if double {
                    self.open_history(i, false);
                }
            }
            Hit::RecentRow(i) => self.open_history(i, false),
            Hit::Bar(p) => {
                self.focus = Focus::Bar(p);
                let kind = self.tab().map(|t| t.ed.kind);
                match (kind, p) {
                    (Some(_), 2) => self.dispatch(Action::Send),
                    (Some(Kind::Http), 0) => self.open_method_picker(None),
                    (Some(Kind::Grpc), 1) => self.open_grpc_method_picker(),
                    (Some(Kind::Http), 1) | (Some(Kind::Grpc), 0) => {
                        let (rx, ry) = (x.saturating_sub(rect.x), y.saturating_sub(rect.y));
                        if let Some(e) = self.focused_text() {
                            e.click(rx, ry, shift);
                        }
                        self.drag = Some(Drag::Text {
                            field: None,
                            bar: true,
                        });
                    }
                    _ => {}
                }
            }
            Hit::ReqTab(i) => {
                self.focus = Focus::ReqTabs;
                if let Some(t) = self.tab_mut() {
                    t.req_tab = i;
                    t.field = 0;
                }
            }
            Hit::ReqField(i) => {
                self.focus = Focus::ReqBody;
                if let Some(t) = self.tab_mut() {
                    t.field = i;
                    if let Some(f) = t.current_field() {
                        if f.is_toggle() {
                            t.ed.cycle(f, 1);
                        } else if f.is_choice() {
                            self.open_choice_picker(f);
                            return;
                        }
                    }
                }
                self.changed();
            }
            Hit::TextArea { field } => {
                self.focus = Focus::ReqBody;
                let (rx, ry) = (x.saturating_sub(rect.x), y.saturating_sub(rect.y));
                if let Some(t) = self.tab_mut() {
                    t.field = field;
                    if let Some(f) = t.current_field()
                        && let Some(e) = t.ed.text_mut(f)
                    {
                        e.click(rx, ry, shift);
                    }
                }
                self.drag = Some(Drag::Text {
                    field: Some(field),
                    bar: false,
                });
            }
            Hit::TableCell { field, row, col } => {
                self.focus = Focus::ReqBody;
                let (rx, ry) = (x.saturating_sub(rect.x), y.saturating_sub(rect.y));
                let mut changed = false;
                if let Some(t) = self.tab_mut() {
                    t.field = field;
                    if let Some(f) = t.current_field()
                        && let Some(tb) = t.ed.table_mut(f)
                    {
                        tb.select(row, col);
                        if !tb.editing_text() {
                            changed = tb.activate(1);
                        } else if let Some(e) = tb.editor_mut() {
                            e.click(rx, ry, false);
                        }
                    }
                }
                if changed {
                    self.changed();
                }
            }
            Hit::TableToggle { field, row } => {
                if let Some(t) = self.tab_mut() {
                    t.field = field;
                    if let Some(f) = t.current_field()
                        && let Some(tb) = t.ed.table_mut(f)
                    {
                        tb.select(row, 0);
                        tb.toggle_row();
                    }
                }
                self.focus = Focus::ReqBody;
                self.changed();
            }
            Hit::TableDelete { field, row } => {
                if let Some(t) = self.tab_mut() {
                    t.field = field;
                    if let Some(f) = t.current_field()
                        && let Some(tb) = t.ed.table_mut(f)
                    {
                        tb.select(row, 0);
                        tb.delete_row();
                    }
                }
                self.focus = Focus::ReqBody;
                self.changed();
            }
            Hit::RespTab(i) => {
                self.focus = Focus::RespTabs;
                if let Some(t) = self.tab_mut() {
                    t.resp_tab = i;
                }
            }
            Hit::RespView => {
                self.focus = Focus::RespBody;
                let ry = y.saturating_sub(rect.y);
                let gutter_click = x < rect.x + 2;
                self.with_viewer(|v| {
                    v.click(ry);
                    if gutter_click || double {
                        v.toggle_fold();
                    }
                });
            }
            Hit::ViewMode(m) => {
                self.focus = Focus::RespBody;
                self.set_view(m);
            }
            Hit::RespAction(a) => match a {
                RespAction::Copy => self.copy_body(),
                RespAction::Save => self.prompt_save_body(),
                RespAction::Wrap => self.with_viewer(Viewer::toggle_wrap),
                RespAction::Cancel => self.cancel(),
                RespAction::SendMessage => self.send(false),
                RespAction::EndStream => self.end_stream(),
                RespAction::Template => self.load_schema(false, true, false),
                RespAction::Format => self.format_body(),
            },
            Hit::SidebarBorder => self.drag = Some(Drag::Sidebar),
            Hit::SplitBorder => self.drag = Some(Drag::Split),
            Hit::Overlay(_) => {}
        }
    }

    fn overlay_click(&mut self, oh: OverlayHit, rect: Rect, x: u16, y: u16, double: bool) {
        let Some(mut ov) = self.overlay.take() else {
            return;
        };
        let mut keep = true;
        match (&mut ov, oh) {
            (_, OverlayHit::Close) | (_, OverlayHit::Outside) => {
                keep = matches!(ov, Overlay::Confirm(_))
            }
            (Overlay::Picker(p), OverlayHit::Item(i)) => {
                p.sel = i;
                if let Some(idx) = p.selected() {
                    let (action, item, detail) = (
                        p.action.clone(),
                        p.items[idx].clone(),
                        p.details.get(idx).cloned(),
                    );
                    self.pick(action, idx, &item, detail);
                }
                keep = false;
            }
            (Overlay::Palette(p), OverlayHit::Item(i)) => {
                p.sel = i;
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                return;
            }
            (Overlay::Confirm(c), OverlayHit::Button(i)) => {
                let action = c.action.clone();
                self.confirm(action, i);
                keep = false;
            }
            (Overlay::Settings { sel }, OverlayHit::Item(i)) => {
                *sel = i;
                self.change_setting(SETTINGS[i].0, 1);
            }
            (Overlay::Prompt(p), OverlayHit::Choice(k)) => {
                p.focus = k + 1;
                if let Some((_, opts, i)) = p.choices.get_mut(k) {
                    *i = (*i + 1) % opts.len().max(1);
                }
            }
            (Overlay::Prompt(p), OverlayHit::Input) => {
                p.focus = 0;
                p.input.click(x.saturating_sub(rect.x), 0, false);
            }
            (Overlay::Prompt(_), OverlayHit::Button(0)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
                return;
            }
            (Overlay::Prompt(_), OverlayHit::Button(_)) => keep = false,
            (Overlay::Import(i), OverlayHit::Input) => {
                i.input
                    .click(x.saturating_sub(rect.x), y.saturating_sub(rect.y), false)
            }
            (Overlay::Import(_), OverlayHit::Button(0)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE));
                return;
            }
            (Overlay::Import(_), OverlayHit::Button(_)) => keep = false,
            (Overlay::Env(e), OverlayHit::EnvTab(i)) => {
                if e.dirty {
                    e.error = Some("save (Ctrl+S) or discard changes before switching".into());
                } else {
                    e.current = i;
                    let mut ed = std::mem::replace(
                        e,
                        EnvEditor {
                            envs: vec![],
                            current: 0,
                            table: EnvEditor::table(),
                            dirty: false,
                            error: None,
                        },
                    );
                    self.load_env_into(&mut ed);
                    *e = ed;
                }
            }
            (Overlay::Env(e), OverlayHit::EnvCell { row, col }) => {
                e.table.select(row, col);
                if !e.table.editing_text() && e.table.activate(1) {
                    e.dirty = true;
                } else if let Some(ed) = e.table.editor_mut() {
                    ed.click(x.saturating_sub(rect.x), 0, false);
                }
            }
            (Overlay::Env(_), OverlayHit::Button(0)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
                return;
            }
            (Overlay::Env(_), OverlayHit::Button(1)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Char('n'), KeyModifiers::CONTROL));
                return;
            }
            (Overlay::Env(_), OverlayHit::Button(_)) => {
                self.overlay = Some(ov);
                self.overlay_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
                return;
            }
            (Overlay::Report(r), OverlayHit::Button(0)) => {
                self.clipboard.set(r.copy_text.clone(), false);
                self.osc52.push(r.copy_text.clone());
                self.set_status("copied (sent to the terminal clipboard)", StatusKind::Ok);
            }
            (Overlay::Report(r), OverlayHit::Button(_)) => {
                if let Some(m) = r.close_status.clone() {
                    self.set_status(m, StatusKind::Ok);
                }
                keep = false;
            }
            // The right column of a two-column Help continues the left one.
            (Overlay::Help { viewer, .. }, OverlayHit::Item(off)) => viewer
                .click((y.saturating_sub(rect.y) as usize + off).min(u16::MAX as usize) as u16),
            _ => {
                let _ = double;
            }
        }
        if keep && self.overlay.is_none() {
            self.overlay = Some(ov);
        }
    }
}
