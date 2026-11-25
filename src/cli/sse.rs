//! CLI SSE output and session termination.
use super::run::Ran;
use sendito_core::{Scope, SseRequest};
use sendito_pipeline::runner::{self, Control, RunCtx, SseControl, SseUpdate};
use std::io::Write;

pub(super) async fn run_sse(
    ctx: &RunCtx,
    req: &SseRequest,
    scope: Scope,
    errors: Vec<String>,
    print: bool,
    include: bool,
    out: &mut dyn Write,
) -> Ran {
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let (control, controls) = tokio::sync::mpsc::unbounded_channel();
    let run = runner::exec_sse(ctx, req, scope, errors, tx, controls);
    let mut result = Ran {
        outcome: "error",
        label: "SSE session did not finish".into(),
        scripts: Default::default(),
        events: Some(0),
        messages: None,
    };
    let collect = async {
        let mut interrupted = false;
        let mut output_failed = false;
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c(), if !interrupted => {
                    interrupted = true;
                    let _ = control.send(Control::Sse(SseControl::Cancel));
                }
                update = rx.recv() => match update {
                    Some(SseUpdate::Started { response, .. }) => {
                        eprintln!("SSE connected: {}", response.status);
                        if print && include {
                            let _ = writeln!(out, "HTTP {} {}", response.status, response.reason);
                            for (name, value) in response.headers { let _ = writeln!(out, "{name}: {value}"); }
                            let _ = writeln!(out);
                        }
                    }
                    Some(SseUpdate::Event { event, .. }) => {
                        if print {
                            for line in event.data.split('\n') {
                                if let Err(e) = writeln!(out, "{line}") {
                                    eprintln!("sendito: stdout: {e}");
                                    output_failed = true;
                                    let _ = control.send(Control::Sse(SseControl::Cancel));
                                }
                            }
                            if out.flush().is_err() { output_failed = true; let _ = control.send(Control::Sse(SseControl::Cancel)); }
                        }
                    }
                    Some(SseUpdate::Done { events, reason, error, scripts, .. }) => {
                        eprintln!("SSE {reason}: {events} events");
                        if let Some(error) = &error { eprintln!("sendito: {error}"); }
                        result = Ran { outcome: if error.is_some() || interrupted || output_failed { "error" } else if scripts.failed_asserts() > 0 { "fail" } else { "pass" },
                            label: error.unwrap_or(reason), scripts, events: Some(events), messages: None };
                    }
                    None => break,
                }
            }
        }
    };
    tokio::join!(run, collect);
    result
}
