//! MQTT sessions over TCP or TLS: protocol v3.1.1 and v5, QoS 0 to 2,
//! subscriptions with wildcards, retained publishes and an optional Last Will.
//!
//! One session per request: the client connects, subscribes, publishes the
//! initial messages and then reports every incoming message until the deadline,
//! a disconnect or a cancel. There is no automatic reconnect (see `DEBT.md`).
//! TLS uses the same [`sendito_http::tls_config`] as HTTP, gRPC and WebSocket.

use std::time::{Duration, Instant};

use sendito_http::TlsMaterial;
use tokio::sync::mpsc;

mod run;
use run::Cmd;

/// The MQTT protocol version of one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    #[default]
    V3_1_1,
    V5,
}

impl Protocol {
    pub const fn as_str(self) -> &'static str {
        match self {
            Protocol::V3_1_1 => "3.1.1",
            Protocol::V5 => "5",
        }
    }
    /// Parses `3.1.1` or `5`; anything else is a validation error.
    pub fn parse(text: &str) -> Result<Self, String> {
        match text.trim() {
            "3.1.1" | "3.1" | "" => Ok(Protocol::V3_1_1),
            "5" | "5.0" => Ok(Protocol::V5),
            other => Err(format!(
                "protocol must be \"3.1.1\" or \"5\", not \"{other}\""
            )),
        }
    }
}

/// One subscription: a topic filter and the maximum QoS the broker may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subscription {
    pub topic: String,
    pub qos: u8,
}

/// One publish: topic, payload, QoS and the retain flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Publish {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
}

/// The Last Will the broker publishes when the connection drops ungracefully.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Will {
    pub topic: String,
    pub payload: Vec<u8>,
    pub qos: u8,
    pub retain: bool,
}

/// Everything a session needs to connect and identify itself.
#[derive(Debug, Clone)]
pub struct BrokerOptions {
    pub host: String,
    pub port: u16,
    pub tls: bool,
    pub client_id: String,
    pub username: Option<String>,
    pub password: Option<String>,
    pub protocol: Protocol,
    /// `false` asks the broker to keep the session (v3.1.1 `clean session`,
    /// v5 `clean start`).
    pub clean_session: bool,
    pub keep_alive_s: u64,
    pub verify_tls: bool,
    pub tls_material: TlsMaterial,
    pub last_will: Option<Will>,
}

impl BrokerOptions {
    /// Checks the values the protocol cannot carry, before any connection.
    pub fn validate(&self) -> Result<(), String> {
        if self.host.trim().is_empty() {
            return Err("the broker host is empty".into());
        }
        if self.port == 0 {
            return Err("the broker port is 0".into());
        }
        if self.client_id.len() > 65_535 {
            return Err("the client ID is longer than 65535 bytes".into());
        }
        if let Some(will) = &self.last_will {
            check_topic(&will.topic, "the Last Will topic")?;
            check_qos(will.qos, "the Last Will")?;
        }
        if self.tls_material.pair_error().is_some() {
            return Err(self.tls_material.pair_error().unwrap_or_default());
        }
        Ok(())
    }
}

fn check_qos(qos: u8, what: &str) -> Result<(), String> {
    if qos > 2 {
        return Err(format!("{what} QoS must be 0, 1 or 2, not {qos}"));
    }
    Ok(())
}

/// Checks a subscription filter or a publish topic.
pub fn check_topic(topic: &str, what: &str) -> Result<(), String> {
    if topic.is_empty() {
        return Err(format!("{what} is empty"));
    }
    if topic.len() > 65_535 {
        return Err(format!("{what} is longer than 65535 bytes"));
    }
    if topic.contains('\u{0}') {
        return Err(format!("{what} contains a NUL byte"));
    }
    Ok(())
}

/// Checks one subscription before the connection opens.
pub fn check_subscription(s: &Subscription) -> Result<(), String> {
    check_topic(&s.topic, "a subscription topic filter")?;
    if !rumqttc::valid_filter(&s.topic) {
        return Err(format!(
            "the topic filter \"{}\" is not valid: + and # must fill a whole level, and # must be the last level",
            s.topic
        ));
    }
    check_qos(s.qos, "the subscription")
}

/// Checks one publish before it goes out.
pub fn check_publish(p: &Publish) -> Result<(), String> {
    check_topic(&p.topic, "the publish topic")?;
    check_qos(p.qos, "the publish")?;
    if p.topic.contains(['+', '#']) {
        return Err("a publish topic must not contain the wildcards + or #".into());
    }
    Ok(())
}

/// One event of a running session, in arrival order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Connected {
        session_present: bool,
        at: Duration,
    },
    Subscribed {
        topic: String,
        qos: u8,
        at: Duration,
    },
    /// The broker refused the subscription (SUBACK failure code). The
    /// session continues.
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
    Ended {
        reason: String,
        error: Option<String>,
        at: Duration,
    },
}

impl Event {
    pub const fn label(&self) -> &'static str {
        match self {
            Event::Connected { .. } => "connected",
            Event::Subscribed { .. } => "subscribed",
            Event::SubscribeRefused { .. } => "subscribe refused",
            Event::Message { .. } => "message",
            Event::Published { .. } => "published",
            Event::Ended { .. } => "ended",
        }
    }
}

/// A running session. Drop or [`Session::cancel`] to abort it.
pub struct Session {
    pub events: mpsc::UnboundedReceiver<Event>,
    cmd: Option<mpsc::UnboundedSender<Cmd>>,
    event_tx: mpsc::UnboundedSender<Event>,
    task: tokio::task::JoinHandle<()>,
    start: Instant,
}

impl Session {
    /// Whether the session task is still alive.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    /// Sends one publish. QoS 1 and 2 are acknowledged by the broker after
    /// this returns; the [`Event::Published`] event reports the hand-off.
    pub fn publish(&self, publish: Publish) -> Result<(), String> {
        check_publish(&publish)?;
        self.cmd
            .as_ref()
            .ok_or_else(|| "the session is closed".to_string())?
            .send(Cmd::Publish(publish))
            .map_err(|_| "the session has ended".to_string())
    }

    /// Sends DISCONNECT and lets the session end cleanly.
    pub fn disconnect(&mut self) {
        if let Some(tx) = self.cmd.take() {
            let _ = tx.send(Cmd::Disconnect);
        }
    }

    /// Aborts the session without DISCONNECT; the broker sends the Last Will.
    pub fn cancel(&mut self) {
        self.cmd = None;
        if !self.task.is_finished() {
            self.task.abort();
            let _ = self.event_tx.send(Event::Ended {
                reason: "cancelled".into(),
                error: None,
                at: self.start.elapsed(),
            });
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Opens a session. The task subscribes, publishes the initial messages after
/// the SUBACK of every subscription, and reports events until it ends.
pub async fn open(
    options: BrokerOptions,
    subscriptions: Vec<Subscription>,
    initial: Vec<Publish>,
    deadline: Option<Duration>,
) -> Result<Session, String> {
    options.validate()?;
    for s in &subscriptions {
        check_subscription(s)?;
    }
    for p in &initial {
        check_publish(p)?;
    }
    run::open(options, subscriptions, initial, deadline).await
}
