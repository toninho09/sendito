//! The task behind one session: connect, subscribe, publish, report.
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rumqttc::{AsyncClient, EventLoop, MqttOptions, QoS, TlsConfiguration, Transport};
use tokio::sync::mpsc;

use super::{BrokerOptions, Event, Protocol, Publish, Session, Subscription};

pub(super) enum Cmd {
    Publish(Publish),
    Disconnect,
}

/// One packet the session cares about, in the shape both protocols share.
enum In {
    ConnAck {
        session_present: bool,
    },
    SubAck {
        granted: Vec<u8>,
    },
    Message {
        topic: String,
        qos: u8,
        retain: bool,
        duplicate: bool,
        payload: Vec<u8>,
    },
    Ack,
    Other,
}

/// One request that waits for room in the client queue.
enum Out {
    Subscribe(Subscription),
    Publish(Publish),
}

enum Driver {
    V4(Box<(AsyncClient, EventLoop)>),
    V5(Box<(rumqttc::v5::AsyncClient, rumqttc::v5::EventLoop)>),
}

impl Driver {
    fn build(options: &BrokerOptions) -> Result<Self, String> {
        let transport = if options.tls {
            let cfg = sendito_http::tls_config(options.verify_tls, &[], &options.tls_material)?;
            Transport::tls_with_config(TlsConfiguration::Rustls(Arc::new(cfg)))
        } else {
            Transport::Tcp
        };
        match options.protocol {
            Protocol::V3_1_1 => {
                let mut opts = MqttOptions::new(&options.client_id, &options.host, options.port);
                opts.set_transport(transport)
                    .set_keep_alive(Duration::from_secs(options.keep_alive_s.max(1)))
                    .set_clean_session(options.clean_session);
                if let (Some(user), Some(pass)) = (&options.username, &options.password) {
                    opts.set_credentials(user, pass);
                }
                if let Some(will) = &options.last_will {
                    opts.set_last_will(rumqttc::LastWill::new(
                        &will.topic,
                        will.payload.clone(),
                        qos(will.qos),
                        will.retain,
                    ));
                }
                let (client, eventloop) = AsyncClient::new(opts, 64);
                Ok(Driver::V4(Box::new((client, eventloop))))
            }
            Protocol::V5 => {
                let mut opts =
                    rumqttc::v5::MqttOptions::new(&options.client_id, &options.host, options.port);
                opts.set_transport(transport)
                    .set_keep_alive(Duration::from_secs(options.keep_alive_s.max(1)))
                    .set_clean_start(options.clean_session);
                if let (Some(user), Some(pass)) = (&options.username, &options.password) {
                    opts.set_credentials(user, pass);
                }
                if let Some(will) = &options.last_will {
                    opts.set_last_will(rumqttc::v5::mqttbytes::v5::LastWill::new(
                        &will.topic,
                        will.payload.clone(),
                        v5_qos(will.qos),
                        will.retain,
                        None,
                    ));
                }
                let (client, eventloop) = rumqttc::v5::AsyncClient::new(opts, 64);
                Ok(Driver::V5(Box::new((client, eventloop))))
            }
        }
    }

    /// Hands one request to the client without waiting. `false` means the
    /// client queue is full: the next poll of the event loop makes room.
    /// The topics are already checked, so a refusal can only mean a full
    /// queue.
    fn try_send(&self, out: &Out) -> bool {
        match (self, out) {
            (Driver::V4(inner), Out::Subscribe(s)) => {
                inner.0.try_subscribe(&s.topic, qos(s.qos)).is_ok()
            }
            (Driver::V4(inner), Out::Publish(p)) => inner
                .0
                .try_publish(&p.topic, qos(p.qos), p.retain, p.payload.clone())
                .is_ok(),
            (Driver::V5(inner), Out::Subscribe(s)) => {
                inner.0.try_subscribe(&s.topic, v5_qos(s.qos)).is_ok()
            }
            (Driver::V5(inner), Out::Publish(p)) => inner
                .0
                .try_publish(&p.topic, v5_qos(p.qos), p.retain, p.payload.clone())
                .is_ok(),
        }
    }

    /// Sends DISCONNECT and polls the event loop until the packet is
    /// written, at most [`CLOSE_WAIT`]. Without the poll the request stays
    /// in the queue and the broker sees an ungraceful close.
    async fn close(&mut self) {
        let _ = tokio::time::timeout(CLOSE_WAIT, async {
            match self {
                Driver::V4(inner) => {
                    if inner.0.disconnect().await.is_err() {
                        return;
                    }
                    while let Ok(event) = inner.1.poll().await {
                        if event == rumqttc::Event::Outgoing(rumqttc::Outgoing::Disconnect) {
                            return;
                        }
                    }
                }
                Driver::V5(inner) => {
                    if inner.0.disconnect().await.is_err() {
                        return;
                    }
                    while let Ok(event) = inner.1.poll().await {
                        if matches!(
                            event,
                            rumqttc::v5::Event::Outgoing(rumqttc::Outgoing::Disconnect)
                        ) {
                            return;
                        }
                    }
                }
            }
        })
        .await;
    }

    async fn poll(&mut self) -> Result<In, String> {
        match self {
            Driver::V4(inner) => {
                let event = inner.1.poll().await.map_err(|e| e.to_string())?;
                Ok(match event {
                    rumqttc::Event::Incoming(rumqttc::Packet::ConnAck(ack)) => In::ConnAck {
                        session_present: ack.session_present,
                    },
                    rumqttc::Event::Incoming(rumqttc::Packet::SubAck(ack)) => In::SubAck {
                        granted: ack
                            .return_codes
                            .iter()
                            .map(|c| match c {
                                rumqttc::SubscribeReasonCode::Success(qos) => *qos as u8,
                                rumqttc::SubscribeReasonCode::Failure => 0x80,
                            })
                            .collect(),
                    },
                    rumqttc::Event::Incoming(rumqttc::Packet::Publish(p)) => In::Message {
                        topic: p.topic,
                        qos: p.qos as u8,
                        retain: p.retain,
                        duplicate: p.dup,
                        payload: p.payload.to_vec(),
                    },
                    rumqttc::Event::Incoming(
                        rumqttc::Packet::PubAck(_)
                        | rumqttc::Packet::PubRec(_)
                        | rumqttc::Packet::PubRel(_)
                        | rumqttc::Packet::PubComp(_),
                    ) => In::Ack,
                    _ => In::Other,
                })
            }
            Driver::V5(inner) => {
                let event = inner.1.poll().await.map_err(|e| e.to_string())?;
                Ok(match event {
                    rumqttc::v5::Event::Incoming(rumqttc::v5::mqttbytes::v5::Packet::ConnAck(
                        ack,
                    )) => In::ConnAck {
                        session_present: ack.session_present,
                    },
                    rumqttc::v5::Event::Incoming(rumqttc::v5::mqttbytes::v5::Packet::SubAck(
                        ack,
                    )) => In::SubAck {
                        granted: ack
                            .return_codes
                            .iter()
                            .map(|c| match c {
                                rumqttc::v5::mqttbytes::v5::SubscribeReasonCode::Success(qos) => {
                                    *qos as u8
                                }
                                _ => 0x80,
                            })
                            .collect(),
                    },
                    rumqttc::v5::Event::Incoming(rumqttc::v5::mqttbytes::v5::Packet::Publish(
                        p,
                    )) => In::Message {
                        topic: String::from_utf8_lossy(&p.topic).into_owned(),
                        qos: p.qos as u8,
                        retain: p.retain,
                        duplicate: p.dup,
                        payload: p.payload.to_vec(),
                    },
                    rumqttc::v5::Event::Incoming(
                        rumqttc::v5::mqttbytes::v5::Packet::PubAck(_)
                        | rumqttc::v5::mqttbytes::v5::Packet::PubRec(_)
                        | rumqttc::v5::mqttbytes::v5::Packet::PubRel(_)
                        | rumqttc::v5::mqttbytes::v5::Packet::PubComp(_),
                    ) => In::Ack,
                    _ => In::Other,
                })
            }
        }
    }
}

/// The longest time a session waits for its DISCONNECT to go out.
const CLOSE_WAIT: Duration = Duration::from_secs(1);

fn qos(qos: u8) -> QoS {
    match qos {
        0 => QoS::AtMostOnce,
        1 => QoS::AtLeastOnce,
        _ => QoS::ExactlyOnce,
    }
}

fn v5_qos(qos: u8) -> rumqttc::v5::mqttbytes::QoS {
    match qos {
        0 => rumqttc::v5::mqttbytes::QoS::AtMostOnce,
        1 => rumqttc::v5::mqttbytes::QoS::AtLeastOnce,
        _ => rumqttc::v5::mqttbytes::QoS::ExactlyOnce,
    }
}

pub(super) async fn open(
    options: BrokerOptions,
    subscriptions: Vec<Subscription>,
    initial: Vec<Publish>,
    deadline: Option<Duration>,
) -> Result<Session, String> {
    let driver = Driver::build(&options)?;
    let (event_tx, events) = mpsc::unbounded_channel();
    let (cmd_tx, cmd) = mpsc::unbounded_channel();
    let start = Instant::now();
    // The task starts with a connected event only after the broker accepts.
    let cancel_tx = event_tx.clone();
    let task = tokio::spawn(async move {
        run(
            driver,
            subscriptions,
            initial,
            deadline,
            event_tx,
            cmd,
            start,
        )
        .await;
    });
    Ok(Session {
        events,
        cmd: Some(cmd_tx),
        event_tx: cancel_tx,
        task,
        start,
    })
}

#[allow(clippy::too_many_arguments)]
async fn run(
    mut driver: Driver,
    subscriptions: Vec<Subscription>,
    initial: Vec<Publish>,
    deadline: Option<Duration>,
    events: mpsc::UnboundedSender<Event>,
    mut cmd: mpsc::UnboundedReceiver<Cmd>,
    start: Instant,
) {
    enum Step {
        Packet(Result<In, String>),
        Cmd(Option<Cmd>),
    }
    let send = |event: Event| {
        let _ = events.send(event);
    };
    let end = |reason: &str, error: Option<String>| Event::Ended {
        reason: reason.into(),
        error,
        at: start.elapsed(),
    };
    let mut waiting = subscriptions.len();
    let mut pending_initial = initial;
    // Requests go to the client with `try_*` and wait here while its queue
    // is full. An `await` on a full queue never returns, because the same
    // task must poll the event loop to empty it.
    let mut queue: VecDeque<Out> = VecDeque::new();
    let deadline_at = deadline.map(|d| tokio::time::Instant::now() + d);
    loop {
        let step = tokio::select! {
            packet = driver.poll() => Step::Packet(packet),
            command = cmd.recv() => Step::Cmd(command),
            _ = sleep_until(deadline_at) => {
                driver.close().await;
                send(end("deadline", None));
                return;
            }
        };
        let connected = match step {
            Step::Packet(Err(error)) => {
                send(end("error", Some(error)));
                return;
            }
            Step::Packet(Ok(In::ConnAck { session_present })) => {
                send(Event::Connected {
                    session_present,
                    at: start.elapsed(),
                });
                queue.extend(subscriptions.iter().cloned().map(Out::Subscribe));
                true
            }
            Step::Packet(Ok(In::SubAck { granted })) => {
                let code = granted.first().copied().unwrap_or(0x80);
                let index = subscriptions.len().saturating_sub(waiting);
                if let Some(subscription) = subscriptions.get(index) {
                    let topic = subscription.topic.clone();
                    let at = start.elapsed();
                    send(if code == 0x80 {
                        Event::SubscribeRefused { topic, at }
                    } else {
                        Event::Subscribed {
                            topic,
                            qos: code.min(subscription.qos),
                            at,
                        }
                    });
                }
                waiting = waiting.saturating_sub(1);
                waiting == 0
            }
            Step::Packet(Ok(In::Message {
                topic,
                qos,
                retain,
                duplicate,
                payload,
            })) => {
                send(Event::Message {
                    topic,
                    qos,
                    retain,
                    duplicate,
                    payload,
                    at: start.elapsed(),
                });
                false
            }
            Step::Packet(Ok(In::Ack | In::Other)) => false,
            Step::Cmd(Some(Cmd::Publish(publish))) => {
                queue.push_back(Out::Publish(publish));
                false
            }
            Step::Cmd(Some(Cmd::Disconnect)) => {
                driver.close().await;
                send(end("disconnected", None));
                return;
            }
            Step::Cmd(None) => {
                driver.close().await;
                send(end("cancelled", None));
                return;
            }
        };
        if connected && waiting == 0 {
            queue.extend(
                std::mem::take(&mut pending_initial)
                    .into_iter()
                    .map(Out::Publish),
            );
        }
        while let Some(out) = queue.front() {
            if !driver.try_send(out) {
                break;
            }
            if let Some(Out::Publish(publish)) = queue.pop_front() {
                send(Event::Published {
                    topic: publish.topic,
                    qos: publish.qos,
                    at: start.elapsed(),
                });
            }
        }
    }
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}
