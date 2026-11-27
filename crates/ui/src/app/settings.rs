//! Settings: values, parsing, saving and themes.

use super::*;
use crate::units::format_ms;

/// What a typed setting does and what it takes: the settings dialog shows it
/// for the selected row, and the dialog that types the value shows it too.
pub fn setting_note(id: SettingId) -> &'static str {
    match id {
        SettingId::HistoryMax => "History keeps this many entries; the oldest go first.",
        SettingId::HistoryBody => {
            "History keeps the first this many KB of each HTTP response body. 0 keeps no response."
        }
        SettingId::WatchInterval => {
            "The :watch command re-sends the active request every this many seconds."
        }
        SettingId::StreamKept => {
            "A gRPC, WebSocket or SSE stream keeps this many messages; the oldest go first."
        }
        SettingId::HttpTimeout => {
            "How long an HTTP request may take when it sets no timeout of its own, e.g. 30 s or 500 ms."
        }
        SettingId::GrpcDeadline => {
            "How long a gRPC call may take when it sets no deadline of its own, e.g. 10 s; 0 = no deadline."
        }
        SettingId::SseTimeout => {
            "How long an SSE stream may take to open, e.g. 30 s; the events have no total timeout."
        }
        SettingId::MqttTimeout => {
            "How long to wait for the broker's CONNACK when a request sets none, e.g. 30 s; 0 = no limit."
        }
        SettingId::WsTimeout => {
            "How long a WebSocket handshake may take when the request sets none, e.g. 10 s; 0 = no limit."
        }
        SettingId::McpTimeout => {
            "How long an MCP call (handshake included) may take when the request sets none, e.g. 30 s."
        }
        SettingId::McpIdle => {
            "A server session stays open between calls and closes after this many idle seconds; 0 = a new session per call."
        }
        SettingId::McpServers => {
            "The servers of [mcp.servers] in the file chosen above, as Ctrl+W S shows them."
        }
        SettingId::MaxBody => "Response bodies larger than this many MB are cut (a note says so).",
        SettingId::Proxy => {
            "env (read HTTPS_PROXY, HTTP_PROXY, NO_PROXY) or a URL: http://, https://, socks5://host:port. Empty = the client default. gRPC does not use it."
        }
        SettingId::TlsCa => {
            "PEM file whose certificates replace the system roots. Relative to the collection; ~/ is your home. Empty = system roots."
        }
        SettingId::TlsCert => {
            "PEM file with the client certificate chain; set the client key too. Relative to the collection; ~/ is your home."
        }
        SettingId::TlsKey => {
            "PEM file with the client private key; set the client certificate too. Relative to the collection; ~/ is your home."
        }
        SettingId::CollectionName => {
            "Saved to the project config. Keychain secrets are stored under this name: after a change, set them again. Empty = the folder name."
        }
        SettingId::OAuthIds => {
            "Named OAuth identities that HTTP requests get a token with (authorization code, client credentials, password or device code). Enter opens the editor."
        }
        SettingId::KeyBindings => {
            "Your own keys for actions, replacing the default ones. Enter opens the editor."
        }
        SettingId::CollectionIgnore => {
            "Directory names the request tree skips, separated by commas. Empty = the defaults (target, node_modules, vendor, dist, build)."
        }
        _ => "",
    }
}

impl App {
    // ----- settings -----------------------------------------------------

    /// Opens the settings dialog with `id` selected.
    pub fn open_settings(&mut self, id: SettingId) {
        let sel = SETTINGS.iter().position(|(s, _)| *s == id).unwrap_or(0);
        self.overlay = Some(Overlay::Settings(SettingsView::new(sel)));
    }

    pub fn setting_value(&self, id: SettingId) -> String {
        let c = &self.project.config;
        let on = |b: bool| if b { "On" } else { "Off" }.to_string();
        let raw = self.setting_raw(id);
        match id {
            SettingId::Scope => match self.config_scope {
                ConfigScope::Global => "global".into(),
                ConfigScope::Project => "project".into(),
            },
            SettingId::Vim => on(self.vim),
            SettingId::Theme => c.ui.theme.clone(),
            SettingId::Icons => c.ui.icons.clone(),
            SettingId::Sidebar => c.ui.sidebar.clone(),
            SettingId::Mouse => on(c.ui.mouse),
            SettingId::ConfirmQuit => on(c.ui.confirm_quit),
            SettingId::StickyFolders => on(c.ui.sticky_folders),
            SettingId::History => on(c.history.enabled),
            SettingId::CookieJar => on(c.http.cookie_jar),
            SettingId::Hints => c.ui.hints.clone(),
            SettingId::WatchInterval => format!("{raw} s"),
            SettingId::McpIdle if raw == "0" => "none".into(),
            SettingId::McpIdle => format!("{raw} s"),
            SettingId::MaxBody => format!("{raw} MB"),
            SettingId::HistoryBody if raw == "0" => "none".into(),
            SettingId::HistoryBody => format!("{raw} KB"),
            SettingId::GrpcDeadline | SettingId::WsTimeout | SettingId::MqttTimeout
                if raw == "0 ms" =>
            {
                "none".into()
            }
            SettingId::HidePlainSecrets => on(c.ui.hide_plain_secrets),
            SettingId::CollectionName if raw.is_empty() => {
                format!("{} (folder name)", self.project.name)
            }
            SettingId::CollectionIgnore if raw.is_empty() => "none".into(),
            SettingId::OAuthIds => match c.oauth.len() {
                1 => "1 identity".into(),
                n => format!("{n} identities"),
            },
            // What Enter opens: the scope's file, not the merged servers.
            SettingId::McpServers => match self.scope_mcp_servers() {
                Ok(s) => match s.len() {
                    1 => "1 server".into(),
                    n => format!("{n} servers"),
                },
                Err(_) => "not readable".into(),
            },
            SettingId::KeyBindings => match c.keymap.values().map(BTreeMap::len).sum::<usize>() {
                1 => "1 change".into(),
                n => format!("{n} changes"),
            },
            _ if raw.is_empty() => "not set".into(),
            _ => raw,
        }
    }

    /// A typed setting's value as its dialog shows it (no unit; empty when
    /// it is not set).
    pub fn setting_raw(&self, id: SettingId) -> String {
        let c = &self.project.config;
        let opt = |v: &Option<String>| v.clone().unwrap_or_default();
        match id {
            SettingId::HistoryMax => c.history.max_entries.to_string(),
            SettingId::HistoryBody => c.history.max_body_kb.to_string(),
            SettingId::WatchInterval => c.ui.watch_interval_s.to_string(),
            SettingId::StreamKept => c.ui.stream_messages_kept.to_string(),
            SettingId::HttpTimeout => format_ms(c.http.timeout_ms),
            SettingId::GrpcDeadline => format_ms(c.grpc.deadline_ms),
            SettingId::WsTimeout => format_ms(c.websocket.timeout_ms),
            SettingId::SseTimeout => format_ms(c.sse.timeout_ms),
            SettingId::MqttTimeout => format_ms(c.mqtt.timeout_ms),
            SettingId::McpTimeout => format_ms(c.mcp.timeout_ms),
            SettingId::McpIdle => c.mcp.idle_timeout_s.to_string(),
            SettingId::MaxBody => c.http.max_body_mb.to_string(),
            SettingId::Proxy => opt(&c.http.proxy),
            SettingId::TlsCa => opt(&c.http.tls.ca),
            SettingId::TlsCert => opt(&c.http.tls.client_cert),
            SettingId::TlsKey => opt(&c.http.tls.client_key),
            SettingId::CollectionName => opt(&c.collection.name),
            SettingId::CollectionIgnore => c.collection.ignore.join(", "),
            _ => String::new(),
        }
    }

    /// The config file that settings and the OAuth and keymap editors write
    /// to; `None` when there is no global file to write.
    pub fn scope_path(&self, scope: ConfigScope) -> Option<std::path::PathBuf> {
        match scope {
            ConfigScope::Global => self.project.global_config.clone(),
            ConfigScope::Project => Some(self.project_config_path()),
        }
    }

    /// The file a setting is saved to: the chosen scope, except the
    /// collection name, which only means something in the project file.
    pub fn setting_path(&self, id: SettingId) -> Option<std::path::PathBuf> {
        match id {
            SettingId::CollectionName => self.scope_path(ConfigScope::Project),
            _ => self.scope_path(self.config_scope),
        }
    }

    /// Writes a setting to its file (`None` removes the key). Says so when the
    /// project file sets the same key, since that value wins at the next start.
    /// Returns false when the write failed.
    pub(super) fn save_setting(&mut self, id: SettingId, value: Option<toml::Value>) -> bool {
        let (Some((section, key)), Some(path)) = (id.key(), self.setting_path(id)) else {
            return true;
        };
        if let Err(e) = sendito_store::config::set_value(&path, section, key, value) {
            self.set_status(format!("could not write settings: {e}"), StatusKind::Error);
            return false;
        }
        let project = self.project_config_path();
        if path != project && sendito_store::config::has_value(&project, section, key) {
            self.set_status(
                format!(
                    "saved to the global config, but the project config also sets \
                     {section}.{key} and wins when Sendito starts again: \
                     save to project instead"
                ),
                StatusKind::Warn,
            );
        }
        true
    }

    /// Reads a typed setting: the value to save (`None` removes the key) or
    /// why the text is not valid.
    pub(super) fn parse_setting(
        &self,
        id: SettingId,
        text: &str,
    ) -> Result<Option<toml::Value>, String> {
        let text = text.trim();
        if id.duration() {
            let min = id.min().unwrap_or(0);
            return match crate::units::parse_ms(text) {
                Some(n) if n >= min => i64::try_from(n)
                    .map(|n| Some(toml::Value::Integer(n)))
                    .map_err(|_| "the time is too long".to_string()),
                _ if min == 0 => Err("type a time such as 30 s, 500 ms or 2 min (0 = none)".into()),
                _ => Err("type a time such as 30 s, 500 ms or 2 min".into()),
            };
        }
        if let Some(min) = id.min() {
            return match text.parse::<u64>() {
                Ok(n) if n >= min => i64::try_from(n)
                    .map(|n| Some(toml::Value::Integer(n)))
                    .map_err(|_| "the number is too large".to_string()),
                _ => Err(format!("type a whole number ({min} or more)")),
            };
        }
        if text.is_empty() {
            return Ok(None);
        }
        match id {
            SettingId::Proxy => {
                sendito_pipeline::http::ProxySpec::parse(text)?;
            }
            SettingId::TlsCa | SettingId::TlsCert | SettingId::TlsKey => {
                let p = sendito_store::env::expand(&self.project.root, text);
                if !p.is_file() {
                    return Err(format!("no file at {}", p.display()));
                }
            }
            SettingId::CollectionIgnore => {
                let dirs: Vec<toml::Value> = text
                    .split(',')
                    .map(str::trim)
                    .filter(|d| !d.is_empty())
                    .map(|d| toml::Value::String(d.to_string()))
                    .collect();
                return Ok(Some(toml::Value::Array(dirs)));
            }
            _ => {}
        }
        Ok(Some(toml::Value::String(text.to_string())))
    }

    /// Saves a typed setting and applies it at once.
    pub(crate) fn set_typed_setting(&mut self, id: SettingId, text: &str) -> Result<(), String> {
        let value = self.parse_setting(id, text)?;
        let num = value
            .as_ref()
            .and_then(toml::Value::as_integer)
            .unwrap_or(0) as u64;
        let s = value
            .as_ref()
            .and_then(toml::Value::as_str)
            .map(str::to_string);
        let c = &mut self.project.config;
        match id {
            SettingId::HistoryMax => c.history.max_entries = num as usize,
            SettingId::HistoryBody => c.history.max_body_kb = num as usize,
            SettingId::WatchInterval => c.ui.watch_interval_s = num,
            SettingId::StreamKept => c.ui.stream_messages_kept = num as usize,
            SettingId::HttpTimeout => c.http.timeout_ms = num,
            SettingId::GrpcDeadline => c.grpc.deadline_ms = num,
            SettingId::WsTimeout => c.websocket.timeout_ms = num,
            SettingId::SseTimeout => c.sse.timeout_ms = num,
            SettingId::MqttTimeout => c.mqtt.timeout_ms = num,
            SettingId::McpTimeout => c.mcp.timeout_ms = num,
            SettingId::McpIdle => {
                c.mcp.idle_timeout_s = num;
                self.mcp_pool.clear();
            }
            SettingId::MaxBody => c.http.max_body_mb = num,
            SettingId::Proxy => c.http.proxy = s,
            SettingId::TlsCa => c.http.tls.ca = s,
            SettingId::TlsCert => c.http.tls.client_cert = s,
            SettingId::TlsKey => c.http.tls.client_key = s,
            SettingId::CollectionName => c.collection.name = s,
            SettingId::CollectionIgnore => {
                c.collection.ignore = match &value {
                    Some(toml::Value::Array(a)) => a
                        .iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect(),
                    _ => sendito_store::config::CollectionConfig::default().ignore,
                }
            }
            _ => {}
        }
        let problems = self.project.apply_config();
        if matches!(id, SettingId::CollectionIgnore) {
            self.rescan();
        }
        self.clear_transient_status();
        if !self.save_setting(id, value) {
            return Ok(());
        }
        let mut notes = Vec::new();
        if id == SettingId::CollectionName {
            notes.push(format!(
                "collection name: {}; keychain secrets saved under the old name \
                 are not found until you set them again",
                self.project.name
            ));
        }
        notes.extend(problems);
        if !notes.is_empty() {
            self.set_status(notes.join("; "), StatusKind::Warn);
        }
        Ok(())
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
    pub(super) fn theme_command(&mut self, arg: &str) {
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
            if !self.save_setting(SettingId::Theme, Some(toml::Value::String(n.clone()))) {
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

    /// A click on a setting's value: types a number, path or text in
    /// place, changes the others.
    pub(super) fn click_setting(&mut self, v: &mut SettingsView, i: usize) {
        let id = SETTINGS[i].0;
        if id.typed() {
            v.start_edit(self.setting_raw(id));
        } else {
            self.change_setting(id, 1);
        }
    }

    pub fn change_setting(&mut self, id: SettingId, dir: isize) {
        if id.typed() {
            return;
        }
        match id {
            SettingId::OAuthIds => return self.open_oauth_editor(),
            SettingId::KeyBindings => return self.open_keymap_editor(),
            SettingId::McpServers => return self.open_mcp_editor(),
            _ => {}
        }
        let c = &mut self.project.config;
        let value = match id {
            SettingId::Scope => {
                self.config_scope = match self.config_scope {
                    ConfigScope::Global => ConfigScope::Project,
                    ConfigScope::Project => ConfigScope::Global,
                };
                // Kept in session.toml: the next start restores it.
                self.session_dirty = true;
                return;
            }
            SettingId::Vim => {
                self.vim = !self.vim;
                c.ui.vim_mode = self.vim;
                let vim = self.vim;
                for t in &mut self.tabs {
                    t.ed.set_vim(vim);
                }
                toml::Value::Boolean(vim)
            }
            SettingId::Theme => {
                let names = self.themes.names();
                let i = names.iter().position(|n| *n == c.ui.theme).unwrap_or(0);
                let next = names[crate::list::cycle(i, dir, names.len())].clone();
                self.apply_theme(&next);
                toml::Value::String(next)
            }
            SettingId::Icons => {
                let v = if self.icons.nerd { "unicode" } else { "nerd" };
                c.ui.icons = v.into();
                self.icons = crate::icons::Icons::by_name(v);
                toml::Value::String(v.into())
            }
            SettingId::Sidebar => {
                let v = if c.ui.sidebar == "right" {
                    "left"
                } else {
                    "right"
                };
                c.ui.sidebar = v.into();
                toml::Value::String(v.into())
            }
            SettingId::Mouse => {
                c.ui.mouse = !c.ui.mouse;
                toml::Value::Boolean(c.ui.mouse)
            }
            SettingId::ConfirmQuit => {
                c.ui.confirm_quit = !c.ui.confirm_quit;
                toml::Value::Boolean(c.ui.confirm_quit)
            }
            SettingId::StickyFolders => {
                c.ui.sticky_folders = !c.ui.sticky_folders;
                toml::Value::Boolean(c.ui.sticky_folders)
            }
            SettingId::HidePlainSecrets => {
                c.ui.hide_plain_secrets = !c.ui.hide_plain_secrets;
                toml::Value::Boolean(c.ui.hide_plain_secrets)
            }
            SettingId::History => {
                c.history.enabled = !c.history.enabled;
                toml::Value::Boolean(c.history.enabled)
            }
            SettingId::CookieJar => {
                c.http.cookie_jar = !c.http.cookie_jar;
                toml::Value::Boolean(c.http.cookie_jar)
            }
            SettingId::Hints => {
                const LEVELS: [&str; 3] = ["full", "short", "off"];
                let i = LEVELS.iter().position(|l| *l == c.ui.hints).unwrap_or(1);
                let next = LEVELS[crate::list::cycle(i, dir, LEVELS.len())];
                c.ui.hints = next.into();
                toml::Value::String(next.into())
            }
            _ => return,
        };
        self.clear_transient_status();
        self.save_setting(id, Some(value));
    }
}
