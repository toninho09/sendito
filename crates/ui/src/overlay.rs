//! Modal overlays: palette, help, settings, confirm, prompt, picker, env
//! editor, import, report.

use crate::editor::TextEditor;
use crate::form::{Cell, ColKind, Column, Table};
use crate::viewer::Viewer;

/// What to do when a confirm dialog's first button is chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfirmAction {
    SendAnyway,
    /// Send although a secret did not load (its `{{name}}` goes out as typed).
    SendMissingSecrets,
    Delete(String),
    Quit,
    /// Close tab: button 0 = save, 1 = discard.
    CloseTab(u64),
    /// Close tab with a live request or open session: 0 = close, 1 = keep it.
    CloseTabRunning(u64),
    /// Env editor closing with changes: 0 = save, 1 = discard.
    EnvClose,
    /// MCP servers editor closing with changes: 0 = save, 1 = discard.
    McpClose,
    /// A config editor (MCP server details, OAuth, keymap) closing with
    /// changes: 0 = save, 1 = discard.
    CfgClose,
    /// Prompt or import dialog closing with typed text: 0 = discard,
    /// 1 = keep editing.
    DialogClose,
    ClearHistory,
    /// Delete the cookie with this number from the active environment's jar.
    DeleteCookie(usize),
    /// Delete every cookie of the active environment.
    ClearCookies,
    /// An MCP server asks for a sign-in: 0 = sign in, then do `after` in `tab`.
    McpLogin {
        server: String,
        tab: u64,
        after: McpAfterLogin,
    },
}

/// What runs again after a sign-in that a failed MCP call started.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpAfterLogin {
    Send,
    Tools { open_picker: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptAction {
    NewRequest {
        folder: String,
    },
    NewFolder {
        folder: String,
    },
    Rename(String),
    SaveAs {
        tab: u64,
    },
    SaveBody,
    NewEnv,
    /// The entry itself, not its row: the list can reload under the prompt.
    SaveHistory(Box<sendito_store::history::HistoryEntry>),
    CustomMethod,
    /// A new server for the MCP editor: (name, URL or command).
    McpServer {
        name: String,
    },
    /// A typed setting of the settings dialog (number, path or text).
    Setting(SettingId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickAction {
    Env,
    Method,
    GrpcMethod,
    McpServer,
    McpTool,
    MoveTo(String),
    Choice(crate::tab::Field),
    RequestKind,
    Theme,
    /// The cookie jar: item i is cookie i + 1; the last item clears all.
    Cookie,
    /// Sign in to (or out of) an OAuth identity or an MCP server.
    Auth {
        login: bool,
    },
    /// Open the details editor of an MCP server.
    McpServerDetails,
}

pub struct Prompt {
    pub title: String,
    pub label: String,
    pub input: TextEditor,
    /// The field's text when the dialog opened: a different text is pending.
    pub initial: String,
    /// Choice rows (e.g. HTTP / gRPC, target folder): (label, options, selected).
    pub choices: Vec<(String, Vec<String>, usize)>,
    /// 0 = input, 1.. = choice rows.
    pub focus: usize,
    pub action: PromptAction,
    pub error: Option<String>,
    /// Information about a pre-filled value (shown until the input changes).
    pub note: Option<String>,
    /// The option list of the focused choice row is open, with this row highlighted.
    pub open: Option<usize>,
}

impl Prompt {
    /// A prompt whose pending state is the text its field was opened with.
    pub fn new(
        title: &str,
        label: &str,
        input: TextEditor,
        choices: Vec<(String, Vec<String>, usize)>,
        action: PromptAction,
    ) -> Self {
        let initial = input.text();
        Self {
            title: title.to_string(),
            label: label.to_string(),
            input,
            initial,
            choices,
            focus: 0,
            action,
            error: None,
            note: None,
            open: None,
        }
    }

    /// The field holds text typed after the dialog opened (closing would drop it).
    pub fn pending(&self) -> bool {
        self.input.text() != self.initial
    }
}

pub struct Picker {
    pub title: String,
    pub items: Vec<String>,
    /// Secondary text shown dimmed after each item.
    pub details: Vec<String>,
    pub filter: TextEditor,
    pub sel: usize,
    pub scroll: usize,
    pub action: PickAction,
}

impl Picker {
    pub fn new(
        title: &str,
        items: Vec<String>,
        details: Vec<String>,
        action: PickAction,
        current: Option<usize>,
    ) -> Self {
        Self {
            title: title.to_string(),
            items,
            details,
            filter: TextEditor::single("").with_placeholder("type to filter"),
            sel: current.unwrap_or(0),
            scroll: 0,
            action,
        }
    }

    /// Indices of items matching the filter.
    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.text().to_lowercase();
        self.items
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                f.is_empty()
                    || fuzzy(&s.to_lowercase(), &f)
                    || self
                        .details
                        .get(*i)
                        .is_some_and(|d| fuzzy(&d.to_lowercase(), &f))
            })
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected(&self) -> Option<usize> {
        self.visible().get(self.sel).copied()
    }
}

/// Subsequence match: all query characters appear in order.
pub fn fuzzy(hay: &str, q: &str) -> bool {
    if hay.contains(q) {
        return true;
    }
    let mut it = hay.chars();
    q.chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| it.any(|h| h == c))
}

pub struct Confirm {
    pub title: String,
    pub message: String,
    pub buttons: Vec<String>,
    pub sel: usize,
    pub action: ConfirmAction,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaletteItem {
    Action(crate::keymap::Action),
    Open(String),
    Env(Option<String>),
    Command(&'static str, &'static str),
    /// Opens the settings dialog on this row.
    Setting(SettingId),
}

pub struct Palette {
    pub input: TextEditor,
    pub items: Vec<(PaletteItem, String, String)>,
    pub sel: usize,
    pub scroll: usize,
    /// A row is highlighted and Enter runs it. Always true for the app's
    /// palette (its untouched first row is harmless, see `visible`).
    pub armed: bool,
}

pub const COMMANDS: &[(&str, &str)] = &[
    (":send", "Send the current request"),
    (":cancel", "Cancel the running request"),
    (
        ":watch [seconds]",
        "Repeat the active HTTP request on an interval",
    ),
    (":end", "Finish sending on a client/bidi stream"),
    (
        ":env <name>",
        "Switch environment (names listed as you type)",
    ),
    (":envs", "Edit environments"),
    (":newenv", "Create an environment (asks for the name)"),
    (
        ":auth login [name]",
        "Sign in to an HTTP OAuth identity or an MCP server (lists them without a name)",
    ),
    (
        ":auth logout [name]",
        "Remove the OAuth tokens of an identity or an MCP server (lists them without a name)",
    ),
    (":w", "Save the request"),
    (":saveas <name>", "Save the request under a new name"),
    (":wq", "Save and quit"),
    (":q", "Quit (asks when there are unsaved changes)"),
    (":q!", "Quit without saving"),
    (":new", "New HTTP request (Ctrl+N)"),
    (":newgrpc", "New gRPC request (Ctrl+W g)"),
    (":newws", "New WebSocket request (Ctrl+W y)"),
    (":newmcp", "New MCP request"),
    (":mcpservers", "Choose an MCP server"),
    (
        ":mcpservers edit",
        "Edit the MCP servers in the project config (Ctrl+W S)",
    ),
    (
        ":mcpadd <name>",
        "Add an MCP server to the project config (asks for the URL or command)",
    ),
    (
        ":mcpedit [name]",
        "Edit the details of an MCP server: env, headers, TLS, OAuth (lists them without a name)",
    ),
    (":mcptools", "Discover and choose an MCP tool"),
    (":method", "Choose the HTTP method"),
    (":bodytype", "Choose the body type of the HTTP request"),
    (":authtype", "Choose the auth type of the request"),
    (":grpcmethod", "Choose the gRPC service and method"),
    (":rescan", "Read the collection from disk again"),
    (
        ":import [path]",
        "Import a Postman/Insomnia file or Bruno directory, or paste curl",
    ),
    (
        ":curl",
        "Copy the request as curl (secret variables stay {{placeholders}})",
    ),
    (":curl!", "Copy the request as curl including secret values"),
    (":httpfile", "Copy the request as an .http file"),
    (":httpfile!", "Copy the .http file including secret values"),
    (
        ":cookies",
        "Manage the active environment's cookies: delete one or clear all (values masked)",
    ),
    (":cookies clear", "Clear the active environment's cookies"),
    (
        ":cookies delete <number>",
        "Delete one cookie from the active environment",
    ),
    (":history", "Show history"),
    (":clearhistory", "Delete all history entries"),
    (":format", "Pretty-print the JSON body"),
    (
        ":jq [expr]",
        "Filter the response body with a jq expression",
    ),
    (
        ":template",
        "Fill the gRPC message with a template from the schema",
    ),
    (":reload", "Reload the gRPC schema"),
    (":settings", "Open settings"),
    (
        ":oauth",
        "Edit the HTTP OAuth identities (the file Settings saves to)",
    ),
    (
        ":keymap",
        "Edit your key bindings (the file Settings saves to)",
    ),
    (
        ":theme [name]",
        "Switch theme (lists them without a name); :theme reload reads your theme files again",
    ),
    (":help", "Show keys"),
    (
        ":report",
        "Show the last report (import, curl export) again",
    ),
    (":savebody [path]", "Save the response body to a file"),
];

/// A palette command without its arguments (`":auth login [name]"` →
/// `"auth login"`) and whether it needs an argument before it can run.
pub fn command_base(c: &str) -> (&str, bool) {
    let c = c.trim_start_matches(':');
    let end = c.find(['<', '[']).unwrap_or(c.len());
    (c[..end].trim(), c.contains('<'))
}

impl Palette {
    pub fn visible(&self) -> Vec<usize> {
        let q = self.input.text().to_lowercase();
        let q = q.trim();
        if let Some(cmd) = q.strip_prefix(':') {
            // ":env" and ":env <prefix>" list the matching environments, real
            // ones first; "no environment" comes last.
            if cmd == "env" || cmd.starts_with("env ") {
                let arg = cmd[3..].trim();
                let mut v: Vec<usize> = self
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, (it, _, _))| {
                        matches!(it, PaletteItem::Env(Some(name)) if name.to_lowercase().starts_with(arg))
                    })
                    .map(|(i, _)| i)
                    .collect();
                if arg.is_empty() {
                    v.extend(
                        self.items
                            .iter()
                            .position(|(it, _, _)| *it == PaletteItem::Env(None)),
                    );
                }
                return v;
            }
            let word = cmd.split_whitespace().next().unwrap_or("");
            return self
                .items
                .iter()
                .enumerate()
                .filter(|(_, (it, label, _))| {
                    matches!(it, PaletteItem::Command(..)) && label[1..].starts_with(word)
                })
                .map(|(i, _)| i)
                .collect();
        }
        let mut v: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, (_, label, detail))| {
                q.is_empty() || fuzzy(&label.to_lowercase(), q) || fuzzy(&detail.to_lowercase(), q)
            })
            .map(|(i, _)| i)
            .collect();
        // Rows that hold the typed text as written come before the scattered
        // matches: "rescan" finds :rescan before "Re-send … on an interval".
        v.sort_by_key(|i| {
            let (_, label, detail) = &self.items[*i];
            !(label.to_lowercase().contains(q) || detail.to_lowercase().contains(q))
        });
        // The untouched palette highlights its first row, so that row must be
        // harmless: the key list, never an action that sends or deletes.
        if q.is_empty()
            && let Some(pos) = v
                .iter()
                .position(|i| self.items[*i].0 == PaletteItem::Action(crate::keymap::Action::Help))
        {
            let help = v.remove(pos);
            v.insert(0, help);
        }
        v
    }
}

/// The config file the settings dialog and the OAuth and keymap editors
/// write to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfigScope {
    /// `~/.config/sendito/config.toml`: every collection.
    #[default]
    Global,
    /// `<collection>/.sendito/config.toml`: this collection; it wins over
    /// the global file.
    Project,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingId {
    Scope,
    Vim,
    Theme,
    Icons,
    Sidebar,
    Mouse,
    ConfirmQuit,
    StickyFolders,
    History,
    HistoryMax,
    WatchInterval,
    StreamKept,
    Hints,
    HttpTimeout,
    GrpcDeadline,
    WsTimeout,
    McpTimeout,
    MaxBody,
    CookieJar,
    Proxy,
    TlsCa,
    TlsCert,
    TlsKey,
    CollectionName,
    CollectionIgnore,
    OAuthIds,
    KeyBindings,
}

impl SettingId {
    /// Where the setting is saved: (dotted section, key). The scope is not
    /// saved.
    pub fn key(self) -> Option<(&'static str, &'static str)> {
        use SettingId::*;
        Some(match self {
            Scope | OAuthIds | KeyBindings => return None,
            Vim => ("ui", "vim_mode"),
            Theme => ("ui", "theme"),
            Icons => ("ui", "icons"),
            Sidebar => ("ui", "sidebar"),
            Mouse => ("ui", "mouse"),
            ConfirmQuit => ("ui", "confirm_quit"),
            StickyFolders => ("ui", "sticky_folders"),
            History => ("history", "enabled"),
            HistoryMax => ("history", "max_entries"),
            WatchInterval => ("ui", "watch_interval_s"),
            StreamKept => ("ui", "stream_messages_kept"),
            Hints => ("ui", "hints"),
            HttpTimeout => ("http", "timeout_ms"),
            GrpcDeadline => ("grpc", "deadline_ms"),
            WsTimeout => ("websocket", "timeout_ms"),
            McpTimeout => ("mcp", "timeout_ms"),
            MaxBody => ("http", "max_body_mb"),
            CookieJar => ("http", "cookie_jar"),
            Proxy => ("http", "proxy"),
            TlsCa => ("http.tls", "ca"),
            TlsCert => ("http.tls", "client_cert"),
            TlsKey => ("http.tls", "client_key"),
            CollectionName => ("collection", "name"),
            CollectionIgnore => ("collection", "ignore"),
        })
    }

    /// The value is typed in a dialog (numbers, paths, text) instead of
    /// changed with ← →.
    pub fn typed(self) -> bool {
        use SettingId::*;
        matches!(
            self,
            HistoryMax
                | WatchInterval
                | StreamKept
                | HttpTimeout
                | GrpcDeadline
                | WsTimeout
                | McpTimeout
                | MaxBody
                | Proxy
                | TlsCa
                | TlsCert
                | TlsKey
                | CollectionName
                | CollectionIgnore
        )
    }

    /// Enter opens an editor of its own (the value is a table).
    pub fn opens(self) -> bool {
        matches!(self, SettingId::OAuthIds | SettingId::KeyBindings)
    }

    /// The smallest number a numeric setting takes, `None` for text.
    pub fn min(self) -> Option<u64> {
        use SettingId::*;
        match self {
            GrpcDeadline | WsTimeout => Some(0),
            HistoryMax | WatchInterval | StreamKept | HttpTimeout | McpTimeout | MaxBody => Some(1),
            _ => None,
        }
    }

    /// The heading drawn above this setting, for the first of a group.
    pub fn heading(self) -> Option<&'static str> {
        use SettingId::*;
        match self {
            Scope => Some("Saving"),
            Theme => Some("Appearance"),
            Vim => Some("Editing"),
            Sidebar => Some("Layout"),
            Mouse => Some("Behavior"),
            HttpTimeout => Some("Timeouts"),
            MaxBody => Some("HTTP"),
            TlsCa => Some("TLS"),
            CollectionName => Some("Collection"),
            _ => None,
        }
    }
}

pub const SETTINGS: &[(SettingId, &str)] = &[
    (SettingId::Scope, "Save changes to"),
    (SettingId::Theme, "Theme"),
    (SettingId::Icons, "Icons"),
    (SettingId::Hints, "Key hints"),
    (SettingId::Vim, "Vim mode"),
    (SettingId::KeyBindings, "Key bindings"),
    (SettingId::Sidebar, "Sidebar position"),
    (SettingId::StickyFolders, "Pin folders while scrolling"),
    (SettingId::Mouse, "Mouse support"),
    (SettingId::ConfirmQuit, "Confirm quit with unsaved changes"),
    (SettingId::History, "Record history"),
    (SettingId::HistoryMax, "History entries kept"),
    (SettingId::WatchInterval, "Watch interval"),
    (SettingId::StreamKept, "Stream messages kept"),
    (SettingId::HttpTimeout, "HTTP request timeout"),
    (SettingId::GrpcDeadline, "gRPC deadline"),
    (SettingId::WsTimeout, "WebSocket handshake timeout"),
    (SettingId::McpTimeout, "MCP call timeout"),
    (SettingId::MaxBody, "Largest response body"),
    (SettingId::CookieJar, "Cookie jar"),
    (SettingId::Proxy, "Proxy"),
    (SettingId::OAuthIds, "OAuth identities"),
    (SettingId::TlsCa, "CA certificates (PEM)"),
    (SettingId::TlsCert, "Client certificate (PEM)"),
    (SettingId::TlsKey, "Client key (PEM)"),
    (SettingId::CollectionName, "Collection name"),
    (SettingId::CollectionIgnore, "Skipped directories"),
];

/// Source labels in the editor, in the order of the file's `source` values
/// (plain, "local", "keychain", "file"). "local" reads as "secrets" here so it
/// is not confused with an environment named "local".
pub const SOURCE_NAMES: &[&str] = &["plain", "secrets", "keychain", "file"];

pub struct EnvEditor {
    /// Environment file names; index 0 is the collection-level env.
    pub envs: Vec<String>,
    pub current: usize,
    pub table: Table,
    pub dirty: bool,
    pub error: Option<String>,
}

impl EnvEditor {
    pub fn table() -> Table {
        let mut t = Table::new(
            vec![
                Column {
                    title: "Variable",
                    kind: ColKind::Text,
                    weight: 2,
                },
                Column {
                    title: "Source",
                    kind: ColKind::Choice(SOURCE_NAMES),
                    weight: 1,
                },
                Column {
                    title: "Value, key or path",
                    kind: ColKind::Text,
                    weight: 3,
                },
                Column {
                    title: "Secret value",
                    kind: ColKind::Text,
                    weight: 2,
                },
            ],
            false,
        );
        // Rows added through the trailing "add" row must mask secrets too.
        t.masked_cols = vec![3];
        t
    }

    /// Loads `env`; `is_set` tells whether a secret already has a value
    /// (`None` when unknown, e.g. the keychain).
    pub fn load(
        &mut self,
        env: &sendito_store::env::EnvFile,
        is_set: impl Fn(&sendito_store::env::EnvVar) -> Option<bool>,
    ) {
        use sendito_store::env::VarSource;
        let mut t = Self::table();
        for v in &env.vars {
            let (src, value) = match &v.source {
                VarSource::Plain(s) => (0, s.clone()),
                VarSource::Local { key } => (1, key.clone().unwrap_or_default()),
                VarSource::Keychain { key } => (2, key.clone().unwrap_or_default()),
                VarSource::File { path } => (3, path.clone()),
            };
            let hint = match (src, is_set(v)) {
                (1 | 2, Some(true)) => "set",
                (1 | 2, Some(false)) => "NOT SET",
                (1 | 2, None) => "type to set",
                (3, Some(false)) => "file not found",
                (3, _) => "from the file",
                _ => "",
            };
            let mut secret = TextEditor::single("").with_placeholder(hint);
            secret.mask = true;
            t.push(
                true,
                vec![
                    Cell::Text(TextEditor::single(&v.name)),
                    Cell::Choice(src),
                    Cell::Text(TextEditor::single(&value)),
                    Cell::Text(secret),
                ],
            );
        }
        self.table = t;
        self.dirty = false;
        self.error = None;
    }

    /// The environment as edited, plus secret values to store.
    pub fn to_env(&self, name: &str) -> (sendito_store::env::EnvFile, Vec<(String, String)>) {
        use sendito_store::env::{EnvFile, EnvVar, VarSource};
        let mut env = EnvFile::new(name);
        let mut secrets = Vec::new();
        for r in self.table.values() {
            let var = r.cells[0].text().trim().to_string();
            if var.is_empty() {
                continue;
            }
            let value = r.cells[2].text();
            let opt = |s: String| {
                if s.trim().is_empty() {
                    None
                } else {
                    Some(s.trim().to_string())
                }
            };
            let source = match r.cells[1].choice() {
                1 => VarSource::Local { key: opt(value) },
                2 => VarSource::Keychain { key: opt(value) },
                3 => VarSource::File { path: value },
                _ => VarSource::Plain(value),
            };
            let secret = r.cells[3].text();
            if !secret.is_empty()
                && matches!(source, VarSource::Local { .. } | VarSource::Keychain { .. })
            {
                secrets.push((var.clone(), secret));
            }
            env.set(EnvVar { name: var, source });
        }
        (env, secrets)
    }
}

/// The MCP servers from `[mcp.servers]`, edited as a table: a name, the kind
/// (stdio or http), the command or URL, and the auth of an HTTP server (none,
/// bearer token or OAuth). Extra settings (args, env, headers, OAuth fields)
/// are kept as they are in the file and are edited there.
pub struct McpEditor {
    pub table: Table,
    pub dirty: bool,
    pub error: Option<String>,
}

pub const MCP_KINDS: &[&str] = &["http", "stdio"];
pub const MCP_AUTHS: &[&str] = &["none", "bearer", "oauth"];

/// One row of the MCP servers editor.
pub struct McpRow {
    pub name: String,
    /// Index in `MCP_KINDS`.
    pub kind: usize,
    pub value: String,
    /// Index in `MCP_AUTHS`.
    pub auth: usize,
    pub token: String,
}

impl McpEditor {
    pub fn table() -> Table {
        Table::new(
            vec![
                Column {
                    title: "Name",
                    kind: ColKind::Text,
                    weight: 2,
                },
                Column {
                    title: "Kind",
                    kind: ColKind::Choice(MCP_KINDS),
                    weight: 1,
                },
                Column {
                    title: "URL or command",
                    kind: ColKind::Text,
                    weight: 5,
                },
                Column {
                    title: "Auth",
                    kind: ColKind::Choice(MCP_AUTHS),
                    weight: 1,
                },
                Column {
                    title: "Token",
                    kind: ColKind::Text,
                    weight: 2,
                },
            ],
            false,
        )
    }

    /// Loads `servers` into the table.
    pub fn load(
        &mut self,
        servers: &std::collections::BTreeMap<String, sendito_store::config::McpServer>,
    ) {
        use sendito_store::config::{McpServer, McpServerAuth};
        let mut t = Self::table();
        for (name, s) in servers {
            let (kind, value, auth, token) = match s {
                McpServer::Http(h) => {
                    let (auth, token) = match &h.auth {
                        None => (0, ""),
                        Some(McpServerAuth::Bearer { token }) => (1, token.as_str()),
                        Some(McpServerAuth::Oauth { .. }) => (2, ""),
                    };
                    (0, h.url.clone(), auth, token.to_string())
                }
                // stdio rows read as the command line they start: command and
                // arguments together (env stays in the file).
                McpServer::Stdio(s) => {
                    (1, stdio_command_line(&s.command, &s.args), 0, String::new())
                }
            };
            t.push(
                true,
                vec![
                    Cell::Text(TextEditor::single(name)),
                    Cell::Choice(kind),
                    Cell::Text(TextEditor::single(&value)),
                    Cell::Choice(auth),
                    Cell::Text(TextEditor::single(&token)),
                ],
            );
        }
        self.table = t;
        self.dirty = false;
        self.error = None;
    }

    /// The rows, skipping empty and duplicate names.
    pub fn rows(&self) -> Vec<McpRow> {
        let mut out: Vec<McpRow> = Vec::new();
        for r in self.table.values() {
            let name = r.cells[0].text().trim().to_string();
            if name.is_empty() || out.iter().any(|row| row.name == name) {
                continue;
            }
            out.push(McpRow {
                name,
                kind: r.cells[1].choice(),
                value: r.cells[2].text().trim().to_string(),
                auth: r.cells[3].choice(),
                token: r.cells[4].text().trim().to_string(),
            });
        }
        out
    }
}

/// A stdio server as one line: the command, then its arguments, quoted only
/// when they hold spaces (so the line splits back into the same parts).
pub fn stdio_command_line(command: &str, args: &[String]) -> String {
    let mut parts = vec![quote_arg(command)];
    parts.extend(args.iter().map(|a| quote_arg(a)));
    parts.join(" ")
}

/// Splits a stdio command line on spaces, honoring single and double quotes.
pub fn split_command_line(line: &str) -> (String, Vec<String>) {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut has = false;
    for c in line.chars() {
        match quote {
            Some(q) if c == q => quote = None,
            Some(_) => cur.push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                has = true;
            }
            None if c.is_whitespace() => {
                if has || !cur.is_empty() {
                    parts.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            None => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        parts.push(cur);
    }
    let mut it = parts.into_iter();
    let command = it.next().unwrap_or_default();
    (command, it.collect())
}

fn quote_arg(s: &str) -> String {
    if s.is_empty() {
        "''".to_string()
    } else if s.chars().any(char::is_whitespace) {
        format!("\"{s}\"")
    } else {
        s.to_string()
    }
}

pub struct Report {
    pub title: String,
    pub viewer: Viewer,
    /// Text copied with `y`.
    pub copy_text: String,
    /// Already on the clipboard (the button then reads "Copy again").
    pub copied: bool,
    /// Status line message left when the dialog closes (what was done).
    pub close_status: Option<String>,
}

pub struct Import {
    pub input: TextEditor,
    /// The field's text when the dialog opened: a different text is pending.
    pub initial: String,
    pub error: Option<String>,
}

impl Import {
    /// An import dialog whose pending state is the text its field was opened with.
    pub fn new(input: TextEditor) -> Self {
        let initial = input.text();
        Self {
            input,
            initial,
            error: None,
        }
    }

    /// The field holds text typed after the dialog opened (closing would drop it).
    pub fn pending(&self) -> bool {
        self.input.text() != self.initial
    }
}

pub enum Overlay {
    Palette(Palette),
    Help { viewer: Viewer, filter: TextEditor },
    Settings { sel: usize },
    Confirm(Confirm),
    Prompt(Prompt),
    Picker(Picker),
    Env(EnvEditor),
    Mcp(McpEditor),
    Cfg(crate::config_editor::CfgEditor),
    Import(Import),
    Report(Report),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_and_picker() {
        assert!(fuzzy("users/list users", "lsu"));
        assert!(!fuzzy("abc", "abd"));
        let mut p = Picker::new(
            "t",
            vec!["dev".into(), "prod".into()],
            vec![],
            PickAction::Env,
            Some(1),
        );
        assert_eq!(p.selected(), Some(1));
        p.filter.set_text("pr");
        p.sel = 0;
        assert_eq!(p.selected(), Some(1));
    }

    #[test]
    fn env_editor_roundtrip() {
        use sendito_store::env::{EnvFile, EnvVar, VarSource};
        let mut env = EnvFile::new("dev");
        env.set(EnvVar {
            name: "a".into(),
            source: VarSource::Plain("1".into()),
        });
        env.set(EnvVar {
            name: "b".into(),
            source: VarSource::Local { key: None },
        });
        env.set(EnvVar {
            name: "c".into(),
            source: VarSource::Keychain {
                key: Some("k".into()),
            },
        });
        env.set(EnvVar {
            name: "d".into(),
            source: VarSource::File { path: "f".into() },
        });
        let mut ed = EnvEditor {
            envs: vec![],
            current: 0,
            table: EnvEditor::table(),
            dirty: false,
            error: None,
        };
        ed.load(&env, |_| None);
        if let Cell::Text(e) = &mut ed.table.rows[1].cells[3] {
            e.set_text("s3cret");
        }
        let (back, secrets) = ed.to_env("dev");
        assert_eq!(back, env);
        assert_eq!(secrets, vec![("b".to_string(), "s3cret".to_string())]);
    }

    #[test]
    fn env_command_lists_real_environments_first() {
        let mut items: Vec<(PaletteItem, String, String)> = COMMANDS
            .iter()
            .map(|(c, d)| (PaletteItem::Command(c, d), c.to_string(), d.to_string()))
            .collect();
        items.push((
            PaletteItem::Env(None),
            "No environment".into(),
            String::new(),
        ));
        items.push((
            PaletteItem::Env(Some("local".into())),
            "local".into(),
            String::new(),
        ));
        let p = Palette {
            input: TextEditor::single(":env"),
            items,
            sel: 0,
            scroll: 0,
            armed: true,
        };
        let v: Vec<PaletteItem> = p.visible().iter().map(|i| p.items[*i].0.clone()).collect();
        assert_eq!(
            v,
            vec![
                PaletteItem::Env(Some("local".into())),
                PaletteItem::Env(None)
            ]
        );
    }

    #[test]
    fn palette_filters() {
        let p = Palette {
            input: TextEditor::single(":en"),
            items: COMMANDS
                .iter()
                .map(|(c, d)| (PaletteItem::Command(c, d), c.to_string(), d.to_string()))
                .collect(),
            sel: 0,
            scroll: 0,
            armed: true,
        };
        let v = p.visible();
        assert!(v.len() >= 2);
    }

    #[test]
    fn commands_match_without_colon() {
        let mut items: Vec<(PaletteItem, String, String)> = vec![(
            PaletteItem::Action(crate::keymap::Action::Help),
            "Show all keys and actions".into(),
            "F1".into(),
        )];
        items.extend(
            COMMANDS
                .iter()
                .map(|(c, d)| (PaletteItem::Command(c, d), c.to_string(), d.to_string())),
        );
        let mut p = Palette {
            input: TextEditor::single("cookie"),
            items,
            sel: 0,
            scroll: 0,
            armed: true,
        };
        let v: Vec<&str> = p.visible().iter().map(|i| p.items[*i].1.as_str()).collect();
        assert!(v.contains(&":cookies"), "{v:?}");
        // The untouched palette still starts with the key list.
        p.input = TextEditor::single("");
        assert_eq!(p.visible().first(), Some(&0));
    }

    #[test]
    fn command_words() {
        assert_eq!(command_base(":saveas <name>"), ("saveas", true));
        assert_eq!(command_base(":theme [name]"), ("theme", false));
        assert_eq!(command_base(":auth login [name]"), ("auth login", false));
        assert_eq!(
            command_base(":cookies delete <number>"),
            ("cookies delete", true)
        );
        assert_eq!(command_base(":send"), ("send", false));
    }
}
