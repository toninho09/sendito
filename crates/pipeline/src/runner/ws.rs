//! Opens a WebSocket session.

use super::*;

/// Commands from the UI to a running WebSocket session.
#[derive(Debug, Clone)]
pub enum WsControl {
    /// One text frame.
    Text(String),
    /// One binary frame (raw bytes).
    Binary(Vec<u8>),
    /// Start the close handshake (1000).
    EndStream,
    /// Abort without a close handshake.
    Cancel,
}

/// Progress of a WebSocket session, forwarded to the UI as it happens.
#[derive(Debug, Clone)]
pub enum WsUpdate {
    Started {
        resolved: sendito_core::resolve::ResolvedWs,
    },
    Event(sendito_ws::Event),
    Error(String),
    Done {
        scripts: ScriptResults,
        scope_errors: Vec<String>,
    },
}

pub(super) fn scrub_ws_event(ev: sendito_ws::Event, secrets: &[String]) -> sendito_ws::Event {
    use sendito_core::vars::scrub;
    let kv = |m: Vec<(String, String)>| -> Vec<(String, String)> {
        m.into_iter()
            .map(|(k, v)| (k, scrub(&v, secrets)))
            .collect()
    };
    match ev {
        sendito_ws::Event::Handshake {
            status,
            headers,
            subprotocol,
            at,
            wall,
        } => sendito_ws::Event::Handshake {
            status,
            // Set-Cookie values are masked, as on an HTTP response.
            headers: kv(headers)
                .into_iter()
                .map(|(k, v)| {
                    if k.eq_ignore_ascii_case("set-cookie") {
                        (k, "••••••".to_string())
                    } else {
                        (k, v)
                    }
                })
                .collect(),
            subprotocol,
            at,
            wall,
        },
        sendito_ws::Event::Sent {
            kind,
            preview,
            size,
            at,
            wall,
        } => sendito_ws::Event::Sent {
            kind,
            preview: scrub(&preview, secrets),
            size,
            at,
            wall,
        },
        sendito_ws::Event::Received {
            kind,
            preview,
            size,
            at,
            wall,
        } => sendito_ws::Event::Received {
            kind,
            preview: scrub(&preview, secrets),
            size,
            at,
            wall,
        },
        sendito_ws::Event::Finished { code, reason, at } => sendito_ws::Event::Finished {
            code,
            reason: scrub(&reason, secrets),
            at,
        },
    }
}

/// The close code the session must end with to count as a success.
pub(super) const WS_CLEAN_CLOSE: u16 = 1000;

/// The HTTP equivalent of a resolved WebSocket URL: the cookie jar only speaks
/// HTTP, and `ws`/`wss` mean the same origin and secure flag as `http`/`https`.
pub(super) fn http_equivalent(url: &str) -> String {
    match url.strip_prefix("wss://") {
        Some(rest) => format!("https://{rest}"),
        None => match url.strip_prefix("ws://") {
            Some(rest) => format!("http://{rest}"),
            None => url.to_string(),
        },
    }
}

/// Runs a WebSocket session. Updates go to `updates`; `control` drives
/// interactive input (like the gRPC streaming path).
pub async fn exec_ws(
    ctx: &RunCtx,
    req: &WebSocketRequest,
    mut scope: Scope,
    scope_errors: Vec<String>,
    updates: mpsc::UnboundedSender<WsUpdate>,
    mut control: mpsc::UnboundedReceiver<Control>,
) {
    let mut results = ScriptResults {
        pre: script::run_pre(&req.scripts.pre, &mut scope),
        ..Default::default()
    };
    let mut resolved = sendito_core::resolve::resolve_ws(req, &scope);
    // `timeout_ms` unset falls back to `[websocket] timeout_ms`; 0 means no
    // limit in either place.
    if resolved.timeout_ms.is_none() {
        resolved.timeout_ms = Some(ctx.ws_timeout_ms);
    }
    if resolved.timeout_ms == Some(0) {
        resolved.timeout_ms = None;
    }
    let template = Request::Ws(req.clone());
    let target = match mask_request(&template, &scope) {
        Request::Ws(w) => sendito_core::resolve::resolve_ws(&w, &Scope::new()).url,
        _ => String::new(),
    };
    let started = std::time::Instant::now();
    let fail_secrets = scope.secret_values();
    let fail = |msg: String, results: ScriptResults| {
        let msg = sendito_core::vars::scrub(&msg, &fail_secrets);
        let entry = history_entry(
            ctx,
            &template,
            target.clone(),
            None,
            started.elapsed().as_millis() as u64,
            0,
            Some(msg.clone()),
        );
        append_history(ctx, &entry);
        let _ = updates.send(WsUpdate::Error(msg));
        let _ = updates.send(WsUpdate::Done {
            scripts: results,
            scope_errors: scope_errors.clone(),
        });
    };
    if req.auth.kind().http_only() {
        return fail(
            format!(
                "{} authentication is only supported for HTTP requests",
                req.auth.kind().label()
            ),
            results,
        );
    }
    // A JWT is signed now; the token is masked like the other secrets.
    match jwt_token(&ctx.root, resolved.signed.as_ref()).await {
        Ok(Some(token)) => {
            scope.set(SIGNED_LAYER, "jwt", VarValue::secret(token.clone()));
            resolved
                .headers
                .push(("Authorization".into(), format!("Bearer {token}")));
        }
        Ok(None) => {}
        Err(e) => return fail(e, results),
    }
    if resolved.url.is_empty() {
        return fail("choose a WebSocket URL first".into(), results);
    }
    let proxy = match sendito_ws::WsProxy::of(ctx.send.proxy.as_ref()) {
        Ok(p) => p,
        Err(e) => return fail(e, results),
    };
    let mut settings = sendito_ws::ConnSettings {
        url: resolved.url.clone(),
        headers: resolved.headers.clone(),
        subprotocols: resolved.subprotocols.clone(),
        verify_tls: resolved.verify_tls,
        handshake_timeout: resolved.timeout_ms.map(Duration::from_millis),
        deadline: resolved.deadline_ms.map(Duration::from_millis),
        close_timeout: sendito_ws::CLOSE_TIMEOUT,
        tls: ctx.send.tls.clone(),
        proxy,
    };
    // The collection cookie jar can add a Cookie header to the handshake. The
    // jar speaks HTTP, so a ws:// URL is presented to it as http:// (and wss as
    // https): the same host, path and secure flag the cookie rules depend on.
    let cookie_url = http_equivalent(&resolved.url);
    let cookies_on = req.options.cookies.unwrap_or(ctx.cookie_jar);
    let jar_env = ctx.env.as_deref().unwrap_or(COLLECTION_ENV);
    let mut cookie_secrets = Vec::new();
    if cookies_on {
        match jar_request(&ctx.root, jar_env, &cookie_url).await {
            Ok((header, secrets)) => {
                cookie_secrets = secrets;
                if let Some(header) = header {
                    settings.headers.push(("Cookie".into(), header));
                }
            }
            Err(e) => return fail(e, results),
        }
    }
    let mut session = match sendito_ws::open(&settings).await {
        Ok(s) => s,
        Err(e) => return fail(e.to_string(), results),
    };
    // The opening messages go out right after the handshake, in order.
    let frames = match sendito_ws::parse_messages(&resolved.messages) {
        Ok(f) => f,
        Err(e) => {
            session.cancel();
            return fail(e, results);
        }
    };
    let mut initial_error = None;
    for f in frames {
        let r = match f {
            sendito_ws::Frame::Text(t) => session.send_text(t),
            sendito_ws::Frame::Binary(b) => session.send_binary(b),
        };
        if let Err(e) = r {
            initial_error = Some(e.to_string());
            break;
        }
    }
    let secrets = scope.secret_values();
    // The UI shows this URL ("connecting to"): a key in the query is masked.
    let mut shown = resolved.clone();
    shown.url = sendito_core::vars::scrub(&shown.url, &secrets);
    let _ = updates.send(WsUpdate::Started { resolved: shown });
    let mut summary = WsSummary::new();
    let mut control_open = true;
    if let Some(e) = initial_error {
        let _ = updates.send(WsUpdate::Error(e.clone()));
    }
    loop {
        tokio::select! {
            ev = session.events.recv() => {
                let Some(ev) = ev else { break };
                if cookies_on
                    && let sendito_ws::Event::Handshake { headers, .. } = &ev
                {
                    // Store cookies the handshake response set.
                    let sets = set_cookie_headers(headers);
                    if !sets.is_empty()
                        && let Err(e) = jar_store(&ctx.root, jar_env, &cookie_url, &sets).await
                    {
                        let _ = updates.send(WsUpdate::Error(format!(
                            "cookies not stored: {e}"
                        )));
                    }
                }
                let done = matches!(ev, sendito_ws::Event::Finished { .. });
                summary.add(&ev);
                let ev = if ctx.mask_echoed_secrets { scrub_ws_event(ev, &secrets) } else { ev };
                let _ = updates.send(WsUpdate::Event(ev));
                if done { break; }
            }
            cmd = control.recv(), if control_open => {
                let cmd = cmd.and_then(|c| match c {
                    Control::Ws(w) => Some(w),
                    Control::Grpc(_) | Control::Sse(_) | Control::Mqtt(_) => None,
                });
                match cmd {
                    Some(WsControl::Text(text)) => {
                        let rendered = scope.render(&text).text;
                        if let Err(e) = session.send_text(rendered) {
                            let _ = updates.send(WsUpdate::Error(e.to_string()));
                        }
                    }
                    Some(WsControl::Binary(bytes)) => {
                        if let Err(e) = session.send_binary(bytes) {
                            let _ = updates.send(WsUpdate::Error(e.to_string()));
                        }
                    }
                    Some(WsControl::EndStream) => session.close(),
                    Some(WsControl::Cancel) => session.cancel(),
                    // The controller went away (headless runs). With a deadline
                    // the session runs until it expires, so a scripted run sees
                    // every frame the server sends; without one there is nothing
                    // to wait for, and the close handshake ends it. `close()`
                    // is also bounded by `close_timeout`, so this never hangs.
                    None => {
                        control_open = false;
                        if resolved.deadline_ms.is_none() {
                            session.close();
                        }
                    }
                }
            }
        }
    }
    // Scripts see the handshake headers, the close code as status and the
    // last text frame as body.
    let WsSummary {
        code,
        reason,
        headers,
        body,
        total_ms,
        size,
    } = summary;
    let view = ResponseView {
        status: code,
        headers: &headers,
        body: &body,
    };
    results.extracts = script::run_extract(&req.scripts.extract, &view, &mut scope);
    results.asserts = script::run_asserts(&req.scripts.asserts, &view, &scope);
    scrub_asserts(
        &mut results.asserts,
        &[&secrets[..], &cookie_secrets[..]].concat(),
    );
    let total = total_ms.unwrap_or_else(|| started.elapsed().as_millis() as u64);
    let err = if code == WS_CLEAN_CLOSE {
        None
    } else if reason.is_empty() {
        Some(format!("close {code}"))
    } else {
        // The peer's reason can echo a secret: mask it before history.
        let all = [&secrets[..], &cookie_secrets[..]].concat();
        Some(format!(
            "close {code}: {}",
            sendito_core::vars::scrub(&reason, &all)
        ))
    };
    let entry = history_entry(ctx, &template, target, Some(code as i64), total, size, err);
    write_history(ctx, &entry).await;
    let _ = updates.send(WsUpdate::Done {
        scripts: results,
        scope_errors,
    });
}

/// What scripts and history need from a WebSocket session, kept as events
/// arrive: the close code and reason, the handshake headers, the last text
/// frame, the total time and the received bytes.
pub(super) struct WsSummary {
    code: u16,
    reason: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
    total_ms: Option<u64>,
    size: u64,
}

impl WsSummary {
    fn new() -> Self {
        Self {
            code: 1006,
            reason: String::new(),
            headers: Vec::new(),
            body: Vec::new(),
            total_ms: None,
            size: 0,
        }
    }

    fn add(&mut self, e: &sendito_ws::Event) {
        match e {
            sendito_ws::Event::Handshake { headers, .. } => self.headers = headers.clone(),
            sendito_ws::Event::Received {
                kind,
                preview,
                size,
                ..
            } => {
                if *kind == sendito_ws::FrameKind::Text {
                    self.body = preview.as_bytes().to_vec();
                }
                self.size += *size as u64;
            }
            sendito_ws::Event::Finished {
                code, reason, at, ..
            } => {
                self.code = *code;
                self.reason = reason.clone();
                self.total_ms = Some(at.as_millis() as u64);
            }
            _ => {}
        }
    }
}
