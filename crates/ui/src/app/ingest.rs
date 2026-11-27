//! Results that arrive from the runner: WebSocket, HTTP, gRPC and MCP updates and app messages.

use super::*;

impl App {
    /// WebSocket session progress: same shape as the gRPC log.
    pub(super) fn on_ws(&mut self, tab_id: u64, u: WsUpdate) {
        let mut status: Option<(String, StatusKind)> = None;
        let mut updates = Vec::new();
        let keep = self.project.config.ui.stream_messages_kept;
        let Some(t) = self.tab_by_id(tab_id) else {
            return;
        };
        let resp = t.response.get_or_insert_with(Response::new);
        // Follow the log while the cursor is at its end.
        let before = resp.messages.row_count();
        let follow = resp.messages.cursor + 2 >= before;
        match u {
            WsUpdate::Started { resolved } => {
                resp.summary.status = String::new();
                // The resolved URL can carry an API key in its query string.
                resp.messages
                    .doc
                    .append_text(&format!("connecting to {}\n", resolved.url));
            }
            WsUpdate::Event(ev) => {
                resp.messages.doc.append_text(&ws_event_text(&ev));
                match &ev {
                    sendito_pipeline::ws::Event::Handshake {
                        status,
                        headers,
                        subprotocol,
                        ..
                    } => {
                        if *status == 101 {
                            let sub = subprotocol
                                .as_ref()
                                .map(|s| format!(" · subprotocol {s}"))
                                .unwrap_or_default();
                            resp.summary.status = format!("101 Switching Protocols{sub}");
                            resp.summary.ok = true;
                        } else {
                            resp.summary.status = format!("handshake answered {status}");
                            resp.summary.ok = false;
                        }
                        resp.headers = crate::tab::wrapped(Doc::text(&ws_headers_text(headers)));
                    }
                    sendito_pipeline::ws::Event::Finished {
                        code, reason, at, ..
                    } => {
                        let label = format!(
                            "close {code}{}",
                            if reason.is_empty() {
                                String::new()
                            } else {
                                format!(": {reason}")
                            }
                        );
                        resp.summary.status = if *code == 1000 && reason.is_empty() {
                            "closed cleanly (1000)".into()
                        } else {
                            label.clone()
                        };
                        resp.summary.ok = *code == 1000;
                        resp.summary.time_ms = Some(at.as_secs_f64() * 1000.0);
                        if *code != 1000 {
                            resp.summary.error = Some(label);
                        }
                    }
                    _ => {}
                }
                resp.push_ws(ev, keep);
                resp.summary.size = Some(resp.stream.bytes);
                resp.timing = crate::tab::wrapped(Doc::text(&ws_timing_text(&resp.stream)));
            }
            WsUpdate::Error(e) => {
                resp.messages.doc.append_text(&format!("error: {e}\n"));
                if resp.ws_events.is_empty() {
                    resp.summary.status = "Error".into();
                    resp.summary.error = Some(e.clone());
                    resp.set_body_wrapped(format!("Session failed:\n\n{e}\n"));
                }
                status = Some((e, StatusKind::Error));
            }
            WsUpdate::Done {
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
                let result = if resp.summary.status.is_empty() {
                    "session finished".to_string()
                } else {
                    format!("session finished: {}", resp.summary.status)
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

    pub fn on_msg(&mut self, msg: AppMsg) {
        match msg {
            AppMsg::Scope {
                result,
                then,
                secrets_ok,
                accepted,
            } => {
                let (scope, errors, failed) = *result;
                // Check the loaded values before `{{name}}` (or a lower
                // layer's value) goes out: a secret that did not load (no
                // keychain entry, no secrets.toml value), or a function that
                // the names check could not run (a secret argument, a Lua
                // function), passed the names check. "Send anyway" for
                // undefined names covers only the names it listed.
                if !secrets_ok {
                    let (tab, req) = then.request();
                    let mut un = runner::unresolved_send(
                        &req,
                        &scope,
                        &failed,
                        &self.project.config.mcp.servers,
                    );
                    un.retain(|u| !accepted.contains(u));
                    if !un.is_empty() {
                        self.not_resolved(tab, &un, &errors, &scope, !failed.is_empty());
                        return;
                    }
                }
                self.start_with_scope(scope, errors, then);
            }
            AppMsg::Run { tab, event } => self.on_run(tab, *event),
            AppMsg::RunDone { tab } => self.on_run_done(tab),
            AppMsg::Http { tab, outcome } => self.on_http(tab, *outcome),
            AppMsg::Grpc { tab, update } => self.on_grpc(tab, update),
            AppMsg::Ws { tab, update } => self.on_ws(tab, update),
            AppMsg::Sse { tab, update, ack } => {
                self.on_sse(tab, update);
                let _ = ack.send(());
            }
            AppMsg::Mqtt { tab, update, ack } => {
                self.on_mqtt(tab, update);
                let _ = ack.send(());
            }
            AppMsg::Mcp { tab, update } => self.on_mcp(tab, update),
            AppMsg::OAuthUrl { job, url } => {
                if self.oauth_login.as_ref().is_some_and(|(id, _)| *id == job) {
                    let text = if url.starts_with("http") {
                        format!(
                            "Open this URL in a browser if it did not open automatically:\n\n{url}\n\nWaiting for the callback. Close this dialog and use Ctrl+W x to cancel.\n"
                        )
                    } else {
                        // A device sign-in: the page and the code.
                        format!(
                            "{url}\n\nWaiting for the sign-in on the other device. Close this dialog and use Ctrl+W x to cancel.\n"
                        )
                    };
                    if self.overlay.is_none() {
                        self.show_report("OAuth login", &text);
                    } else {
                        self.last_report = Some(("OAuth login".into(), text));
                        self.set_status("OAuth URL available with :report", StatusKind::Info);
                    }
                }
            }
            AppMsg::OAuthDone { job, result } => {
                if self.oauth_login.as_ref().is_some_and(|(id, _)| *id == job) {
                    self.oauth_login = None;
                    if matches!(&self.overlay, Some(Overlay::Report(r)) if r.title == "OAuth login")
                    {
                        self.overlay = None;
                    }
                    let then = self.oauth_then.take();
                    match result {
                        Ok(message) => {
                            self.set_status(message, StatusKind::Ok);
                            if let Some(then) = then {
                                self.after_mcp_login(then);
                            }
                        }
                        Err(e) => self.set_status(e, StatusKind::Error),
                    }
                }
            }
            AppMsg::Cookies { job, result } => {
                if self.cookies_job.as_ref().is_some_and(|(id, _)| *id == job) {
                    self.cookies_job = None;
                    self.status = None;
                    match result {
                        Ok(CookiesOutcome::List(rows)) if rows.is_empty() => self
                            .set_status("no cookies in the active environment", StatusKind::Info),
                        Ok(CookiesOutcome::List(rows)) if self.overlay.is_none() => {
                            let mut items = rows;
                            items.push(CLEAR_COOKIES.into());
                            self.overlay = Some(Overlay::Picker(Picker::new(
                                "Cookie jar (Enter deletes)",
                                items,
                                vec![],
                                PickAction::Cookie,
                                None,
                            )));
                        }
                        // Another dialog opened meanwhile: keep the list for :report.
                        Ok(CookiesOutcome::List(rows)) => {
                            let text = format!(
                                "{}\n\n:cookies delete <number> removes one; :cookies clear removes all.\n",
                                rows.join("\n")
                            );
                            self.last_report = Some(("Cookie jar".into(), text));
                            self.set_status("cookies listed with :report", StatusKind::Info);
                        }
                        Ok(CookiesOutcome::Done(m)) => self.set_status(m, StatusKind::Ok),
                        Err(e) => self.set_status(format!("cookie jar: {e}"), StatusKind::Error),
                    }
                }
            }
            AppMsg::McpTest { server, result } => {
                if let Some(Overlay::Records(e)) = &mut self.overlay
                    && e.kind == crate::records::RecordKind::Mcp
                {
                    match result {
                        Ok(n) => {
                            e.error = None;
                            e.note = Some(format!(
                                "✓ {server} answered: {n} tool{}",
                                if n == 1 { "" } else { "s" }
                            ));
                        }
                        Err(err) => {
                            e.note = None;
                            e.error = Some(format!("{server}: {err}"));
                        }
                    }
                }
            }
            AppMsg::McpTools {
                job,
                tab,
                server,
                open_picker,
                result,
                needs_login,
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
                if active {
                    match result {
                        Ok(catalog) => {
                            if let Some(t) = self.tab_mut() {
                                t.mcp = catalog;
                                t.mcp_tools_server = server.clone();
                                t.mcp_tools_error = None;
                            }
                            if self.complete.is_some() {
                                self.refresh_complete();
                            } else if open_picker {
                                let items =
                                    self.tab().map(|t| mcp_items(&t.mcp)).unwrap_or_default();
                                if items.is_empty() {
                                    self.set_status(
                                        "the server has no tools, resources or prompts",
                                        StatusKind::Info,
                                    );
                                } else if let Some(t) = self.tab() {
                                    let current = items.iter().position(|(op, target, _)| {
                                        *op == t.ed.mcp_op && *target == t.ed.mcp_tool.text()
                                    });
                                    let (names, details) = items
                                        .into_iter()
                                        .map(|(_, target, detail)| (target, detail))
                                        .unzip();
                                    self.overlay = Some(Overlay::Picker(Picker::new(
                                        "Choose an MCP tool, resource or prompt",
                                        names,
                                        details,
                                        PickAction::McpTool,
                                        current,
                                    )));
                                }
                            }
                        }
                        Err(e) => {
                            if let Some(t) = self.tab_mut() {
                                t.mcp_tools_error = Some(e.clone());
                                t.mcp_tools_server = server.clone();
                            }
                            if let Some(c) = &mut self.complete {
                                // The popup reports the failure; it is not
                                // retried until asked.
                                c.loading = None;
                                c.failed = Some("tools");
                            }
                            self.set_status(format!("tools: {e}"), StatusKind::Error);
                            // A sign-in is offered when the list was asked
                            // for, not for a background completion pull.
                            if needs_login && open_picker {
                                self.offer_mcp_login(
                                    tab,
                                    &server,
                                    McpAfterLogin::Tools { open_picker },
                                );
                            }
                        }
                    }
                }
            }
            AppMsg::Schema {
                tab,
                job,
                result,
                open_picker,
                template,
            } => {
                if !self.schema_job.as_ref().is_some_and(|(id, _)| *id == job) {
                    return;
                }
                self.schema_job = None;
                let mut status = None;
                let mut status_ok: Option<String> = None;
                let mut failed = false;
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
                            failed = true;
                        }
                    }
                }
                if failed && let Some(c) = &mut self.complete {
                    // The popup reports the failure; it is not retried until asked.
                    c.loading = None;
                    c.failed = Some("schema");
                } else if self.complete.is_some() {
                    self.refresh_complete();
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
            AppMsg::GraphqlSchema {
                tab,
                job,
                source,
                result,
            } => {
                if !self.schema_job.as_ref().is_some_and(|(id, _)| *id == job) {
                    return;
                }
                self.schema_job = None;
                let current = self
                    .tabs
                    .iter()
                    .find(|t| t.id == tab)
                    .map(|t| self.graphql_source(t));
                if current.as_deref() != Some(source.as_str()) {
                    return;
                }
                if let Some(t) = self.tab_by_id(tab) {
                    t.schema_error = result.as_ref().err().cloned();
                    t.graphql_schema = result.as_ref().ok().cloned();
                }
                if self.complete.is_some() {
                    if let Some(c) = &mut self.complete {
                        c.loading = None;
                        c.failed = result.as_ref().err().map(|_| "schema");
                    }
                    self.refresh_complete();
                }
                match result {
                    Ok(schema) => self.set_status(
                        format!("schema loaded: {} types", schema.types.len()),
                        StatusKind::Ok,
                    ),
                    Err(e) => self.set_status(format!("schema: {e}"), StatusKind::Error),
                }
            }
            AppMsg::History(h) => {
                // Keep the selection on the same entry: new ones arrive on top.
                let key = |e: &sendito_store::history::HistoryEntry| (e.ts.clone(), e.name.clone());
                let selected = self
                    .history
                    .as_ref()
                    .and_then(|old| old.get(self.history_sel))
                    .map(key);
                let mut h = h;
                h.reverse();
                self.history_sel = selected
                    .and_then(|k| h.iter().position(|e| key(e) == k))
                    .unwrap_or(0);
                self.history_follow = true;
                self.history = Some(h);
            }
        }
    }

    pub(super) fn apply_updates(&mut self, updates: Vec<(String, VarValue)>) {
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

    pub(super) fn on_http(&mut self, tab_id: u64, mut o: HttpOutcome) {
        let updates = o.scripts.updates();
        let mut status: Option<(String, StatusKind)> = None;
        if let Some(t) = self.tab_by_id(tab_id) {
            let elapsed_ms = t
                .running
                .as_ref()
                .map(|r| r.started.elapsed().as_secs_f64() * 1000.0);
            t.running = None;
            let watch = t.watch.as_mut().map(|w| {
                match &o.response {
                    Ok(r) => w.record(r.status < 400, r.timing.total.as_secs_f64() * 1000.0),
                    Err(_) => w.record(false, elapsed_ms.unwrap_or(0.0)),
                }
                watch_text(&w.samples)
            });
            let mut resp = http_response(&mut o, watch.as_deref());
            match &o.response {
                Ok(r) => {
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
                    // The reason is in the response pane; the status line names the
                    // outcome only (it would be cut short anyway).
                    status = Some((
                        format!("{} · details in the response pane", short_error(e)),
                        StatusKind::Error,
                    ));
                }
            }
            if t.watch.is_some()
                && o.response.is_ok()
                && let Some(old) = &t.response
            {
                resp.keep_view_of(old);
            }
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

    pub(super) fn on_grpc(&mut self, tab_id: u64, u: GrpcUpdate) {
        let mut status: Option<(String, StatusKind)> = None;
        let mut updates = Vec::new();
        let end_key = self.keymap.hint(Action::EndStream, self.vim);
        let keep = self.project.config.ui.stream_messages_kept;
        let Some(t) = self.tab_by_id(tab_id) else {
            return;
        };
        let resp = t.response.get_or_insert_with(Response::new);
        // Follow the log while the cursor is at its end.
        let before = resp.messages.row_count();
        // A unary call's log is short and reads from the top.
        let follow = resp.messages.cursor + 2 >= before
            && resp.grpc_kind != Some(sendito_pipeline::grpc::CallKind::Unary);
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
                if let sendito_pipeline::grpc::Event::Finished {
                    code, message, at, ..
                } = &ev
                {
                    let name = sendito_pipeline::grpc::code_name(*code);
                    resp.summary.status = if message.is_empty() {
                        format!("{name} ({code})")
                    } else {
                        format!("{name} ({code}): {message}")
                    };
                    resp.summary.ok = *code == 0;
                    resp.summary.code = None;
                    resp.summary.time_ms = Some(at.as_secs_f64() * 1000.0);
                    if *code != 0 {
                        resp.summary.error = Some(format!("{name}: {message}"));
                    }
                }
                // Headers and trailers change only with their own events.
                let meta = matches!(
                    ev,
                    sendito_pipeline::grpc::Event::Headers { .. }
                        | sendito_pipeline::grpc::Event::Finished { .. }
                );
                resp.push_grpc(ev, keep);
                resp.summary.size = Some(resp.stream.bytes);
                if meta {
                    resp.headers =
                        crate::tab::wrapped(Doc::text(&grpc_headers_text(&resp.grpc_events)));
                }
                resp.timing = crate::tab::wrapped(Doc::text(&grpc_timing_text(&resp.stream)));
            }
            GrpcUpdate::Error(e) => {
                resp.messages.doc.append_text(&format!("error: {e}\n"));
                if resp.grpc_events.is_empty() {
                    resp.summary.status = "Error".into();
                    resp.summary.error = Some(e.clone());
                    resp.set_body_wrapped(format!("Call failed:\n\n{e}\n"));
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

    pub(super) fn on_mcp(&mut self, tab_id: u64, u: runner::McpUpdate) {
        let mut status: Option<(String, StatusKind)> = None;
        let mut login_offer: Option<String> = None;
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
                needs_login,
            } => {
                if needs_login {
                    login_offer = Some(t.ed.mcp_server.text());
                }
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
                    resp.set_body(Doc::from_bytes(body, Some("application/json")));
                } else if let Some(e) = &error {
                    resp.set_body_wrapped(format!("Call failed:\n\n{e}\n"));
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
        if let Some(server) = login_offer {
            self.offer_mcp_login(tab_id, &server, McpAfterLogin::Send);
        }
    }
}

/// The response pane of an HTTP exchange; the body moves out of `o`.
/// `watch` is the watch line added to the timing.
pub(super) fn http_response(o: &mut HttpOutcome, watch: Option<&str>) -> Response {
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
            resp.set_body(std::mem::take(&mut o.doc));
            resp.headers = crate::tab::wrapped(Doc::text(&headers_text(r, o)));
            resp.http_head = Some(crate::tab::HttpHead {
                status: r.status,
                reason: r.reason.clone(),
                headers: r.headers.clone(),
            });
            let mut timing = timing_text(r, o.body_size);
            timing.push_str(watch.unwrap_or_default());
            resp.timing = crate::tab::wrapped(Doc::text(&timing));
        }
        Err(e) => {
            if let Some(w) = watch {
                resp.timing = crate::tab::wrapped(Doc::text(w));
            }
            resp.summary = Summary {
                status: "Error".into(),
                error: Some(e.clone()),
                ..Default::default()
            };
            resp.set_body_wrapped(format!(
                "Request failed: {}\n\nDetails:\n{e}\n",
                explain_error(e)
            ));
            resp.headers = crate::tab::wrapped(Doc::text(&request_headers_text(o)));
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
    resp
}

/// The picker rows of a server's catalog, in this order: tools, resources,
/// resource templates and prompts. Each row is the operation, the target
/// (tool name, URI or prompt name) and the detail text.
pub(super) fn mcp_items(c: &sendito_pipeline::mcp::Catalog) -> Vec<(McpOp, String, String)> {
    let with = |kind: &str, text: Option<&str>| match text {
        Some(t) if !t.is_empty() => format!("{kind} · {t}"),
        _ => kind.to_string(),
    };
    let mut v: Vec<(McpOp, String, String)> = Vec::new();
    v.extend(c.tools.iter().map(|t| {
        (
            McpOp::Tool,
            t.name.clone(),
            with("tool", t.description.as_deref()),
        )
    }));
    v.extend(c.resources.iter().map(|r| {
        (
            McpOp::Resource,
            r.uri.clone(),
            with("resource", Some(&r.name)),
        )
    }));
    v.extend(c.templates.iter().map(|r| {
        (
            McpOp::Resource,
            r.uri.clone(),
            with("resource template", Some(&r.name)),
        )
    }));
    v.extend(c.prompts.iter().map(|p| {
        (
            McpOp::Prompt,
            p.name.clone(),
            with("prompt", p.description.as_deref()),
        )
    }));
    v
}
