//! The settings overlay: setting ids, scopes and the setting list.

/// The config file the settings dialog and the OAuth and keymap editors
/// write to.
/// The config file the settings write to; kept in `session.toml`.
pub use sendito_store::session::ConfigScope;

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
    HidePlainSecrets,
    HttpTimeout,
    GrpcDeadline,
    WsTimeout,
    SseTimeout,
    MqttTimeout,
    McpTimeout,
    McpServers,
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
            Scope | OAuthIds | KeyBindings | McpServers => return None,
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
            HidePlainSecrets => ("ui", "hide_plain_secrets"),
            HttpTimeout => ("http", "timeout_ms"),
            GrpcDeadline => ("grpc", "deadline_ms"),
            WsTimeout => ("websocket", "timeout_ms"),
            SseTimeout => ("sse", "timeout_ms"),
            MqttTimeout => ("mqtt", "timeout_ms"),
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
                | SseTimeout
                | MqttTimeout
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
        matches!(
            self,
            SettingId::OAuthIds | SettingId::KeyBindings | SettingId::McpServers
        )
    }

    /// The value is a duration in milliseconds, shown and typed as "30 s".
    pub fn duration(self) -> bool {
        use SettingId::*;
        matches!(
            self,
            HttpTimeout | GrpcDeadline | WsTimeout | SseTimeout | MqttTimeout | McpTimeout
        )
    }

    /// The smallest number a numeric setting takes, `None` for text.
    pub fn min(self) -> Option<u64> {
        use SettingId::*;
        match self {
            GrpcDeadline | WsTimeout | MqttTimeout => Some(0),
            HistoryMax | WatchInterval | StreamKept | HttpTimeout | SseTimeout | McpTimeout
            | MaxBody => Some(1),
            _ => None,
        }
    }

    /// The heading drawn above this setting, for the first of a group.
    pub fn heading(self) -> Option<&'static str> {
        use SettingId::*;
        match self {
            Theme => Some("Appearance"),
            Vim => Some("Editing"),
            Sidebar => Some("Layout"),
            Mouse => Some("Behavior"),
            HttpTimeout => Some("Timeouts"),
            MaxBody => Some("HTTP"),
            TlsCa => Some("TLS"),
            McpServers => Some("MCP"),
            CollectionName => Some("Collection"),
            _ => None,
        }
    }
}

/// The settings dialog: the selected row, the filter, the value being
/// typed (number, path and text settings are edited in place) and why it
/// was refused.
pub struct SettingsView {
    /// The selected setting, as an index into `SETTINGS`.
    pub sel: usize,
    pub filter: crate::editor::TextEditor,
    pub edit: Option<crate::editor::TextEditor>,
    pub error: Option<String>,
}

impl SettingsView {
    pub fn new(sel: usize) -> Self {
        Self {
            sel,
            filter: crate::editor::TextEditor::single("")
                .with_placeholder("type to filter settings"),
            edit: None,
            error: None,
        }
    }

    /// The settings that match the filter (label or config key), as
    /// indices into `SETTINGS`.
    pub fn visible(&self) -> Vec<usize> {
        let f = self.filter.text().to_lowercase();
        SETTINGS
            .iter()
            .enumerate()
            .filter(|(_, (id, label))| {
                f.is_empty()
                    || label.to_lowercase().contains(&f)
                    || id
                        .key()
                        .is_some_and(|(s, k)| format!("{s}.{k}").contains(&f))
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Moves the selection by `d` among the visible settings.
    pub fn step(&mut self, d: isize) {
        let vis = self.visible();
        let at = vis.iter().position(|i| *i == self.sel).unwrap_or(0);
        if let Some(i) = vis.get(crate::list::step(at, d, vis.len())) {
            self.sel = *i;
        }
    }

    /// Starts typing the selected setting's value, `raw` selected.
    pub fn start_edit(&mut self, raw: String) {
        let mut ed = crate::editor::TextEditor::single(&raw);
        ed.select_all();
        self.edit = Some(ed);
        self.error = None;
    }

    /// Keeps the selection on a visible setting after the filter changed.
    pub fn clamp(&mut self) {
        let vis = self.visible();
        if !vis.contains(&self.sel)
            && let Some(i) = vis.first()
        {
            self.sel = *i;
        }
    }
}

pub const SETTINGS: &[(SettingId, &str)] = &[
    (SettingId::Theme, "Theme"),
    (SettingId::Icons, "Icons"),
    (SettingId::Hints, "Key hints"),
    (SettingId::HidePlainSecrets, "Hide plain secrets"),
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
    (SettingId::SseTimeout, "SSE opening timeout"),
    (SettingId::MqttTimeout, "MQTT timeout"),
    (SettingId::McpTimeout, "MCP call timeout"),
    (SettingId::McpServers, "MCP servers"),
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
