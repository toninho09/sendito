//! Live MQTT sessions: pre-request script, connect, subscribe, publish, then
//! report every message until the deadline, a disconnect or a cancel.
use super::*;
use sendito_core::resolve::{
    ResolvedMqtt, ResolvedMqttMessage, resolve_mqtt, resolve_mqtt_message,
};
use sendito_core::vars::scrub;

/// Commands from the UI or the CLI to a running MQTT session.
#[derive(Debug, Clone)]
pub enum MqttControl {
    /// One publish. Its variables are rendered with the session scope.
    Publish(MqttMessage),
    /// Send DISCONNECT and end cleanly.
    Disconnect,
    /// Abort without DISCONNECT: the broker publishes the Last Will.
    Cancel,
}

/// Progress of an MQTT session, in arrival order.
#[derive(Debug, Clone)]
pub enum MqttUpdate {
    /// The connection starts. Secrets in `resolved` are masked.
    Started {
        resolved: Box<ResolvedMqtt>,
    },
    Connected {
        session_present: bool,
        at: Duration,
    },
    Subscribed {
        topic: String,
        qos: u8,
        at: Duration,
    },
    /// The broker refused a subscription. The session continues.
    SubscribeRefused {
        topic: String,
        at: Duration,
    },
    Message {
        topic: String,
        qos: u8,
        retain: bool,
        duplicate: bool,
        payload: Vec<u8>,
        at: Duration,
    },
    Published {
        topic: String,
        qos: u8,
        at: Duration,
    },
    /// A live publish the session refused. The session continues.
    PublishFailed {
        error: String,
    },
    Done {
        messages: u64,
        duration: Duration,
        reason: String,
        error: Option<String>,
        scripts: ScriptResults,
        scope_errors: Vec<String>,
    },
}

/// The status scripts see when the broker accepts the connection: the
/// CONNACK return code 0.
pub const MQTT_ACCEPTED: u16 = 0;

fn wire_message(m: &ResolvedMqttMessage) -> sendito_mqtt::Publish {
    sendito_mqtt::Publish {
        topic: m.topic.clone(),
        payload: m.payload.clone(),
        qos: m.qos,
        retain: m.retain,
    }
}

/// The broker options of a resolved request.
fn broker_options(ctx: &RunCtx, r: &ResolvedMqtt) -> Result<sendito_mqtt::BrokerOptions, String> {
    let target = r
        .target
        .as_ref()
        .ok_or_else(|| r.error.clone().unwrap_or_default())?;
    Ok(sendito_mqtt::BrokerOptions {
        host: target.host.clone(),
        port: target.port,
        tls: target.tls,
        client_id: r.client_id.clone(),
        username: r.username.clone(),
        password: r.password.clone(),
        protocol: match r.protocol {
            MqttProtocol::V3_1_1 => sendito_mqtt::Protocol::V3_1_1,
            MqttProtocol::V5 => sendito_mqtt::Protocol::V5,
        },
        clean_session: r.clean_session,
        keep_alive_s: r.keep_alive_s,
        verify_tls: r.verify_tls,
        tls_material: ctx.send.tls.clone(),
        last_will: r.last_will.as_ref().map(|w| sendito_mqtt::Will {
            topic: w.topic.clone(),
            payload: w.payload.clone(),
            qos: w.qos,
            retain: w.retain,
        }),
    })
}

/// A copy of `r` safe to show: the password and every secret value masked.
fn shown(r: &ResolvedMqtt, secrets: &[String]) -> ResolvedMqtt {
    let mut out = r.clone();
    out.url = scrub(&out.url, secrets);
    out.client_id = scrub(&out.client_id, secrets);
    out.username = out.username.map(|u| scrub(&u, secrets));
    out.password = out.password.map(|_| sendito_core::vars::MASK.to_string());
    for (topic, _) in &mut out.subscriptions {
        *topic = scrub(topic, secrets);
    }
    for m in out.publish.iter_mut().chain(out.last_will.as_mut()) {
        m.topic = scrub(&m.topic, secrets);
        if let Ok(text) = std::str::from_utf8(&m.payload) {
            m.payload = scrub(text, secrets).into_bytes();
        }
    }
    out
}

/// The payload as shown: secrets masked when it is UTF-8 text.
fn scrub_payload(payload: Vec<u8>, secrets: &[String]) -> Vec<u8> {
    match String::from_utf8(payload) {
        Ok(text) => scrub(&text, secrets).into_bytes(),
        Err(e) => e.into_bytes(),
    }
}

async fn wait_until(at: Option<tokio::time::Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// Runs one MQTT session. Updates go to `updates`; `control` publishes,
/// disconnects or cancels.
pub async fn exec_mqtt(
    ctx: &RunCtx,
    req: &MqttRequest,
    mut scope: Scope,
    scope_errors: Vec<String>,
    updates: mpsc::Sender<MqttUpdate>,
    mut control: mpsc::UnboundedReceiver<Control>,
) {
    let start = Instant::now();
    let PrePhase {
        req,
        results: mut scripts,
        error: pre_error,
    } = pre_phase(req, &mut scope);
    let req = &req;
    let resolved = resolve_mqtt(req, &scope);
    let secrets = scope.secret_values();
    let timeout = match resolved.timeout_ms.unwrap_or(ctx.mqtt_timeout_ms) {
        0 => None,
        ms => Some(Duration::from_millis(ms)),
    };
    let mut messages = 0u64;
    let mut size = 0u64;
    let mut last = Vec::new();
    let mut connack: Option<bool> = None;
    let mut error: Option<String> = None;
    let reason: String;
    let opened = match pre_error.or_else(|| resolved.error.clone()) {
        Some(e) => Err(e),
        None => match broker_options(ctx, &resolved) {
            Err(e) => Err(e),
            Ok(options) => {
                let _ = updates
                    .send(MqttUpdate::Started {
                        resolved: Box::new(shown(&resolved, &secrets)),
                    })
                    .await;
                sendito_mqtt::open(
                    options,
                    resolved
                        .subscriptions
                        .iter()
                        .map(|(topic, qos)| sendito_mqtt::Subscription {
                            topic: topic.clone(),
                            qos: *qos,
                        })
                        .collect(),
                    resolved.publish.iter().map(wire_message).collect(),
                    resolved.deadline_ms.map(Duration::from_millis),
                )
                .await
            }
        },
    };
    match opened {
        Err(e) => {
            error = Some(e);
            reason = "error".into();
        }
        Ok(mut session) => {
            let connect_by = timeout.map(|t| tokio::time::Instant::now() + t);
            let mut control_open = true;
            reason = loop {
                let waiting = connack.is_none();
                tokio::select! {
                    event = session.events.recv() => {
                        let Some(event) = event else { break "ended".to_string() };
                        let update = match event {
                            sendito_mqtt::Event::Connected { session_present, at } => {
                                connack = Some(session_present);
                                MqttUpdate::Connected { session_present, at }
                            }
                            sendito_mqtt::Event::Subscribed { topic, qos, at } => {
                                MqttUpdate::Subscribed { topic: scrub(&topic, &secrets), qos, at }
                            }
                            sendito_mqtt::Event::SubscribeRefused { topic, at } => {
                                MqttUpdate::SubscribeRefused { topic: scrub(&topic, &secrets), at }
                            }
                            sendito_mqtt::Event::Message { topic, qos, retain, duplicate, payload, at } => {
                                messages += 1;
                                size += payload.len() as u64;
                                last.clone_from(&payload);
                                let (topic, payload) = if ctx.mask_echoed_secrets {
                                    (scrub(&topic, &secrets), scrub_payload(payload, &secrets))
                                } else {
                                    (topic, payload)
                                };
                                MqttUpdate::Message { topic, qos, retain, duplicate, payload, at }
                            }
                            sendito_mqtt::Event::Published { topic, qos, at } => {
                                MqttUpdate::Published { topic: scrub(&topic, &secrets), qos, at }
                            }
                            sendito_mqtt::Event::Ended { reason, error: e, .. } => {
                                error = e;
                                break reason;
                            }
                        };
                        if updates.send(update).await.is_err() {
                            session.cancel();
                            break "cancelled".into();
                        }
                    }
                    command = control.recv(), if control_open => match command {
                        Some(Control::Mqtt(MqttControl::Publish(m))) => {
                            let result = resolve_mqtt_message(&m, &scope)
                                .and_then(|m| session.publish(wire_message(&m)));
                            if let Err(e) = result {
                                let error = scrub(&e, &secrets);
                                let _ = updates.send(MqttUpdate::PublishFailed { error }).await;
                            }
                        }
                        Some(Control::Mqtt(MqttControl::Disconnect)) => session.disconnect(),
                        Some(Control::Mqtt(MqttControl::Cancel)) => {
                            session.cancel();
                            break "cancelled".into();
                        }
                        Some(_) => {}
                        // The controller went away (a headless run): with a
                        // deadline the session runs until it expires, without
                        // one it disconnects.
                        None => {
                            control_open = false;
                            if resolved.deadline_ms.is_none() {
                                session.disconnect();
                            }
                        }
                    },
                    _ = wait_until(connect_by), if waiting => {
                        session.cancel();
                        let ms = timeout.map(|t| t.as_millis()).unwrap_or_default();
                        error = Some(format!("the broker sent no CONNACK within {ms} ms"));
                        break "error".into();
                    }
                }
            };
        }
    }
    if let Some(session_present) = connack {
        let headers = vec![
            ("connack".to_string(), "Connection Accepted".to_string()),
            ("session-present".to_string(), session_present.to_string()),
        ];
        let view = ResponseView {
            status: MQTT_ACCEPTED,
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
    error = error.map(|e| scrub(&e, &secrets));
    let template = mask_request(&Request::Mqtt(req.clone()), &scope);
    let target = match &template {
        Request::Mqtt(m) => m.url.clone(),
        _ => String::new(),
    };
    let entry = history_entry(
        ctx,
        &template,
        target,
        connack.map(|_| i64::from(MQTT_ACCEPTED)),
        start.elapsed().as_millis() as u64,
        size,
        error.clone(),
    );
    write_history(ctx, &entry).await;
    let _ = updates
        .send(MqttUpdate::Done {
            messages,
            duration: start.elapsed(),
            reason,
            error,
            scripts,
            scope_errors,
        })
        .await;
}
