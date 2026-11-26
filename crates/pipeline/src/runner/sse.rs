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
    // The pre phase runs here, on the SSE request, so the scripts see and
    // change its fields; the opening request does not run it again.
    let PrePhase {
        req,
        results: mut scripts,
        error: pre_error,
    } = prepare(ctx, req, &mut scope).await;
    let req = &req;
    let mut request = req.http_request();
    request.common.scripts.pre.clear();
    request.common.scripts.lua_pre.clear();
    let mut opening_ctx = ctx.clone();
    opening_ctx.history = None;
    opening_ctx.send.default_timeout = Duration::from_millis(ctx.sse_timeout_ms);
    let run = if pre_error.is_some() {
        None
    } else {
        let opening = http::exec_http_inner(
            &opening_ctx,
            &request,
            scope.clone(),
            scope_errors.clone(),
            true,
        );
        tokio::pin!(opening);
        tokio::select! {
            run = &mut opening => Some(run),
            _ = control.recv() => None,
        }
    };
    let mut events = 0;
    let mut size = 0;
    let mut last = Vec::new();
    let mut headers = Vec::new();
    let mut status = 0;
    let mut secrets = scope.secret_values();
    let mut error = None;
    let reason;
    if let Some(mut run) = run {
        secrets.extend(run.secrets);
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
    } else if let Some(e) = pre_error {
        error = Some(e);
        reason = "error";
    } else {
        reason = "cancelled";
    }
    if status > 0 {
        let view = ResponseView {
            status,
            headers: &headers,
            body: &last,
            timing: &[("total", start.elapsed())],
        };
        response_phase(
            &req.common.scripts,
            &view,
            &mut scope,
            &mut scripts,
            &secrets,
        );
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
    write_history(ctx, &entry, None).await;
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
