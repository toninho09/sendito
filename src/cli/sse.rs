//! CLI SSE output: the session runs in `sendito_pipeline::suite::run_sse`.
use sendito_core::{Scope, SseRequest};
use sendito_pipeline::runner::RunCtx;
use sendito_pipeline::suite::{self, Live, Ran};
use std::io::Write;

/// Runs one SSE session. With `print`, each event's data goes to `out`
/// (with `include`, the response head first); progress goes to stderr.
/// Ctrl+C cancels the session.
pub(super) async fn run_sse(
    ctx: &RunCtx,
    req: &SseRequest,
    scope: Scope,
    errors: Vec<String>,
    print: bool,
    include: bool,
    out: &mut dyn Write,
) -> Ran {
    let ctrl_c = super::run::CtrlC::new();
    let mut on = |live: Live<'_>| match live {
        Live::Line(l) => {
            eprintln!("{l}");
            true
        }
        Live::Head(r) => {
            if print && include {
                let _ = writeln!(out, "HTTP {} {}", r.status, r.reason);
                for (name, value) in &r.headers {
                    let _ = writeln!(out, "{name}: {value}");
                }
                let _ = writeln!(out);
            }
            true
        }
        Live::Data(data) => !print || write_data(out, data),
        Live::Message { .. } => true,
    };
    suite::run_sse(ctx, req, scope, errors, &ctrl_c.cancel, &mut on).await
}

/// Writes one event's data, one line per data line; `false` when stdout fails.
fn write_data(out: &mut dyn Write, data: &str) -> bool {
    for line in data.split('\n') {
        if let Err(e) = writeln!(out, "{line}") {
            eprintln!("sendito: stdout: {e}");
            return false;
        }
    }
    out.flush().is_ok()
}
