//! Sending: options, missing secrets, the run context, cancel and end of stream.

use super::*;

impl App {
    // ----- sending ------------------------------------------------------

    pub fn send(&mut self, confirmed: bool) {
        self.send_opts(confirmed, false);
    }

    /// The action button of the URL card: Cancel while a call runs that takes
    /// no more input, otherwise Send (an open stream sends one more message).
    pub(super) fn press_send_button(&mut self) {
        let cancels = self
            .tab()
            .and_then(|t| t.running.as_ref())
            .is_some_and(|r| !r.streaming_input);
        self.dispatch(if cancels {
            Action::Cancel
        } else {
            Action::Send
        });
    }

    /// Sends the active tab's request. `from_watch` marks an automatic repeat:
    /// no note about unsaved edits, and the watch cycle restarts.
    pub(super) fn send_opts(&mut self, confirmed: bool, from_watch: bool) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            self.set_status("open or create a request first (Ctrl+N)", StatusKind::Warn);
            return;
        };
        if tab.folder.is_some() {
            self.set_status(
                "folder settings are not sent: they apply to the requests in the folder",
                StatusKind::Warn,
            );
            return;
        }
        tab.refresh_model();
        // On an open client/bidi stream or WebSocket session, Send pushes the
        // editor content as more input.
        if let Some(r) = &tab.running {
            if r.streaming_input && tab.ed.kind == Kind::Mqtt {
                self.mqtt_publish_selected();
                return;
            }
            if r.streaming_input {
                let ws = tab.ed.kind == Kind::Ws;
                let json = tab.ed.message.text();
                let compact = if ws {
                    json.clone()
                } else {
                    serde_json::from_str::<serde_json::Value>(&json)
                        .map(|v| v.to_string())
                        .unwrap_or_else(|_| json.split_whitespace().collect::<Vec<_>>().join(" "))
                };
                let shown: String = if compact.chars().count() > 32 {
                    compact.chars().take(31).chain(['…']).collect()
                } else {
                    compact
                };
                if let Some(c) = &r.control {
                    let _ = c.send(if ws {
                        Control::Ws(WsControl::Text(json))
                    } else {
                        Control::Grpc(GrpcControl::Send(json))
                    });
                }
                // Select the sent text so the next message can be typed over it.
                tab.ed.message.select_all();
                let sent = tab.response.as_ref().map(|r| r.stream.sent).unwrap_or(0) + 1;
                let end = self.keymap.hint(Action::EndStream, self.vim);
                let noun = if ws { "frame" } else { "message" };
                self.set_status(
                    format!("{noun} {sent} sent: {shown} · {end} ends the stream"),
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
        // A history copy is recorded under the request it came from, and
        // inherits from its folders.
        let path = tab.path.clone().or_else(|| tab.origin_path.clone());
        let chain = match runner::folder_chain(&self.project.root, path.as_deref()) {
            Ok(chain) => chain,
            Err(e) => {
                self.set_status(format!("not sent: {e}"), StatusKind::Error);
                return;
            }
        };
        let mut names = self.names.clone();
        let folder_note = sendito_core::inherit::ignored_headers_note(&tab.model, &chain);
        let req = runner::with_folders(&tab.model, &chain, &mut names);
        if let Request::Grpc(g) = &req
            && (g.service.is_empty() || g.method.is_empty())
        {
            self.focus = Focus::Bar(0);
            self.set_status(
                "choose a method first: Enter on the method chip lists the services",
                StatusKind::Warn,
            );
            return;
        }
        if let Request::Ws(w) = &req
            && w.url.trim().is_empty()
        {
            self.focus = Focus::Bar(1);
            self.set_status("enter a ws:// or wss:// URL first", StatusKind::Warn);
            return;
        }
        if let Request::Mcp(m) = &req
            && (m.server.trim().is_empty() || m.tool.trim().is_empty())
        {
            self.focus = Focus::ReqTabs;
            self.set_status(
                "choose a server and a tool first: F2 lists servers, Enter on the tool chip lists tools",
                StatusKind::Warn,
            );
            return;
        }
        if !confirmed {
            let un = runner::unresolved(&req, &names);
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
        let edited = tab.path.is_some() && tab.dirty();
        // A watch repeat keeps the last response on screen until the new
        // one arrives and takes over its view.
        if !(from_watch && tab.response.is_some()) {
            let mut resp = Response::new();
            resp.summary.status = String::new();
            tab.response = Some(resp);
        }
        if let (Some(note), Some(resp)) = (&folder_note, tab.response.as_mut()) {
            resp.messages.doc.append_text(&format!("note: {note}\n"));
        }
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
            Request::Sse(s) => ScopeThen::Sse {
                tab: id,
                req: Box::new(s),
                name,
                path,
            },
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
            Request::Ws(w) => ScopeThen::Ws {
                tab: id,
                req: Box::new(w),
                name,
                path,
            },
            Request::Mqtt(m) => ScopeThen::Mqtt {
                tab: id,
                req: Box::new(m),
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
        let secrets_ok = std::mem::take(&mut self.secrets_ok);
        let task = self.rt.spawn(async move {
            let result = tokio::task::spawn_blocking(move || {
                let mut out = runner::build_scope_checked(&envs, active.as_deref(), &runtime);
                runner::add_folder_vars(&mut out.0, &chain);
                out
            })
            .await
            .unwrap_or_else(|e| (Scope::new(), vec![e.to_string()], Vec::new()));
            let _ = tx.send(AppMsg::Scope {
                result: Box::new(result),
                then,
                secrets_ok,
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

    /// Stops a send whose secrets did not load, and asks before sending
    /// the placeholders as typed (a watch stops: it would repeat this).
    pub(super) fn secrets_missing(&mut self, tab: u64, un: &[String], errors: &[String]) {
        let active = self.tab().is_some_and(|t| t.id == tab);
        if let Some(t) = self.tab_by_id(tab) {
            t.running = None;
            t.watch = None;
            if t.response
                .as_ref()
                .is_some_and(|r| r.summary.status.is_empty())
            {
                t.response = None;
            }
        }
        let list = un
            .iter()
            .map(|u| format!("{{{{{u}}}}}"))
            .collect::<Vec<_>>()
            .join(", ");
        // Another tab, or a dialog the user is working in (which must not
        // be replaced): the status line says it instead.
        if !active || self.overlay.is_some() {
            self.set_status(
                format!("not sent: {list} could not be loaded or checked"),
                StatusKind::Error,
            );
            return;
        }
        self.overlay = Some(Overlay::Confirm(Confirm {
            title: "Secrets not loaded".into(),
            message: format!(
                "{list}: a secret did not load, or the environment file cannot be read so \
                 its values cannot be checked ({}). Send anyway sends the request as it \
                 resolves now: as typed, braces included, or with a value from another layer.",
                errors.join("; ")
            ),
            buttons: vec!["Send anyway".into(), "Cancel".into()],
            sel: 1,
            action: ConfirmAction::SendMissingSecrets,
        }));
    }

    pub(super) fn run_ctx(&self, name: String, path: Option<String>) -> RunCtx {
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
            ws_timeout_ms: self.project.config.websocket.timeout_ms,
            sse_timeout_ms: self.project.config.sse.timeout_ms,
            mqtt_timeout_ms: self.project.config.mqtt.timeout_ms,
            mask_echoed_secrets: true,
            cookie_jar: self.project.config.http.cookie_jar,
            oauth: Some(runner::OAuthRunInfo::of(&self.project)),
        }
    }

    pub(super) fn start_with_scope(&mut self, scope: Scope, errors: Vec<String>, then: ScopeThen) {
        let tx = self.tx.clone();
        match then {
            ScopeThen::Sse {
                tab,
                req,
                name,
                path,
            } => {
                if self.tabs.iter().all(|t| t.id != tab || t.running.is_none()) {
                    return;
                }
                let ctx = self.run_ctx(name, path);
                let (utx, mut urx) = mpsc::channel(64);
                let (control, controls) = mpsc::unbounded_channel();
                let task = self.rt.spawn(async move {
                    let run = runner::exec_sse(&ctx, &req, scope, errors, utx, controls);
                    let forward = async {
                        while let Some(update) = urx.recv().await {
                            let (ack, received) = tokio::sync::oneshot::channel();
                            if tx.send(AppMsg::Sse { tab, update, ack }).is_err() {
                                break;
                            }
                            if received.await.is_err() {
                                break;
                            }
                        }
                    };
                    tokio::join!(run, forward);
                });
                if let Some(t) = self.tab_by_id(tab)
                    && let Some(r) = &mut t.running
                {
                    r.task = task;
                    r.control = Some(control);
                }
            }
            ScopeThen::Mqtt {
                tab,
                req,
                name,
                path,
            } => {
                if self.tabs.iter().all(|t| t.id != tab || t.running.is_none()) {
                    return;
                }
                let ctx = self.run_ctx(name, path);
                let (utx, mut urx) = mpsc::channel(64);
                let (control, controls) = mpsc::unbounded_channel();
                let task = self.rt.spawn(async move {
                    let run = runner::exec_mqtt(&ctx, &req, scope, errors, utx, controls);
                    let forward = async {
                        while let Some(update) = urx.recv().await {
                            let (ack, received) = tokio::sync::oneshot::channel();
                            if tx.send(AppMsg::Mqtt { tab, update, ack }).is_err() {
                                break;
                            }
                            if received.await.is_err() {
                                break;
                            }
                        }
                    };
                    tokio::join!(run, forward);
                });
                if let Some(t) = self.tab_by_id(tab)
                    && let Some(r) = &mut t.running
                {
                    r.task = task;
                    r.control = Some(control);
                }
            }
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
                let (ctx_tx, ctx_rx) = mpsc::unbounded_channel::<Control>();
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
            ScopeThen::Ws {
                tab,
                req,
                name,
                path,
            } => {
                if self.tabs.iter().all(|t| t.id != tab || t.running.is_none()) {
                    return;
                }
                let ctx = self.run_ctx(name, path);
                let (utx, mut urx) = mpsc::unbounded_channel::<runner::WsUpdate>();
                let (ctx_tx, ctx_rx) = mpsc::unbounded_channel::<Control>();
                let fwd = tx.clone();
                let task = self.rt.spawn(async move {
                    let run = runner::exec_ws(&ctx, &req, scope, errors, utx, ctx_rx);
                    let forward = async {
                        while let Some(u) = urx.recv().await {
                            let _ = fwd.send(AppMsg::Ws { tab, update: u });
                        }
                    };
                    tokio::join!(run, forward);
                });
                if let Some(t) = self.tab_by_id(tab)
                    && let Some(r) = &mut t.running
                {
                    r.task = task;
                    r.control = Some(ctx_tx);
                    r.streaming_input = true;
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

    pub(super) fn cancel(&mut self) {
        // The request in view comes first; background jobs are cancelled
        // when it is not running.
        if !self.tab().is_some_and(|t| t.running.is_some()) {
            if let Some((_, task)) = self.cookies_job.take() {
                task.abort();
                self.set_status("cookie command cancelled", StatusKind::Info);
                return;
            }
            if let Some((_, task)) = self.oauth_login.take() {
                task.abort();
                self.oauth_then = None;
                self.set_status("OAuth login cancelled", StatusKind::Info);
                return;
            }
            if let Some((_, task)) = self.mcp_discovery.take() {
                task.abort();
                self.set_status("MCP tool discovery cancelled", StatusKind::Info);
                return;
            }
            if let Some((_, task)) = self.schema_job.take() {
                task.abort();
                self.set_status("schema load cancelled", StatusKind::Info);
                return;
            }
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
        let kind = t.ed.kind;
        if let Some(c) = &r.control {
            let _ = c.send(match kind {
                Kind::Ws => Control::Ws(WsControl::Cancel),
                Kind::Sse => Control::Sse(runner::SseControl::Cancel),
                Kind::Mqtt => Control::Mqtt(runner::MqttControl::Cancel),
                _ => Control::Grpc(GrpcControl::Cancel),
            });
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

    pub(super) fn end_stream(&mut self) {
        let Some(t) = self.tab_mut() else { return };
        if t.ed.kind == Kind::Mqtt {
            match t.running.as_ref().and_then(|r| r.control.as_ref()) {
                Some(control) => {
                    let _ = control.send(Control::Mqtt(runner::MqttControl::Disconnect));
                    self.set_status("disconnecting", StatusKind::Info);
                }
                None => self.set_status("no open stream or session", StatusKind::Info),
            }
            return;
        }
        if t.ed.kind == Kind::Sse {
            if let Some(r) = &t.running
                && let Some(control) = &r.control
            {
                let _ = control.send(Control::Sse(runner::SseControl::End));
            }
            return;
        }
        match t.running.as_mut() {
            Some(r) if r.streaming_input => {
                let ws = t.ed.kind == Kind::Ws;
                if let Some(c) = &r.control {
                    let _ = c.send(if ws {
                        Control::Ws(WsControl::EndStream)
                    } else {
                        Control::Grpc(GrpcControl::EndStream)
                    });
                }
                r.streaming_input = false;
                // Nothing more can be sent: drop the "type over it" selection.
                t.ed.message.anchor = None;
            }
            _ => self.set_status("no open stream or session", StatusKind::Info),
        }
    }
}
