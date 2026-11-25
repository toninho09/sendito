//! Live SSE sessions through the shared HTTP authentication pipeline.
use super::*;
use sendito_http::sse::{SseEvent, SseStream};

#[derive(Debug, Clone)]
pub enum SseControl {
    End,
    Cancel,
}
#[derive(Debug, Clone)]
pub enum SseUpdate {
    Started {
        resolved: Box<ResolvedHttp>,
        response: Box<HttpResponse>,
    },
    Event {
        event: SseEvent,
        at: Duration,
        size: usize,
    },
    Done {
        events: u64,
        duration: Duration,
        reason: String,
        error: Option<String>,
        scripts: ScriptResults,
        scope_errors: Vec<String>,
    },
}

async fn wait_deadline(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(end) => tokio::time::sleep_until(end).await,
        None => std::future::pending().await,
    }
}

fn end_reason(command: Option<Control>) -> &'static str {
    if matches!(command, Some(Control::Sse(SseControl::End))) {
        "ended"
    } else {
        "cancelled"
    }
}

pub async fn exec_sse(
    ctx: &RunCtx,
    req: &SseRequest,
    mut scope: Scope,
    scope_errors: Vec<String>,
    updates: mpsc::Sender<SseUpdate>,
    mut control: mpsc::UnboundedReceiver<Control>,
) {
    let start = Instant::now();
    let request = req.http_request();
    let mut opening_ctx = ctx.clone();
    opening_ctx.history = None;
    opening_ctx.send.default_timeout = Duration::from_millis(ctx.sse_timeout_ms);
    let opening = http::exec_http_inner(
        &opening_ctx,
        &request,
        scope.clone(),
        scope_errors.clone(),
        true,
    );
    tokio::pin!(opening);
    let run = tokio::select! {
        run = &mut opening => Some(run),
        _ = control.recv() => None,
    };
    let mut events = 0;
    let mut size = 0;
    let mut last = Vec::new();
    let mut headers = Vec::new();
    let mut status = 0;
    let mut scripts = ScriptResults::default();
    let mut secrets = scope.secret_values();
    let mut error = None;
    let reason;
    if let Some(mut run) = run {
        secrets.extend(run.secrets);
        scripts = run.outcome.scripts;
        for (name, value) in &scripts.pre {
            scope.set(RUNTIME_LAYER, name, value.clone());
        }
        match run.outcome.response {
            Err(e) => {
                error = Some(e);
                reason = "error";
            }
            Ok(response) => {
                status = response.status;
                headers = response.headers.clone();
                match run
                    .stream
                    .take()
                    .ok_or("SSE opening returned no stream".to_string())
                    .and_then(|s| SseStream::from_transport(s).map_err(|e| e.to_string()))
                {
                    Err(e) => {
                        error = Some(e);
                        reason = "error";
                    }
                    Ok(mut stream) => {
                        let mut resolved = run.outcome.resolved;
                        resolved.url = sendito_core::vars::scrub(&resolved.url, &secrets);
                        for (_, value) in &mut resolved.headers {
                            *value = sendito_core::vars::scrub(value, &secrets);
                        }
                        for note in &mut resolved.notes {
                            *note = sendito_core::vars::scrub(note, &secrets);
                        }
                        let mut shown = response;
                        for (name, value) in &mut shown.headers {
                            *value = if name.eq_ignore_ascii_case("set-cookie") {
                                "••••••".into()
                            } else {
                                sendito_core::vars::scrub(value, &secrets)
                            };
                        }
                        let _ = updates
                            .send(SseUpdate::Started {
                                resolved: Box::new(resolved),
                                response: Box::new(shown),
                            })
                            .await;
                        let deadline = req
                            .options
                            .deadline_ms
                            .map(|ms| tokio::time::Instant::now() + Duration::from_millis(ms));
                        reason = loop {
                            if req.options.max_events.is_some_and(|max| events >= max) {
                                break "max events";
                            }
                            let next = tokio::select! {
                                event = stream.next_event() => event,
                                command = control.recv() => break end_reason(command),
                                _ = wait_deadline(deadline) => break "deadline",
                                _ = updates.closed() => break "cancelled",
                            };
                            match next {
                                Ok(Some(mut event)) => {
                                    events += 1;
                                    size += event.data.len() as u64;
                                    last = event.data.as_bytes().to_vec();
                                    if ctx.mask_echoed_secrets {
                                        event.data =
                                            sendito_core::vars::scrub(&event.data, &secrets);
                                        event.id = sendito_core::vars::scrub(&event.id, &secrets);
                                        event.event =
                                            sendito_core::vars::scrub(&event.event, &secrets);
                                    }
                                    let update = SseUpdate::Event {
                                        event,
                                        at: start.elapsed(),
                                        size: last.len(),
                                    };
                                    tokio::select! {
                                        sent = updates.send(update) => if sent.is_err() { break "cancelled"; },
                                        command = control.recv() => break end_reason(command),
                                        _ = wait_deadline(deadline) => break "deadline",
                                    }
                                }
                                Ok(None) => break "server closed",
                                Err(e) => {
                                    error = Some(e.to_string());
                                    break "error";
                                }
                            }
                        };
                    }
                }
            }
        }
    } else {
        reason = "cancelled";
    }
    if status > 0 {
        let view = ResponseView {
            status,
            headers: &headers,
            body: &last,
        };
        scripts.extracts = script::run_extract(&req.scripts.extract, &view, &mut scope);
        for extract in &mut scripts.extracts {
            if let Ok(value) = &mut extract.value
                && secrets
                    .iter()
                    .any(|s| !s.is_empty() && value.value.contains(s))
            {
                value.secret = true;
                scope.set(RUNTIME_LAYER, &extract.var, value.clone());
            }
        }
        scripts.asserts = script::run_asserts(&req.scripts.asserts, &view, &scope);
        scrub_asserts(&mut scripts.asserts, &secrets);
    }
    error = error.map(|e| sendito_core::vars::scrub(&e, &secrets));
    let template = mask_request(&Request::Sse(req.clone()), &scope);
    let target = match &template {
        Request::Sse(s) => s.url.clone(),
        _ => String::new(),
    };
    let entry = history_entry(
        ctx,
        &template,
        target,
        (status > 0).then_some(status as i64),
        start.elapsed().as_millis() as u64,
        size,
        error.clone(),
    );
    write_history(ctx, &entry).await;
    let _ = updates
        .send(SseUpdate::Done {
            events,
            duration: start.elapsed(),
            reason: reason.into(),
            error,
            scripts,
            scope_errors,
        })
        .await;
}
