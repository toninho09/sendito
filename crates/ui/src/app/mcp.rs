//! MCP sign-in, tool discovery and the server editor.

use super::*;

/// The server config for one editor row. A row that keeps its name and its
/// command line/URL keeps everything else it had (env, headers, auth), which
/// the table does not show; changing either rebuilds the entry from scratch.
fn mcp_server_entry(
    kind: usize,
    value: &str,
    old: Option<sendito_store::config::McpServer>,
) -> sendito_store::config::McpServer {
    use sendito_store::config::{McpHttpServer, McpServer, McpStdioServer};
    match (kind, old) {
        (0, Some(McpServer::Http(old))) if old.url == value => McpServer::Http(old),
        (1, Some(McpServer::Stdio(old)))
            if stdio_command_line(&old.command, &old.args) == value =>
        {
            McpServer::Stdio(old)
        }
        (1, _) => {
            let (command, args) = split_command_line(value);
            McpServer::Stdio(McpStdioServer {
                command,
                args,
                env: Default::default(),
            })
        }
        _ => McpServer::Http(McpHttpServer {
            url: value.to_string(),
            headers: Default::default(),
            verify_tls: true,
            auth: None,
        }),
    }
}

/// The auth of an HTTP server from the editor's Auth and Token cells
/// (`MCP_AUTHS`). An OAuth server keeps its fields from the file.
fn mcp_server_auth(
    choice: usize,
    token: &str,
    old: Option<sendito_store::config::McpServerAuth>,
) -> Option<sendito_store::config::McpServerAuth> {
    use sendito_store::config::McpServerAuth;
    match (choice, old) {
        (0, _) => None,
        (1, _) => Some(McpServerAuth::Bearer {
            token: token.to_string(),
        }),
        (_, Some(old @ McpServerAuth::Oauth { .. })) => Some(old),
        _ => Some(McpServerAuth::oauth()),
    }
}

impl App {
    /// A server asked for a sign-in: offer it when the server is an HTTP
    /// entry of `[mcp.servers]` without a bearer token.
    pub(super) fn offer_mcp_login(&mut self, tab: u64, server: &str, after: McpAfterLogin) {
        use sendito_store::config::{McpServer, McpServerAuth};
        let can_login = matches!(
            self.project.config.mcp.servers.get(server),
            Some(McpServer::Http(h)) if !matches!(h.auth, Some(McpServerAuth::Bearer { .. }))
        );
        if !can_login || self.overlay.is_some() {
            return;
        }
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: "Sign in".into(),
            message: format!(
                "\"{server}\" asks you to sign in. Sign in now? Your browser opens for the sign-in."
            ),
            buttons: vec!["Sign in".into(), "Cancel".into()],
            sel: 0,
            action: ConfirmAction::McpLogin {
                server: server.to_string(),
                tab,
                after,
            },
        }));
    }

    /// Starts the OAuth sign-in (or sign-out) of an MCP server. A sign-in
    /// writes `type = "oauth"` for a server without auth, then runs `retry`.
    pub(super) fn mcp_oauth_command(
        &mut self,
        server: &str,
        login: bool,
        retry: Option<(u64, McpAfterLogin)>,
    ) {
        use sendito_store::config::McpServer;
        if self.oauth_login.is_some() {
            self.set_status("an OAuth operation is already running", StatusKind::Warn);
            return;
        }
        let write_auth = matches!(
            self.project.config.mcp.servers.get(server),
            Some(McpServer::Http(h)) if h.auth.is_none()
        );
        let info = runner::McpRunInfo::of(&self.project);
        let envs = self.project.envs.clone();
        let env = self.active_env.clone();
        let runtime = self.runtime.clone();
        let name = server.to_string();
        self.oauth_login_seq = self.oauth_login_seq.wrapping_add(1);
        let job = self.oauth_login_seq;
        let tx = self.tx.clone();
        let task = self.rt.spawn(async move {
            let result: Result<String, String> = async {
                if !login {
                    runner::mcp_logout(&info, &name)
                        .await
                        .map_err(|_| "could not delete OAuth tokens".to_string())?;
                    return Ok(format!("signed out of \"{name}\""));
                }
                let (scope, errors) = tokio::task::spawn_blocking(move || {
                    runner::build_scope(&envs, env.as_deref(), &runtime)
                })
                .await
                .map_err(|_| "environment resolution failed".to_string())?;
                if !errors.is_empty() {
                    return Err("cannot resolve environment secrets".into());
                }
                let note = |text: String| {
                    if let Some(url) = text.strip_prefix("authorization URL:\n  ") {
                        let _ = tx.send(AppMsg::OAuthUrl {
                            job,
                            url: url.into(),
                        });
                    }
                };
                runner::mcp_login(&info, &name, scope, &runner::open_browser, &note).await?;
                Ok(format!("signed in to \"{name}\""))
            }
            .await;
            let _ = tx.send(AppMsg::OAuthDone { job, result });
        });
        self.oauth_login = Some((job, task));
        self.oauth_then = login.then(|| McpLoginThen {
            server: server.to_string(),
            write_auth,
            retry,
        });
        self.set_status(
            if login {
                format!("signing in to \"{server}\"… (Ctrl+W x cancels)")
            } else {
                format!("removing the OAuth tokens of \"{server}\"")
            },
            StatusKind::Info,
        );
    }

    /// After a successful MCP sign-in: save `type = "oauth"` when the server
    /// had no auth, then run the request that asked for the sign-in.
    pub(super) fn after_mcp_login(&mut self, then: McpLoginThen) {
        if then.write_auth {
            let path = match self.project_mcp_servers() {
                Ok(own) if own.contains_key(&then.server) => Some(self.project_config_path()),
                Ok(_) => self.project.global_config.clone(),
                Err(e) => {
                    self.set_status(format!("signed in, but {e}"), StatusKind::Warn);
                    return;
                }
            };
            let auth = sendito_store::config::McpServerAuth::oauth();
            let written = path
                .ok_or_else(|| "no config file holds this server".to_string())
                .and_then(|p| {
                    sendito_store::config::set_mcp_server_auth(&p, &then.server, &auth)
                        .map(|()| p)
                        .map_err(|e| e.to_string())
                });
            match written {
                Ok(p) => {
                    self.reload_mcp_servers();
                    self.set_status(
                        format!(
                            "signed in to \"{}\"; saved auth type = \"oauth\" to {}",
                            then.server,
                            p.display()
                        ),
                        StatusKind::Ok,
                    );
                }
                Err(e) => {
                    self.set_status(
                        format!("signed in, but the auth was not saved: {e}"),
                        StatusKind::Warn,
                    );
                    return;
                }
            }
        }
        let Some((tab, after)) = then.retry else {
            return;
        };
        let Some(i) = self.tabs.iter().position(|t| t.id == tab) else {
            return;
        };
        self.active = i;
        match after {
            McpAfterLogin::Send => self.send(false),
            McpAfterLogin::Tools { open_picker } => self.load_mcp_tools(open_picker),
        }
    }

    // ----- MCP discovery ------------------------------------------------

    pub(super) fn open_mcp_server_picker(&mut self) {
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

    /// Lists a server's tools. `open_picker` also shows the choose dialog;
    /// completion passes `false` and only fills the tab's list.
    pub(super) fn load_mcp_tools(&mut self, open_picker: bool) {
        let Some(t) = self.tab().filter(|t| t.ed.kind == Kind::Mcp) else {
            return;
        };
        let tab = t.id;
        let server = t.ed.mcp_server.text();
        if server.trim().is_empty() {
            self.set_status(
                "set the MCP server first: a name from [mcp.servers] or an http(s):// URL",
                StatusKind::Warn,
            );
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
        if open_picker || self.complete.is_none() {
            self.set_status("loading MCP tools… (Ctrl+W x cancels)", StatusKind::Info);
        }
        let task = self.rt.spawn(async move {
            // The error carries whether the server needs a sign-in.
            let result = async {
                let (scope, errors) = tokio::task::spawn_blocking(move || {
                    runner::build_scope(&envs, env.as_deref(), &runtime)
                })
                .await
                .map_err(|e| (e.to_string(), false))?;
                if !errors.is_empty() {
                    return Err((errors.join("; "), false));
                }
                let name = scope.render(&server2).text;
                let transport =
                    runner::mcp_transport(&info, &name, &scope).map_err(|e| (e, false))?;
                let opts = sendito_mcp::Options {
                    timeout: std::time::Duration::from_millis(info.timeout_ms.max(1)),
                };
                let (events, _rx) = mpsc::unbounded_channel();
                sendito_mcp::list_tools(&transport, &opts, events)
                    .await
                    .map_err(|e| {
                        let needs_login = matches!(e, sendito_mcp::McpError::NeedsLogin(_));
                        (
                            sendito_core::vars::scrub(&e.to_string(), &scope.secret_values()),
                            needs_login,
                        )
                    })
            }
            .await;
            let needs_login = matches!(result, Err((_, true)));
            let _ = tx.send(AppMsg::McpTools {
                job,
                tab,
                server,
                open_picker,
                result: result.map_err(|(e, _)| e),
                needs_login,
            });
        });
        self.mcp_discovery = Some((job, task));
    }

    // ----- MCP servers --------------------------------------------------

    /// The project config file (`<root>/.sendito/config.toml`), where servers
    /// added from the interface are written.
    pub(crate) fn project_config_path(&self) -> std::path::PathBuf {
        sendito_store::config::project_path(&self.project.root)
    }

    /// The project file's own `[mcp.servers]`. The editors work on these
    /// only: the merged config also holds the global file's servers, whose
    /// literal values (tokens) must never be copied into the project file,
    /// which is versioned.
    ///
    /// A project file that does not load cleanly is an error: its servers
    /// would read as none, and a write would delete them.
    pub(crate) fn project_mcp_servers(
        &self,
    ) -> Result<std::collections::BTreeMap<String, sendito_store::config::McpServer>, String> {
        let (cfg, warnings) = sendito_store::config::load(None, Some(&self.project_config_path()));
        if warnings.is_empty() {
            Ok(cfg.mcp.servers)
        } else {
            Err(format!("fix the config first: {}", warnings.join("; ")))
        }
    }

    /// Reloads the merged servers (global and project) after a write.
    pub(crate) fn reload_mcp_servers(&mut self) {
        let path = self.project_config_path();
        self.project.config.mcp.servers =
            sendito_store::config::load(self.project.global_config.as_deref(), Some(&path))
                .0
                .mcp
                .servers;
    }

    pub fn open_mcp_editor(&mut self) {
        self.clear_transient_status();
        let mut ed = McpEditor {
            table: McpEditor::table(),
            dirty: false,
            error: None,
        };
        match self.project_mcp_servers() {
            Ok(servers) => ed.load(&servers),
            Err(e) => ed.error = Some(e),
        }
        self.overlay = Some(Overlay::Mcp(ed));
    }

    /// `:mcpadd <name>`: asks for the URL or command and writes it at once (the
    /// editor is for reviewing and fixing what is already there).
    pub(super) fn prompt_mcp_add(&mut self, name: &str) {
        self.clear_transient_status();
        if name.trim().is_empty() {
            self.set_status(
                "usage: :mcpadd <name> (then the URL or command, and the kind)",
                StatusKind::Warn,
            );
            return;
        }
        let mut prompt = Prompt::new(
            &format!("Add the MCP server \"{}\"", name.trim()),
            "URL or command",
            TextEditor::single(""),
            vec![("Kind".into(), vec!["http".into(), "stdio".into()], 0)],
            PromptAction::McpServer {
                name: name.trim().to_string(),
            },
        );
        prompt.note = Some(
            "http takes a URL (https://host/mcp); stdio a command line, split on spaces.".into(),
        );
        self.overlay = Some(Overlay::Prompt(prompt));
    }

    /// Replaces `[mcp.servers]` in the project config with the editor's rows,
    /// keeping what the table does not show (env, headers, OAuth fields) for a row
    /// whose name and command/URL did not change.
    pub(super) fn save_mcp_editor(&mut self, ed: &mut McpEditor) -> bool {
        // `rows` keeps the first of two rows with one name: say so instead of
        // dropping the other one on save.
        let mut seen = std::collections::HashSet::new();
        for r in ed.table.values() {
            let name = r.cells[0].text().trim().to_string();
            if !name.is_empty() && !seen.insert(name.clone()) {
                ed.error = Some(format!("{name}: two servers have this name"));
                return false;
            }
        }
        let own = match self.project_mcp_servers() {
            Ok(own) => own,
            Err(e) => {
                ed.error = Some(e);
                return false;
            }
        };
        let mut servers = std::collections::BTreeMap::new();
        let mut plain_tokens = Vec::new();
        for row in ed.rows() {
            let name = row.name;
            if row.value.is_empty() {
                ed.error = Some(format!("{name}: set a URL or a command"));
                return false;
            }
            if row.kind == 1 && row.auth != 0 {
                ed.error = Some(format!(
                    "{name}: a stdio server has no auth; set it to none"
                ));
                return false;
            }
            if row.auth == 1 && row.token.is_empty() {
                ed.error = Some(format!("{name}: set a token, or set the auth to none"));
                return false;
            }
            if row.auth == 1 && !row.token.contains("{{") {
                plain_tokens.push(name.clone());
            }
            let old = own.get(&name).cloned();
            let old_auth = match &old {
                Some(sendito_store::config::McpServer::Http(h)) => h.auth.clone(),
                _ => None,
            };
            let mut entry = mcp_server_entry(row.kind, &row.value, old);
            if let sendito_store::config::McpServer::Http(h) = &mut entry {
                h.auth = mcp_server_auth(row.auth, &row.token, old_auth);
            }
            servers.insert(name, entry);
        }
        let path = self.project_config_path();
        if let Err(e) = sendito_store::config::set_mcp_servers(&path, &servers) {
            ed.error = Some(e.to_string());
            return false;
        }
        let n = servers.len();
        self.reload_mcp_servers();
        ed.load(&servers);
        if plain_tokens.is_empty() {
            self.set_status(
                format!("saved {n} MCP server(s) to {}", path.display()),
                StatusKind::Ok,
            );
        } else {
            self.set_status(
                format!(
                    "saved {n} MCP server(s); the token of {} is plain text in a versioned file: \
                     put it in an environment secret and write {{{{name}}}}",
                    plain_tokens.join(", ")
                ),
                StatusKind::Warn,
            );
        }
        true
    }

    /// `:mcpadd` confirm: one server added to the project config.
    pub(super) fn add_mcp_server(
        &mut self,
        name: &str,
        kind: usize,
        value: &str,
    ) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("a server needs a name".into());
        }
        if name.contains(char::is_whitespace) {
            return Err("a server name has no spaces (it is the [mcp.servers.<name>] key)".into());
        }
        let value = value.trim();
        if value.is_empty() {
            return Err(if kind == 1 {
                "a stdio server needs a command".into()
            } else {
                "an http server needs a URL".into()
            });
        }
        let mut servers = self.project_mcp_servers()?;
        let old = servers.get(name).cloned();
        servers.insert(name.to_string(), mcp_server_entry(kind, value, old));
        let path = self.project_config_path();
        sendito_store::config::set_mcp_servers(&path, &servers).map_err(|e| e.to_string())?;
        self.reload_mcp_servers();
        self.set_status(
            format!("added MCP server {name} to {}", path.display()),
            StatusKind::Ok,
        );
        Ok(())
    }
}
