//! A minimal MQTT broker that records every packet, for v3.1.1 and v5.
//!
//! It answers CONNECT, SUBSCRIBE, UNSUBSCRIBE, PUBLISH, PINGREQ and DISCONNECT,
//! forwards publishes to matching subscriptions (wildcards included), keeps
//! retained messages and publishes the Last Will on an ungraceful close.
//! A client that connects with the user name `bad` gets a refused CONNACK,
//! and a subscription to a filter under `denied/` gets a refused SUBACK.
//! [`MqttBroker::start_tls`] serves the same broker over TLS (`mqtts://`).

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::mpsc;

/// One packet the broker received or sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    /// `>` sent by the broker, `<` received from a client.
    pub out: bool,
    pub name: &'static str,
    pub detail: String,
}

/// One client session as the broker saw it.
#[derive(Debug, Clone, Default)]
pub struct Captured {
    pub protocol: String,
    pub client_id: String,
    pub packets: Vec<Packet>,
}

impl Captured {
    /// One line per packet: direction, name, detail.
    pub fn dump(&self) -> String {
        let mut s = format!("protocol {} client-id {}\n", self.protocol, self.client_id);
        for p in &self.packets {
            let dir = if p.out { '>' } else { '<' };
            s.push_str(&format!("{dir} {} {}\n", p.name, p.detail));
        }
        s
    }
}

struct Subscriber {
    id: u64,
    filter: String,
    qos: u8,
    tx: mpsc::UnboundedSender<Delivery>,
}

#[derive(Debug, Clone)]
struct Delivery {
    topic: String,
    payload: Vec<u8>,
    qos: u8,
    retain: bool,
}

#[derive(Default)]
struct State {
    next_id: u64,
    subscribers: Vec<Subscriber>,
    retained: HashMap<String, (Vec<u8>, u8)>,
}

pub struct MqttBroker {
    pub addr: SocketAddr,
    pub captures: Arc<Mutex<Vec<Captured>>>,
    state: Arc<Mutex<State>>,
    secure: bool,
    task: tokio::task::JoinHandle<()>,
}

impl MqttBroker {
    pub async fn start() -> std::io::Result<Self> {
        Self::listen(None).await
    }

    /// Starts the broker over TLS with the test certificate.
    pub async fn start_tls(
        certs: &crate::tls::TestCerts,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let cfg = crate::tls::server_config(certs, false, false)?;
        Ok(Self::listen(Some(tokio_rustls::TlsAcceptor::from(Arc::new(cfg)))).await?)
    }

    async fn listen(tls: Option<tokio_rustls::TlsAcceptor>) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let captures = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(Mutex::new(State::default()));
        let secure = tls.is_some();
        let task = {
            let captures = captures.clone();
            let state = state.clone();
            tokio::spawn(async move {
                while let Ok((stream, _)) = listener.accept().await {
                    let captures = captures.clone();
                    let state = state.clone();
                    let tls = tls.clone();
                    tokio::spawn(async move {
                        match tls {
                            Some(acceptor) => {
                                if let Ok(stream) = acceptor.accept(stream).await {
                                    let _ = serve(stream, captures, state).await;
                                }
                            }
                            None => {
                                let _ = serve(stream, captures, state).await;
                            }
                        }
                    });
                }
            })
        };
        Ok(Self {
            addr,
            captures,
            state,
            secure,
            task,
        })
    }

    pub fn url(&self) -> String {
        let scheme = if self.secure { "mqtts" } else { "mqtt" };
        format!("{scheme}://127.0.0.1:{}", self.addr.port())
    }

    pub fn take(&self) -> Vec<Captured> {
        std::mem::take(&mut *self.captures.lock().unwrap_or_else(|e| e.into_inner()))
    }

    /// The retained message on a topic, for assertions.
    pub fn retained(&self, topic: &str) -> Option<(Vec<u8>, u8)> {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retained
            .get(topic)
            .cloned()
    }
}

impl Drop for MqttBroker {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn read_packet<S: AsyncRead + Unpin>(
    stream: &mut S,
) -> std::io::Result<Option<(u8, Vec<u8>)>> {
    let mut first = [0u8; 1];
    match stream.read_exact(&mut first).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let mut remaining = 0usize;
    let mut shift = 0;
    loop {
        let mut byte = [0u8; 1];
        stream.read_exact(&mut byte).await?;
        remaining |= ((byte[0] & 0x7f) as usize) << shift;
        if byte[0] & 0x80 == 0 {
            break;
        }
        shift += 7;
        if shift > 21 {
            return Err(std::io::Error::other("malformed remaining length"));
        }
    }
    let mut body = vec![0u8; remaining];
    stream.read_exact(&mut body).await?;
    Ok(Some((first[0], body)))
}

/// A payload in a capture line: the text, or `hex ..` when it is not
/// printable UTF-8.
fn show(payload: &[u8]) -> String {
    match std::str::from_utf8(payload) {
        Ok(text) if !text.chars().any(char::is_control) => text.to_string(),
        _ => {
            let hex: Vec<String> = payload.iter().map(|b| format!("{b:02x}")).collect();
            format!("hex {}", hex.join(" "))
        }
    }
}

fn encode_len(mut len: usize, out: &mut Vec<u8>) {
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
}

fn packet(first: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![first];
    encode_len(body.len(), &mut out);
    out.extend_from_slice(body);
    out
}

fn read_str(body: &[u8], at: &mut usize) -> Option<String> {
    if *at + 2 > body.len() {
        return None;
    }
    let len = u16::from_be_bytes([body[*at], body[*at + 1]]) as usize;
    *at += 2;
    if *at + len > body.len() {
        return None;
    }
    let text = String::from_utf8_lossy(&body[*at..*at + len]).into_owned();
    *at += len;
    Some(text)
}

fn matches(filter: &str, topic: &str) -> bool {
    let f: Vec<&str> = filter.split('/').collect();
    let t: Vec<&str> = topic.split('/').collect();
    let mut i = 0;
    while i < f.len() {
        match f[i] {
            "#" => return true,
            "+" => {
                if i >= t.len() {
                    return false;
                }
            }
            part => {
                if t.get(i) != Some(&part) {
                    return false;
                }
            }
        }
        i += 1;
    }
    i == t.len()
}

async fn serve<S: AsyncRead + AsyncWrite + Unpin>(
    mut stream: S,
    captures: Arc<Mutex<Vec<Captured>>>,
    state: Arc<Mutex<State>>,
) -> std::io::Result<()> {
    let mut capture = Captured::default();
    let mut protocol = 4u8;
    let mut will: Option<Delivery> = None;
    let mut clean_disconnect = false;
    let (tx, mut rx) = mpsc::unbounded_channel::<Delivery>();
    let mut my_id = 0u64;
    let mut next_pkid = 1u16;
    loop {
        // Deliveries first: a forwarded publish goes out before the next
        // packet is read, so the capture order does not depend on timing.
        let (first, body) = tokio::select! {
            biased;
            Some(delivery) = rx.recv() => {
                let mut body = vec![];
                body.extend_from_slice(&(delivery.topic.len() as u16).to_be_bytes());
                body.extend_from_slice(delivery.topic.as_bytes());
                if delivery.qos > 0 {
                    body.extend_from_slice(&next_pkid.to_be_bytes());
                    next_pkid = next_pkid.wrapping_add(1).max(1);
                }
                if protocol == 5 {
                    body.push(0);
                }
                body.extend_from_slice(&delivery.payload);
                let mut flags = 0x30 | (delivery.qos << 1);
                if delivery.retain {
                    flags |= 0x01;
                }
                let out = packet(flags, &body);
                stream.write_all(&out).await?;
                capture.packets.push(Packet {
                    out: true,
                    name: "PUBLISH",
                    detail: format!(
                        "topic {} qos {} retain {} payload {}",
                        delivery.topic,
                        delivery.qos,
                        delivery.retain,
                        show(&delivery.payload)
                    ),
                });
                continue;
            }
            packet = read_packet(&mut stream) => match packet? {
                Some(p) => p,
                None => break,
            },
        };
        let kind = first >> 4;
        match kind {
            1 => {
                // CONNECT
                let mut at = 0;
                let _name = read_str(&body, &mut at).unwrap_or_default();
                protocol = body.get(at).copied().unwrap_or(4);
                at += 1;
                let flags = body.get(at).copied().unwrap_or(0);
                at += 1;
                let keep_alive = if at + 2 <= body.len() {
                    u16::from_be_bytes([body[at], body[at + 1]])
                } else {
                    0
                };
                at += 2;
                if protocol == 5 {
                    // Properties length (varint) and properties: skip.
                    let mut len = 0usize;
                    let mut shift = 0;
                    while let Some(byte) = body.get(at).copied() {
                        at += 1;
                        len |= ((byte & 0x7f) as usize) << shift;
                        if byte & 0x80 == 0 {
                            break;
                        }
                        shift += 7;
                    }
                    at += len;
                }
                let client_id = read_str(&body, &mut at).unwrap_or_default();
                if flags & 0x04 != 0 {
                    // Last Will: topic, payload (v5 has properties before topic).
                    if protocol == 5 {
                        let mut len = 0usize;
                        let mut shift = 0;
                        while let Some(byte) = body.get(at).copied() {
                            at += 1;
                            len |= ((byte & 0x7f) as usize) << shift;
                            if byte & 0x80 == 0 {
                                break;
                            }
                            shift += 7;
                        }
                        at += len;
                    }
                    let topic = read_str(&body, &mut at).unwrap_or_default();
                    let plen = if at + 2 <= body.len() {
                        u16::from_be_bytes([body[at], body[at + 1]]) as usize
                    } else {
                        0
                    };
                    at += 2;
                    let payload = body
                        .get(at..at + plen)
                        .map(|s| s.to_vec())
                        .unwrap_or_default();
                    will = Some(Delivery {
                        topic,
                        payload,
                        qos: (flags >> 3) & 0x03,
                        retain: flags & 0x20 != 0,
                    });
                }
                let (user, bad) = if flags & 0x80 != 0 {
                    let user = read_str(&body, &mut at).unwrap_or_default();
                    let bad = user == "bad";
                    (user, bad)
                } else {
                    (String::new(), false)
                };
                capture.protocol = if protocol == 5 {
                    "5".into()
                } else {
                    "3.1.1".into()
                };
                capture.client_id = client_id.clone();
                capture.packets.push(Packet {
                    out: false,
                    name: "CONNECT",
                    detail: format!(
                        "protocol {} client-id {} clean {} keep-alive {} user {} will {}",
                        capture.protocol,
                        client_id,
                        flags & 0x02 != 0,
                        keep_alive,
                        user,
                        will.as_ref()
                            .map(|w| format!("{} {}", w.topic, show(&w.payload)))
                            .unwrap_or_default()
                    ),
                });
                let ack = if protocol == 5 {
                    packet(0x20, &[0x00, if bad { 0x86 } else { 0x00 }, 0x00])
                } else {
                    packet(0x20, &[0x00, if bad { 0x05 } else { 0x00 }])
                };
                stream.write_all(&ack).await?;
                capture.packets.push(Packet {
                    out: true,
                    name: "CONNACK",
                    detail: if bad {
                        "code 134".into()
                    } else {
                        "code 0".into()
                    },
                });
                if bad {
                    break;
                }
                my_id = {
                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                    state.next_id += 1;
                    state.next_id
                };
            }
            8 => {
                // SUBSCRIBE
                let mut at = 0;
                let pkid = if body.len() >= 2 {
                    u16::from_be_bytes([body[0], body[1]])
                } else {
                    0
                };
                at += 2;
                if protocol == 5 {
                    let mut len = 0usize;
                    let mut shift = 0;
                    while let Some(byte) = body.get(at).copied() {
                        at += 1;
                        len |= ((byte & 0x7f) as usize) << shift;
                        if byte & 0x80 == 0 {
                            break;
                        }
                        shift += 7;
                    }
                    at += len;
                }
                let mut granted = vec![];
                let mut filters: Vec<(String, u8)> = vec![];
                while at < body.len() {
                    let Some(filter) = read_str(&body, &mut at) else {
                        break;
                    };
                    let qos = body.get(at).copied().unwrap_or(0) & 0x03;
                    at += 1;
                    capture.packets.push(Packet {
                        out: false,
                        name: "SUBSCRIBE",
                        detail: format!("topic {filter} qos {qos}"),
                    });
                    // A filter under `denied/` is refused, as a broker ACL does.
                    if filter.starts_with("denied/") {
                        granted.push(0x80);
                    } else {
                        filters.push((filter, qos));
                        granted.push(qos);
                    }
                }
                {
                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                    for (filter, qos) in &filters {
                        state.subscribers.push(Subscriber {
                            id: my_id,
                            filter: filter.clone(),
                            qos: *qos,
                            tx: tx.clone(),
                        });
                    }
                }
                let mut ack = pkid.to_be_bytes().to_vec();
                if protocol == 5 {
                    ack.push(0);
                }
                ack.extend_from_slice(&granted);
                stream.write_all(&packet(0x90, &ack)).await?;
                capture.packets.push(Packet {
                    out: true,
                    name: "SUBACK",
                    detail: format!("granted {granted:?}"),
                });
                // Retained messages that match the new filters.
                let retained: Vec<_> = {
                    let state = state.lock().unwrap_or_else(|e| e.into_inner());
                    state
                        .retained
                        .iter()
                        .map(|(t, (p, q))| (t.clone(), p.clone(), *q))
                        .collect()
                };
                for (topic, payload, qos) in retained {
                    if let Some((_, sub_qos)) = filters
                        .iter()
                        .filter(|(filter, _)| matches(filter, &topic))
                        .map(|(_, q)| (q, q))
                        .next()
                    {
                        let _ = tx.send(Delivery {
                            topic,
                            payload,
                            qos: (*sub_qos).min(qos),
                            retain: true,
                        });
                    }
                }
            }
            3 => {
                // PUBLISH
                let qos = (first >> 1) & 0x03;
                let retain = first & 0x01 != 0;
                let mut at = 0;
                let topic = read_str(&body, &mut at).unwrap_or_default();
                let pkid = if qos > 0 {
                    let id = if at + 2 <= body.len() {
                        u16::from_be_bytes([body[at], body[at + 1]])
                    } else {
                        0
                    };
                    at += 2;
                    Some(id)
                } else {
                    None
                };
                if protocol == 5 {
                    let mut len = 0usize;
                    let mut shift = 0;
                    while let Some(byte) = body.get(at).copied() {
                        at += 1;
                        len |= ((byte & 0x7f) as usize) << shift;
                        if byte & 0x80 == 0 {
                            break;
                        }
                        shift += 7;
                    }
                    at += len;
                }
                let payload = body.get(at..).map(|s| s.to_vec()).unwrap_or_default();
                capture.packets.push(Packet {
                    out: false,
                    name: "PUBLISH",
                    detail: format!(
                        "topic {} qos {} retain {} payload {}",
                        topic,
                        qos,
                        retain,
                        show(&payload)
                    ),
                });
                if retain {
                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                    if payload.is_empty() {
                        state.retained.remove(&topic);
                    } else {
                        state.retained.insert(topic.clone(), (payload.clone(), qos));
                    }
                }
                if let Some(id) = pkid {
                    let ack = if qos == 1 { 0x40 } else { 0x50 };
                    let mut body = id.to_be_bytes().to_vec();
                    if protocol == 5 {
                        body.push(0);
                        body.push(0);
                    }
                    stream.write_all(&packet(ack, &body)).await?;
                    capture.packets.push(Packet {
                        out: true,
                        name: if qos == 1 { "PUBACK" } else { "PUBREC" },
                        detail: format!("pkid {id}"),
                    });
                }
                let targets: Vec<_> = {
                    let state = state.lock().unwrap_or_else(|e| e.into_inner());
                    state
                        .subscribers
                        .iter()
                        .filter(|s| matches(&s.filter, &topic))
                        .map(|s| (s.tx.clone(), s.qos.min(qos)))
                        .collect()
                };
                for (tx, sub_qos) in targets {
                    let _ = tx.send(Delivery {
                        topic: topic.clone(),
                        payload: payload.clone(),
                        qos: sub_qos,
                        retain: false,
                    });
                }
            }
            6 => {
                // PUBREL
                let pkid = if body.len() >= 2 {
                    u16::from_be_bytes([body[0], body[1]])
                } else {
                    0
                };
                let mut ack = pkid.to_be_bytes().to_vec();
                if protocol == 5 {
                    ack.push(0);
                    ack.push(0);
                }
                stream.write_all(&packet(0x70, &ack)).await?;
                capture.packets.push(Packet {
                    out: true,
                    name: "PUBCOMP",
                    detail: format!("pkid {pkid}"),
                });
            }
            10 => {
                // UNSUBSCRIBE
                let mut at = 0;
                let pkid = if body.len() >= 2 {
                    u16::from_be_bytes([body[0], body[1]])
                } else {
                    0
                };
                at += 2;
                let mut filters = vec![];
                while at < body.len() {
                    let Some(filter) = read_str(&body, &mut at) else {
                        break;
                    };
                    filters.push(filter);
                    if protocol == 5 {
                        break;
                    }
                }
                {
                    let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
                    state
                        .subscribers
                        .retain(|s| !(s.id == my_id && filters.contains(&s.filter)));
                }
                let mut ack = pkid.to_be_bytes().to_vec();
                if protocol == 5 {
                    ack.push(0);
                    ack.extend(std::iter::repeat_n(0u8, filters.len()));
                }
                stream.write_all(&packet(0xb0, &ack)).await?;
                capture.packets.push(Packet {
                    out: true,
                    name: "UNSUBACK",
                    detail: format!("filters {filters:?}"),
                });
            }
            12 => {
                // PINGREQ
                stream.write_all(&[0xd0, 0x00]).await?;
                capture.packets.push(Packet {
                    out: true,
                    name: "PINGRESP",
                    detail: String::new(),
                });
            }
            14 => {
                capture.packets.push(Packet {
                    out: false,
                    name: "DISCONNECT",
                    detail: String::new(),
                });
                clean_disconnect = true;
                break;
            }
            _ => {}
        }
    }
    {
        let mut state = state.lock().unwrap_or_else(|e| e.into_inner());
        state.subscribers.retain(|s| s.id != my_id);
    }
    if !clean_disconnect && let Some(will) = will {
        let targets: Vec<_> = {
            let state = state.lock().unwrap_or_else(|e| e.into_inner());
            state
                .subscribers
                .iter()
                .filter(|s| matches(&s.filter, &will.topic))
                .map(|s| (s.tx.clone(), s.qos.min(will.qos)))
                .collect()
        };
        for (tx, qos) in targets {
            let _ = tx.send(Delivery {
                topic: will.topic.clone(),
                payload: will.payload.clone(),
                qos,
                retain: will.retain,
            });
        }
        capture.packets.push(Packet {
            out: false,
            name: "WILL",
            detail: format!("topic {}", will.topic),
        });
    }
    captures
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push(capture);
    Ok(())
}

/// Waits until the broker recorded a session that satisfies `check`.
pub async fn wait_for<F: Fn(&[Captured]) -> bool>(broker: &MqttBroker, check: F) -> Vec<Captured> {
    for _ in 0..200 {
        {
            let captures = broker.captures.lock().unwrap_or_else(|e| e.into_inner());
            if check(&captures) {
                return captures.clone();
            }
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    broker.take()
}
