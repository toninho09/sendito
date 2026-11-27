//! CLI MQTT output: the session runs in `sendito_pipeline::suite::run_mqtt`.
use sendito_core::{MqttRequest, Scope};
use sendito_pipeline::runner::RunCtx;
use sendito_pipeline::suite::{self, Live, Ran};
use std::io::Write;

/// Runs one MQTT session. Each message payload goes to `out` as one block
/// (with `include`, a `# topic qos=N` line first); progress goes to stderr.
/// Ctrl+C cancels the session.
pub(super) async fn run_mqtt(
    ctx: &RunCtx,
    req: &MqttRequest,
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
        Live::Message {
            topic,
            qos,
            retain,
            duplicate,
            payload,
        } => {
            if !print {
                return true;
            }
            let written = write_block(out, include, topic, qos, retain, duplicate, payload);
            if let Err(e) = &written {
                eprintln!("sendito: stdout: {e}");
            }
            written.is_ok()
        }
        Live::Head(_) | Live::Data(_) => true,
    };
    suite::run_mqtt(ctx, req, scope, errors, &ctrl_c.cancel, &mut on).await
}

/// One message block: the optional `# topic` line, the payload bytes and a
/// line break when the payload does not end with one.
fn write_block(
    out: &mut dyn Write,
    include: bool,
    topic: &str,
    qos: u8,
    retain: bool,
    duplicate: bool,
    payload: &[u8],
) -> std::io::Result<()> {
    if include {
        let retain = if retain { " retain" } else { "" };
        let duplicate = if duplicate { " dup" } else { "" };
        writeln!(out, "# {topic} qos={qos}{retain}{duplicate}")?;
    }
    out.write_all(payload)?;
    if !payload.ends_with(b"\n") {
        writeln!(out)?;
    }
    out.flush()
}
