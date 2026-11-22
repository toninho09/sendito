//! Dynamic gRPC client built on tonic and prost-reflect.
//!
//! Every call type goes through one bidirectional code path: unary is a stream
//! with one message each way, which is exactly how gRPC frames it on the wire.

pub mod schema;

use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use prost::Message;
use prost_reflect::{DynamicMessage, MessageDescriptor, MethodDescriptor};
use tokio::sync::mpsc;
use tonic::Status;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::metadata::{
    AsciiMetadataKey, AsciiMetadataValue, BinaryMetadataKey, BinaryMetadataValue, MetadataMap,
};
use tonic::transport::{Channel, Endpoint};

pub use schema::{CallKind, MethodInfo, Schema, ServiceInfo};

#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    #[error("invalid target {0:?}")]
    Target(String),
    #[error("cannot connect to {target}: {msg}")]
    Connect { target: String, msg: String },
    #[error("schema: {0}")]
    Schema(String),
    #[error("reflection failed: {0}")]
    Reflection(String),
    #[error("invalid metadata {key:?}: {msg}")]
    Metadata { key: String, msg: String },
    #[error("{0}")]
    Message(String),
}

/// Connection settings for a target.
#[derive(Debug, Clone)]
pub struct ConnSettings {
    /// `http://host:port` or `https://host:port`.
    pub uri: String,
    pub verify_tls: bool,
    pub connect_timeout: Duration,
    /// `[http] tls` from the config (custom CA, client certificate).
    pub tls: sendito_http::TlsMaterial,
}

/// Opens a channel. TLS (with ALPN `h2`) is used for `https://` URIs.
pub async fn connect(s: &ConnSettings) -> Result<Channel, GrpcError> {
    let endpoint = Endpoint::from_shared(s.uri.clone())
        .map_err(|_| GrpcError::Target(s.uri.clone()))?
        .connect_timeout(s.connect_timeout)
        .user_agent(concat!("sendito/", env!("CARGO_PKG_VERSION")))
        .map_err(|_| GrpcError::Target(s.uri.clone()))?;
    // ALPN is left empty: hyper-rustls sets it from the enabled schemes
    // (h2 + http/1.1 here) and asserts the config's list is empty.
    let tls =
        sendito_http::tls_config(s.verify_tls, &[], &s.tls).map_err(|e| GrpcError::Connect {
            target: s.uri.clone(),
            msg: e,
        })?;
    let mut http = hyper_util::client::legacy::connect::HttpConnector::new();
    http.enforce_http(false);
    http.set_nodelay(true);
    let connector = hyper_rustls::HttpsConnectorBuilder::new()
        .with_tls_config(tls)
        .https_or_http()
        .enable_http2()
        .wrap_connector(http);
    endpoint
        .connect_with_connector(connector)
        .await
        .map_err(|e| GrpcError::Connect {
            target: s.uri.clone(),
            msg: error_chain(&e),
        })
}

fn error_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        let t = s.to_string();
        if !parts.iter().any(|p| p.contains(&t)) {
            parts.push(t);
        }
        cur = s.source();
    }
    parts.join(": ")
}

/// Codec encoding and decoding `DynamicMessage`s.
#[derive(Clone)]
pub struct DynCodec {
    output: MessageDescriptor,
}

impl DynCodec {
    pub fn new(output: MessageDescriptor) -> Self {
        Self { output }
    }
}

pub struct DynEncoder;
pub struct DynDecoder(MessageDescriptor);

impl Codec for DynCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynEncoder;
    type Decoder = DynDecoder;

    fn encoder(&mut self) -> Self::Encoder {
        DynEncoder
    }

    fn decoder(&mut self) -> Self::Decoder {
        DynDecoder(self.output.clone())
    }
}

impl Encoder for DynEncoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn encode(&mut self, item: Self::Item, dst: &mut EncodeBuf<'_>) -> Result<(), Self::Error> {
        item.encode(dst)
            .map_err(|e| Status::internal(format!("encode: {e}")))
    }
}

impl Decoder for DynDecoder {
    type Item = DynamicMessage;
    type Error = Status;

    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<Self::Item>, Self::Error> {
        DynamicMessage::decode(self.0.clone(), src)
            .map(Some)
            .map_err(|e| Status::internal(format!("decode: {e}")))
    }
}

/// Builds tonic metadata. Keys ending in `-bin` take base64 values.
pub fn metadata_map(pairs: &[(String, String)]) -> Result<MetadataMap, GrpcError> {
    let mut map = MetadataMap::new();
    for (k, v) in pairs {
        let key = k.to_ascii_lowercase();
        let err = |msg: String| GrpcError::Metadata {
            key: k.clone(),
            msg,
        };
        if key.ends_with("-bin") {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(v.trim())
                .or_else(|_| base64::engine::general_purpose::STANDARD_NO_PAD.decode(v.trim()))
                .map_err(|e| err(format!("binary metadata must be base64: {e}")))?;
            let mk =
                BinaryMetadataKey::from_bytes(key.as_bytes()).map_err(|e| err(e.to_string()))?;
            map.append_bin(mk, BinaryMetadataValue::from_bytes(&bytes));
        } else {
            let mk =
                AsciiMetadataKey::from_bytes(key.as_bytes()).map_err(|e| err(e.to_string()))?;
            if !v.bytes().all(|b| (0x20..=0x7e).contains(&b)) {
                return Err(err(
                    "value must be printable ASCII (use a -bin key for binary data)".into(),
                ));
            }
            let mv: AsciiMetadataValue = v.parse().map_err(|_| {
                err("value must be printable ASCII (use a -bin key for binary data)".into())
            })?;
            map.append(mk, mv);
        }
    }
    Ok(map)
}

/// Readable metadata pairs (binary values shown as base64).
pub fn metadata_pairs(map: &MetadataMap) -> Vec<(String, String)> {
    map.iter()
        .map(|kv| match kv {
            tonic::metadata::KeyAndValueRef::Ascii(k, v) => (
                k.as_str().to_string(),
                v.to_str()
                    .map(str::to_string)
                    .unwrap_or_else(|_| format!("{v:?}")),
            ),
            tonic::metadata::KeyAndValueRef::Binary(k, v) => (
                k.as_str().to_string(),
                base64::engine::general_purpose::STANDARD.encode(v.to_bytes().unwrap_or_default()),
            ),
        })
        .collect()
}

pub fn code_name(code: i32) -> &'static str {
    match code {
        0 => "OK",
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        4 => "DEADLINE_EXCEEDED",
        5 => "NOT_FOUND",
        6 => "ALREADY_EXISTS",
        7 => "PERMISSION_DENIED",
        8 => "RESOURCE_EXHAUSTED",
        9 => "FAILED_PRECONDITION",
        10 => "ABORTED",
        11 => "OUT_OF_RANGE",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        15 => "DATA_LOSS",
        16 => "UNAUTHENTICATED",
        _ => "UNKNOWN_CODE",
    }
}

/// Something that happened during a call.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A request message was handed to the transport.
    Sent {
        json: String,
        at: Duration,
        wall: SystemTime,
    },
    /// The client half-closed its side of the stream.
    SendClosed { at: Duration },
    /// Response headers (initial metadata).
    Headers {
        metadata: Vec<(String, String)>,
        at: Duration,
    },
    /// A response message: `json` is pretty with the fields that were on the
    /// wire (proto3 JSON: default values omitted), `full` is pretty with every
    /// field (scripts read it, so `$.count == 0` works), `compact` is one line
    /// without default values (for logs).
    Received {
        json: String,
        full: String,
        compact: String,
        size: usize,
        at: Duration,
        wall: SystemTime,
    },
    /// The call finished with a status (from trailers, an error, a deadline or a cancel).
    Finished {
        code: i32,
        message: String,
        metadata: Vec<(String, String)>,
        at: Duration,
    },
}

/// A running call. Drop or [`Call::cancel`] to abort it.
pub struct Call {
    pub events: mpsc::UnboundedReceiver<Event>,
    pub kind: CallKind,
    input: MessageDescriptor,
    sender: Option<mpsc::UnboundedSender<DynamicMessage>>,
    event_tx: mpsc::UnboundedSender<Event>,
    task: tokio::task::JoinHandle<()>,
    start: Instant,
}

impl Call {
    pub fn input(&self) -> &MessageDescriptor {
        &self.input
    }

    /// Whether more request messages can be sent.
    pub fn can_send(&self) -> bool {
        self.sender.is_some()
    }

    /// Queues request messages (client-streaming and bidi calls).
    pub fn send(&mut self, msgs: Vec<DynamicMessage>) -> Result<(), GrpcError> {
        let Some(tx) = &self.sender else {
            return Err(GrpcError::Message("the request stream is closed".into()));
        };
        for m in msgs {
            let json = schema::to_compact(&m);
            tx.send(m)
                .map_err(|_| GrpcError::Message("the call has ended".into()))?;
            let _ = self.event_tx.send(Event::Sent {
                json,
                at: self.start.elapsed(),
                wall: SystemTime::now(),
            });
        }
        Ok(())
    }

    /// Half-closes the request stream.
    pub fn close_send(&mut self) {
        if self.sender.take().is_some() {
            let _ = self.event_tx.send(Event::SendClosed {
                at: self.start.elapsed(),
            });
        }
    }

    /// Aborts the call (the stream is reset with CANCEL).
    pub fn cancel(&mut self) {
        if !self.task.is_finished() {
            self.task.abort();
            self.sender = None;
            let _ = self.event_tx.send(Event::Finished {
                code: 1,
                message: "cancelled by user".into(),
                metadata: vec![],
                at: self.start.elapsed(),
            });
        }
    }

    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }
}

impl Drop for Call {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts a call. `initial` messages are sent first; for unary and
/// server-streaming calls the request side is then closed automatically.
pub fn start_call(
    channel: Channel,
    method: &MethodDescriptor,
    metadata: MetadataMap,
    deadline: Option<Duration>,
    initial: Vec<DynamicMessage>,
) -> Call {
    let kind = CallKind::of(method);
    let (tx, rx) = mpsc::unbounded_channel::<DynamicMessage>();
    let (etx, erx) = mpsc::unbounded_channel::<Event>();
    let start = Instant::now();
    let path = format!("/{}/{}", method.parent_service().full_name(), method.name());
    let codec = DynCodec::new(method.output());
    let events = etx.clone();
    let task = tokio::spawn(async move {
        let fut = run(
            channel,
            path,
            codec,
            metadata,
            deadline,
            rx,
            events.clone(),
            start,
        );
        let finished = match deadline {
            Some(d) => match tokio::time::timeout(d, fut).await {
                Ok(f) => f,
                Err(_) => Event::Finished {
                    code: 4,
                    message: format!("deadline of {} ms exceeded", d.as_millis()),
                    metadata: vec![],
                    at: start.elapsed(),
                },
            },
            None => fut.await,
        };
        let _ = events.send(finished);
    });
    let mut call = Call {
        events: erx,
        kind,
        input: method.input(),
        sender: Some(tx),
        event_tx: etx,
        task,
        start,
    };
    let _ = call.send(initial);
    if !kind.client_streams() {
        call.close_send();
    }
    call
}

#[allow(clippy::too_many_arguments)]
async fn run(
    channel: Channel,
    path: String,
    codec: DynCodec,
    metadata: MetadataMap,
    deadline: Option<Duration>,
    rx: mpsc::UnboundedReceiver<DynamicMessage>,
    events: mpsc::UnboundedSender<Event>,
    start: Instant,
) -> Event {
    let finished = |code: i32, message: String, metadata: Vec<(String, String)>| Event::Finished {
        code,
        message,
        metadata,
        at: start.elapsed(),
    };
    let mut grpc = tonic::client::Grpc::new(channel);
    if let Err(e) = grpc.ready().await {
        return finished(14, format!("service not ready: {e}"), vec![]);
    }
    let Ok(pq) = http::uri::PathAndQuery::try_from(path.as_str()) else {
        return finished(13, format!("invalid method path {path}"), vec![]);
    };
    let mut req = tonic::Request::new(tokio_stream::wrappers::UnboundedReceiverStream::new(rx));
    *req.metadata_mut() = metadata;
    if let Some(d) = deadline {
        req.set_timeout(d);
    }
    let resp = match grpc.streaming(req, pq, codec).await {
        Ok(r) => r,
        Err(s) => return status_event(&s, start, deadline),
    };
    let (md, mut stream, _) = resp.into_parts();
    let _ = events.send(Event::Headers {
        metadata: metadata_pairs(&md),
        at: start.elapsed(),
    });
    let opts = schema::display_options();
    loop {
        match stream.message().await {
            Ok(Some(m)) => {
                let size = m.encoded_len();
                let _ = events.send(Event::Received {
                    json: schema::to_json(&m, &schema::wire_options()),
                    full: schema::to_json(&m, &opts),
                    compact: schema::to_compact(&m),
                    size,
                    at: start.elapsed(),
                    wall: SystemTime::now(),
                });
            }
            Ok(None) => break,
            Err(s) => return status_event(&s, start, deadline),
        }
    }
    match stream.trailers().await {
        Ok(t) => finished(
            0,
            String::new(),
            t.map(|t| metadata_pairs(&t)).unwrap_or_default(),
        ),
        Err(s) => status_event(&s, start, deadline),
    }
}

fn status_event(s: &Status, start: Instant, deadline: Option<Duration>) -> Event {
    // tonic reports its own expiry of the `grpc-timeout` deadline as CANCELLED.
    if let Some(d) = deadline
        && s.code() == tonic::Code::Cancelled
        && start.elapsed() >= d
    {
        return Event::Finished {
            code: 4,
            message: format!("deadline of {} ms exceeded", d.as_millis()),
            metadata: metadata_pairs(s.metadata()),
            at: start.elapsed(),
        };
    }
    Event::Finished {
        code: s.code() as i32,
        message: s.message().to_string(),
        metadata: metadata_pairs(s.metadata()),
        at: start.elapsed(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata() {
        let m = metadata_map(&[
            ("X-A".into(), "1".into()),
            ("x-a".into(), "2".into()),
            ("trace-bin".into(), "AAEC".into()),
        ])
        .unwrap();
        let pairs = metadata_pairs(&m);
        assert!(pairs.contains(&("x-a".to_string(), "1".to_string())));
        assert!(pairs.contains(&("x-a".to_string(), "2".to_string())));
        assert!(pairs.contains(&("trace-bin".to_string(), "AAEC".to_string())));
        assert!(metadata_map(&[("x-bin".into(), "!!".into())]).is_err());
        assert!(metadata_map(&[("bad key".into(), "v".into())]).is_err());
        assert!(metadata_map(&[("k".into(), "é".into())]).is_err());
        assert_eq!(code_name(4), "DEADLINE_EXCEEDED");
        assert_eq!(code_name(99), "UNKNOWN_CODE");
        for c in 0..17 {
            assert_ne!(code_name(c), "UNKNOWN_CODE");
        }
    }
}
