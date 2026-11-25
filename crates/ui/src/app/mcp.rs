//! MCP sign-in, tool discovery, the connection test and `:mcpadd`.

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
            // The file that holds the server: the project file first.
            let path = self.mcp_server_file(&then.server).map(|(p, _)| p);
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
        let list = self.tool_list(server.clone());
        if let Some((_, task)) = self.mcp_discovery.take() {
            task.abort();
        }
        self.mcp_discovery_seq = self.mcp_discovery_seq.wrapping_add(1);
        let job = self.mcp_discovery_seq;
        if open_picker || self.complete.is_none() {
            self.set_status("loading MCP tools… (Ctrl+W x cancels)", StatusKind::Info);
        }
        let tx = self.tx.clone();
        let task = self.rt.spawn(async move {
            let result = list.await;
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

    /// Lists the tools of `server` (a `[mcp.servers]` name or a URL, with
    /// `{{variables}}`) in the active scope. The error says whether the
    /// server needs a sign-in.
    fn tool_list(
        &self,
        server: String,
    ) -> impl std::future::Future<Output = Result<Vec<sendito_pipeline::mcp::Tool>, (String, bool)>>
    + Send
    + 'static {
        let info = runner::McpRunInfo::of(&self.project);
        let envs = self.project.envs.clone();
        let env = self.active_env.clone();
        let runtime = self.runtime.clone();
        async move {
            let (scope, errors) = tokio::task::spawn_blocking(move || {
                runner::build_scope(&envs, env.as_deref(), &runtime)
            })
            .await
            .map_err(|e| (e.to_string(), false))?;
            if !errors.is_empty() {
                return Err((errors.join("; "), false));
            }
            let name = scope.render(&server).text;
            let transport = runner::mcp_transport(&info, &name, &scope).map_err(|e| (e, false))?;
            let opts = sendito_pipeline::mcp::Options {
                timeout: std::time::Duration::from_millis(info.timeout_ms.max(1)),
            };
            let (events, _rx) = mpsc::unbounded_channel();
            sendito_pipeline::mcp::list_tools(&transport, &opts, events)
                .await
                .map_err(|e| {
                    let needs_login = matches!(e, sendito_pipeline::mcp::McpError::NeedsLogin(_));
                    (
                        sendito_core::vars::scrub(&e.to_string(), &scope.secret_values()),
                        needs_login,
                    )
                })
        }
    }

    /// The record editor's connection test: lists the tools of the saved
    /// server `name`; the answer comes back as `AppMsg::McpTest`.
    pub(crate) fn start_mcp_test(&mut self, name: &str) {
        let list = self.tool_list(name.to_string());
        let tx = self.tx.clone();
        let server = name.to_string();
        self.rt.spawn(async move {
            let result = match list.await {
                Ok(tools) => Ok(tools.len()),
                Err((e, true)) => Err(format!("{e} (sign in: :mcp login {server})")),
                Err((e, false)) => Err(e),
            };
            let _ = tx.send(AppMsg::McpTest { server, result });
        });
    }

    // ----- MCP servers --------------------------------------------------

    /// The project config file (`<root>/.sendito/config.toml`).
    pub(crate) fn project_config_path(&self) -> std::path::PathBuf {
        sendito_store::config::project_path(&self.project.root)
    }

    /// The config file the MCP editors write: the settings scope, as for the
    /// OAuth and keymap editors.
    pub(crate) fn mcp_config_path(&self) -> Result<std::path::PathBuf, String> {
        self.scope_path(self.config_scope).ok_or_else(|| {
            "there is no global config file: choose Project (Tab) in Settings".to_string()
        })
    }

    /// The `[mcp.servers]` of one config file, as that file holds it (not
    /// merged). The editors work on one file's own servers: a write replaces
    /// the table, and one file's literal values (tokens) must never be copied
    /// into the other file, which may be versioned.
    ///
    /// A file that does not load cleanly is an error: its servers would read
    /// as none, and a write would delete them.
    pub(crate) fn mcp_servers_in(
        path: &std::path::Path,
    ) -> Result<std::collections::BTreeMap<String, sendito_store::config::McpServer>, String> {
        let (cfg, warnings) = sendito_store::config::load(None, Some(path));
        if warnings.is_empty() {
            Ok(cfg.mcp.servers)
        } else {
            Err(format!("fix the config first: {}", warnings.join("; ")))
        }
    }

    /// The `[mcp.servers]` of the settings scope's file.
    pub(crate) fn scope_mcp_servers(
        &self,
    ) -> Result<std::collections::BTreeMap<String, sendito_store::config::McpServer>, String> {
        Self::mcp_servers_in(&self.mcp_config_path()?)
    }

    /// The file that holds the MCP server `name`, and the server as that file
    /// holds it: the project file first (it wins over the global file), then
    /// the global file. `None` when no file that loads holds it.
    pub(crate) fn mcp_server_file(
        &self,
        name: &str,
    ) -> Option<(std::path::PathBuf, sendito_store::config::McpServer)> {
        let global = self.project.global_config.clone();
        [Some(self.project_config_path()), global]
            .into_iter()
            .flatten()
            .find_map(|path| {
                let servers = Self::mcp_servers_in(&path).ok()?;
                servers.get(name).map(|s| (path, s.clone()))
            })
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

    /// `:mcpadd <name>`: asks for the URL or command and writes it at once to
    /// the settings scope's config file (the editor is for reviewing and
    /// fixing what is already there).
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

    /// `:mcpadd` confirm: one server added to the settings scope's config file.
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
        let path = self.mcp_config_path()?;
        let mut servers = Self::mcp_servers_in(&path)?;
        let old = servers.get(name).cloned();
        servers.insert(name.to_string(), mcp_server_entry(kind, value, old));
        sendito_store::config::set_mcp_servers(&path, &servers).map_err(|e| e.to_string())?;
        self.reload_mcp_servers();
        self.set_status(
            format!("added MCP server {name} to {}", path.display()),
            StatusKind::Ok,
        );
        Ok(())
    }
}
