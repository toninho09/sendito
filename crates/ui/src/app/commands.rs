//! Pickers, the command palette, help and the `:command` handlers.

use super::*;

impl App {
    // ----- misc actions -------------------------------------------------

    pub(super) fn open_method_picker(&mut self, prefix: Option<char>) {
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

    pub(super) fn open_choice_picker(&mut self, f: Field) {
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
        for (id, label) in SETTINGS {
            items.push((
                PaletteItem::Setting(*id),
                format!("Setting: {label}"),
                self.setting_value(*id),
            ));
        }
        // Commands come last: the untouched list starts with the actions.
        for (c, d) in COMMANDS {
            items.push((PaletteItem::Command(c, d), c.to_string(), d.to_string()));
        }
        let mut input = TextEditor::single(prefill)
            .with_placeholder("type to search actions, settings, commands and requests");
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

    // ----- palette commands ---------------------------------------------

    /// `:cookies [clear | delete <number>]`. The jar is guarded by a
    /// cross-process lock, so the work runs off the UI thread and the result
    /// arrives as an [`AppMsg::Cookies`] message.
    pub(super) fn cookies_command(&mut self, arg: &str) {
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

    pub(super) fn http_oauth_command(&mut self, arg: &str) {
        let Some((action, name)) = arg.split_once(char::is_whitespace) else {
            self.set_status("usage: :auth login|logout <name>", StatusKind::Error);
            return;
        };
        let name = name.trim();
        if name.is_empty() || !matches!(action, "login" | "logout") {
            self.set_status("usage: :auth login|logout <name>", StatusKind::Error);
            return;
        }
        // An [oauth] identity wins over an MCP server with the same name.
        if !self.project.config.oauth.contains_key(name)
            && self.project.config.mcp.servers.contains_key(name)
        {
            self.mcp_oauth_command(name, action == "login", None);
            return;
        }
        if self.oauth_login.is_some() {
            self.set_status("an OAuth operation is already running", StatusKind::Warn);
            return;
        }
        let identity = self.project.config.oauth.get(name).cloned();
        if action == "login" && identity.is_none() {
            self.set_status(
                "not an OAuth identity of [oauth] or a server of [mcp.servers]",
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
        let network = self.project.oauth_network();
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
                let settings = runner::oauth_settings(&identity, &scope, network)?;
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
                    &runner::open_browser,
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

    /// True when the active tab is one of `kinds`; otherwise the status
    /// says that `what` is not available.
    pub(super) fn tab_kind_in(&mut self, kinds: &[Kind], what: &str) -> bool {
        let kind = self.tab().map(|t| t.ed.kind);
        match kind {
            Some(k) if kinds.contains(&k) => true,
            Some(k) => {
                let name = match k {
                    Kind::Http => "HTTP",
                    Kind::Grpc => "gRPC",
                    Kind::Ws => "WebSocket",
                    Kind::Mcp => "MCP",
                };
                self.set_status(
                    format!("{what} is not available for {name} requests"),
                    StatusKind::Warn,
                );
                false
            }
            None => {
                self.set_status("open a request first", StatusKind::Warn);
                false
            }
        }
    }

    /// The "New environment" name dialog.
    pub(super) fn prompt_new_env(&mut self) {
        self.overlay = Some(Overlay::Prompt(Prompt::new(
            "New environment",
            "Name",
            TextEditor::single(""),
            vec![],
            PromptAction::NewEnv,
        )));
    }

    /// Lists the themes; choosing one applies and saves it.
    pub(super) fn open_theme_picker(&mut self) {
        let items = self.themes.names();
        let cur = items
            .iter()
            .position(|n| *n == self.project.config.ui.theme);
        self.overlay = Some(Overlay::Picker(Picker::new(
            "Theme",
            items,
            vec![],
            PickAction::Theme,
            cur,
        )));
    }

    /// Lists the HTTP OAuth identities and the MCP servers that use OAuth.
    pub(super) fn open_auth_picker(&mut self, login: bool) {
        use sendito_store::config::{McpServer, McpServerAuth};
        let c = &self.project.config;
        let mut items: Vec<String> = Vec::new();
        let mut details: Vec<String> = Vec::new();
        for name in c.oauth.keys() {
            items.push(name.clone());
            details.push("HTTP identity".into());
        }
        for (name, s) in &c.mcp.servers {
            // An [oauth] identity wins over a server with the same name.
            if let McpServer::Http(h) = s
                && matches!(h.auth, Some(McpServerAuth::Oauth { .. }))
                && !c.oauth.contains_key(name)
            {
                items.push(name.clone());
                details.push("MCP server".into());
            }
        }
        if items.is_empty() {
            self.set_status(
                "no OAuth identities or MCP servers with OAuth are configured",
                StatusKind::Warn,
            );
            return;
        }
        let title = if login { "Sign in" } else { "Sign out" };
        self.overlay = Some(Overlay::Picker(Picker::new(
            title,
            items,
            details,
            PickAction::Auth { login },
            None,
        )));
    }

    /// Lists the MCP servers; choosing one opens its details editor.
    pub(super) fn open_mcp_details_picker(&mut self) {
        let items: Vec<String> = self.project.config.mcp.servers.keys().cloned().collect();
        if items.is_empty() {
            self.set_status("no servers configured in [mcp.servers]", StatusKind::Warn);
            return;
        }
        self.overlay = Some(Overlay::Picker(Picker::new(
            "Edit an MCP server",
            items,
            vec![],
            PickAction::McpServerDetails,
            None,
        )));
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
            "auth" if matches!(arg, "login" | "logout") => self.open_auth_picker(arg == "login"),
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
            "newenv" => self.prompt_new_env(),
            "theme" if arg.is_empty() => self.open_theme_picker(),
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
                // An unnamed request opens the name prompt: let it stay up
                // instead of hiding it under the quit dialog.
                if self.overlay.is_none() {
                    self.request_quit();
                }
            }
            "q" | "quit" => self.request_quit(),
            "q!" | "quit!" => self.quit = true,
            "new" => self.new_tab(Request::new_http()),
            "newgrpc" => self.new_tab(Request::new_grpc()),
            "newws" => self.new_tab(Request::new_ws()),
            "newmcp" => self.new_tab(Request::new_mcp()),
            "mcpservers" if arg == "edit" => self.open_mcp_editor(),
            "mcpservers" => self.open_mcp_server_picker(),
            "mcpadd" => self.prompt_mcp_add(arg),
            "mcptools" => self.load_mcp_tools(true),
            "mcpedit" if arg.is_empty() => self.open_mcp_details_picker(),
            "mcpedit" => {
                if let Err(e) = self.open_mcp_server_editor(arg) {
                    self.set_status(e, StatusKind::Error);
                }
            }
            "method" => {
                if self.tab_kind_in(&[Kind::Http], "the method") {
                    self.open_method_picker(None);
                }
            }
            "bodytype" => {
                if self.tab_kind_in(&[Kind::Http], "the body type") {
                    self.open_choice_picker(Field::BodyKind);
                }
            }
            "authtype" => {
                if self.tab_kind_in(&[Kind::Http, Kind::Grpc, Kind::Ws], "the auth type") {
                    self.open_choice_picker(Field::AuthKind);
                }
            }
            "grpcmethod" => {
                if self.tab_kind_in(&[Kind::Grpc], "the gRPC method") {
                    self.open_grpc_method_picker();
                }
            }
            "rescan" => {
                self.rescan();
                self.set_status("collection read from disk again", StatusKind::Ok);
            }
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
            "jq" => {
                if arg.is_empty() {
                    self.open_filter();
                } else {
                    match self.apply_filter(arg) {
                        Ok(true) => self.set_status("jq filter applied", StatusKind::Ok),
                        Ok(false) => self.set_status("jq filter cleared", StatusKind::Ok),
                        Err(e) => self.set_status(e, StatusKind::Error),
                    }
                }
            }
            "template" => self.load_schema(false, true, false),
            "reload" => self.load_schema(false, false, true),
            "settings" => self.open_settings(SettingId::Theme),
            "oauth" => self.open_oauth_editor(),
            "keymap" => self.open_keymap_editor(),
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
}
