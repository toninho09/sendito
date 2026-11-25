//! CLI MQTT output and session termination.
use super::run::Ran;
use sendito_core::{MqttRequest, Scope};
use sendito_pipeline::runner::{self, Control, MqttControl, MqttUpdate, RunCtx};
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
    let (tx, mut rx) = tokio::sync::mpsc::channel(64);
    let (control, controls) = tokio::sync::mpsc::unbounded_channel();
    let run = runner::exec_mqtt(ctx, req, scope, errors, tx, controls);
    let mut result = Ran {
        outcome: "error",
        label: "MQTT session did not finish".into(),
        scripts: Default::default(),
        events: None,
        messages: Some(0),
    };
    let collect = async {
        let mut interrupted = false;
        let mut output_failed = false;
        let cancel = || {
            let _ = control.send(Control::Mqtt(MqttControl::Cancel));
        };
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c(), if !interrupted => {
                    interrupted = true;
                    cancel();
                }
                update = rx.recv() => match update {
                    Some(MqttUpdate::Started { resolved }) => {
                        eprintln!("MQTT connecting to {} as {}", resolved.url, resolved.client_id);
                    }
                    Some(MqttUpdate::Connected { session_present, .. }) => {
                        let present = if session_present { "yes" } else { "no" };
                        eprintln!("MQTT connected (session present: {present})");
                    }
                    Some(MqttUpdate::Subscribed { topic, qos, .. }) => {
                        eprintln!("MQTT subscribed {topic} (QoS {qos})");
                    }
                    Some(MqttUpdate::SubscribeRefused { topic, .. }) => {
                        eprintln!("MQTT subscription refused {topic}");
                    }
                    Some(MqttUpdate::Published { topic, qos, .. }) => {
                        eprintln!("MQTT published {topic} (QoS {qos})");
                    }
                    Some(MqttUpdate::PublishFailed { error }) => eprintln!("sendito: {error}"),
                    Some(MqttUpdate::Message { topic, qos, retain, duplicate, payload, .. }) => {
                        if print && !output_failed {
                            let written = write_block(out, include, &topic, qos, retain, duplicate, &payload);
                            if let Err(e) = written {
                                eprintln!("sendito: stdout: {e}");
                                output_failed = true;
                                cancel();
                            }
                        }
                    }
                    Some(MqttUpdate::Done { messages, reason, error, scripts, .. }) => {
                        eprintln!("MQTT {reason}: {messages} messages");
                        if let Some(error) = &error {
                            eprintln!("sendito: {error}");
                        }
                        result = Ran {
                            outcome: if error.is_some() || interrupted || output_failed {
                                "error"
                            } else if scripts.failed_asserts() > 0 {
                                "fail"
                            } else {
                                "pass"
                            },
                            label: error.unwrap_or(reason),
                            scripts,
                            events: None,
                            messages: Some(messages),
                        };
                    }
                    None => break,
                }
            }
        }
    };
    tokio::join!(run, collect);
    result
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
