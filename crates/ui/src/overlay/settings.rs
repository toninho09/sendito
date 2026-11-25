//! The settings overlay: setting ids, scopes and the setting list.

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
    SseTimeout,
    MqttTimeout,
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
        matches!(self, SettingId::OAuthIds | SettingId::KeyBindings)
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
    (SettingId::SseTimeout, "SSE opening timeout"),
    (SettingId::MqttTimeout, "MQTT timeout"),
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
