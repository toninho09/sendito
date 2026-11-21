//! gRPC echo server with reflection (v1 and v1alpha). It records the metadata
//! and every request message it receives, decoded with the schema and as raw
//! protobuf bytes.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, SerializeOptions};
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};

pub mod pb {
    #![allow(clippy::all, clippy::unwrap_used, clippy::expect_used)]
    tonic::include_proto!("sendito.test.v1");
}

use pb::echo_server::{Echo, EchoServer};
use pb::{EchoRequest, EchoResponse};

/// Exact bytes of each request message, in arrival order (filled by [`RawCodec`]).
static RAW: Mutex<std::collections::VecDeque<Vec<u8>>> =
    Mutex::new(std::collections::VecDeque::new());

fn take_raw() -> Vec<u8> {
    RAW.lock()
        .map(|mut q| q.pop_front().unwrap_or_default())
        .unwrap_or_default()
}

/// Server codec that records the received protobuf bytes before decoding.
pub struct RawCodec<T, U>(std::marker::PhantomData<(T, U)>);

impl<T, U> Default for RawCodec<T, U> {
    fn default() -> Self {
        Self(std::marker::PhantomData)
    }
}

pub struct RawEncoder<T>(std::marker::PhantomData<T>);
pub struct RawDecoder<U>(std::marker::PhantomData<U>);

impl<T: Message + Send + 'static, U: Message + Default + Send + 'static> tonic::codec::Codec
    for RawCodec<T, U>
{
    type Encode = T;
    type Decode = U;
    type Encoder = RawEncoder<T>;
    type Decoder = RawDecoder<U>;

    fn encoder(&mut self) -> Self::Encoder {
        RawEncoder(std::marker::PhantomData)
    }

    fn decoder(&mut self) -> Self::Decoder {
        RawDecoder(std::marker::PhantomData)
    }
}

impl<T: Message> tonic::codec::Encoder for RawEncoder<T> {
    type Item = T;
    type Error = Status;

    fn encode(&mut self, item: T, buf: &mut tonic::codec::EncodeBuf<'_>) -> Result<(), Status> {
        item.encode(buf)
            .map_err(|e| Status::internal(e.to_string()))
    }
}

impl<U: Message + Default> tonic::codec::Decoder for RawDecoder<U> {
    type Item = U;
    type Error = Status;

    fn decode(&mut self, buf: &mut tonic::codec::DecodeBuf<'_>) -> Result<Option<U>, Status> {
        use bytes::Buf;
        let bytes = buf.copy_to_bytes(buf.remaining());
        if let Ok(mut q) = RAW.lock() {
            q.push_back(bytes.to_vec());
        }
        U::decode(bytes)
            .map(Some)
            .map_err(|e| Status::internal(e.to_string()))
    }
}

pub const DESCRIPTOR: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/echo_descriptor.bin"));

/// Directory holding the test `.proto` files (import root).
pub fn proto_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("proto")
}

/// What the server received for one call.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GrpcCapture {
    pub method: String,
    /// Request metadata in received order (HTTP/2 pseudo and transport headers included).
    pub metadata: Vec<(String, String)>,
    /// Each request message as JSON (proto field names, defaults omitted).
    pub messages: Vec<String>,
    /// Each request message as base64 protobuf bytes.
    pub raw: Vec<String>,
}

fn pool() -> DescriptorPool {
    DescriptorPool::decode(DESCRIPTOR).unwrap_or_default()
}

/// Decoded JSON and the exact received bytes (base64) of one request message.
fn decode(_req: &EchoRequest) -> (String, String) {
    let bytes = take_raw();
    let pool = pool();
    let json = pool
        .get_message_by_name("sendito.test.v1.EchoRequest")
        .and_then(|d| DynamicMessage::decode(d, bytes.as_slice()).ok())
        .map(|m| {
            let mut buf = Vec::new();
            let mut ser = serde_json::Serializer::new(&mut buf);
            let opts = SerializeOptions::new().use_proto_field_name(true);
            let _ = m.serialize_with_options(&mut ser, &opts);
            String::from_utf8(buf).unwrap_or_default()
        })
        .unwrap_or_default();
    (
        json,
        base64::engine::general_purpose::STANDARD.encode(bytes),
    )
}

#[derive(Clone, Default)]
pub struct EchoService {
    pub captures: Arc<Mutex<Vec<GrpcCapture>>>,
}

fn md_pairs(md: &tonic::metadata::MetadataMap) -> Vec<(String, String)> {
    md.iter()
        .map(|kv| match kv {
            tonic::metadata::KeyAndValueRef::Ascii(k, v) => (
                k.as_str().to_string(),
                v.to_str().unwrap_or("<non-ascii>").to_string(),
            ),
            tonic::metadata::KeyAndValueRef::Binary(k, v) => (
                k.as_str().to_string(),
                base64::engine::general_purpose::STANDARD.encode(v.to_bytes().unwrap_or_default()),
            ),
        })
        .collect()
}

impl EchoService {
    fn begin(&self, method: &str, md: &tonic::metadata::MetadataMap) -> usize {
        let mut c = self.captures.lock().unwrap_or_else(|e| e.into_inner());
        c.push(GrpcCapture {
            method: method.into(),
            metadata: md_pairs(md),
            ..Default::default()
        });
        c.len() - 1
    }

    fn record(&self, idx: usize, req: &EchoRequest) {
        let (json, raw) = decode(req);
        let mut c = self.captures.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(cap) = c.get_mut(idx) {
            cap.messages.push(json);
            cap.raw.push(raw);
        }
    }
}

fn custom_md(md: &tonic::metadata::MetadataMap) -> std::collections::HashMap<String, String> {
    md_pairs(md)
        .into_iter()
        .filter(|(k, _)| {
            !matches!(
                k.as_str(),
                "te" | "content-type" | "user-agent" | "grpc-timeout" | "grpc-accept-encoding"
            )
        })
        .collect()
}

async fn control(req: &EchoRequest) -> Result<(), Status> {
    if req.delay_ms > 0 {
        tokio::time::sleep(Duration::from_millis(req.delay_ms as u64)).await;
    }
    if req.fail_code != 0 {
        return Err(Status::new(
            tonic::Code::from_i32(req.fail_code),
            req.fail_message.clone(),
        ));
    }
    Ok(())
}

type RespStream = Pin<Box<dyn Stream<Item = Result<EchoResponse, Status>> + Send>>;

#[tonic::async_trait]
impl Echo for EchoService {
    async fn unary(&self, request: Request<EchoRequest>) -> Result<Response<EchoResponse>, Status> {
        let idx = self.begin("Unary", request.metadata());
        let md = custom_md(request.metadata());
        let req = request.into_inner();
        self.record(idx, &req);
        control(&req).await?;
        let mut resp = Response::new(EchoResponse {
            request: Some(req),
            metadata: md,
            index: 0,
            count: 1,
        });
        resp.metadata_mut().insert(
            "x-served-by",
            tonic::metadata::MetadataValue::from_static("echo"),
        );
        Ok(resp)
    }

    type ServerStreamStream = RespStream;

    async fn server_stream(
        &self,
        request: Request<EchoRequest>,
    ) -> Result<Response<RespStream>, Status> {
        let idx = self.begin("ServerStream", request.metadata());
        let md = custom_md(request.metadata());
        let req = request.into_inner();
        self.record(idx, &req);
        let n = if req.repeat > 0 { req.repeat } else { 3 };
        let s = async_stream(req, md, n);
        Ok(Response::new(Box::pin(s)))
    }

    async fn client_stream(
        &self,
        request: Request<Streaming<EchoRequest>>,
    ) -> Result<Response<EchoResponse>, Status> {
        let idx = self.begin("ClientStream", request.metadata());
        let md = custom_md(request.metadata());
        let mut s = request.into_inner();
        let mut last = None;
        let mut count = 0;
        while let Some(m) = s.next().await {
            let m = m?;
            self.record(idx, &m);
            control(&m).await?;
            count += 1;
            last = Some(m);
        }
        Ok(Response::new(EchoResponse {
            request: last,
            metadata: md,
            index: count - 1,
            count,
        }))
    }

    type BidiStream = RespStream;

    async fn bidi(
        &self,
        request: Request<Streaming<EchoRequest>>,
    ) -> Result<Response<RespStream>, Status> {
        let idx = self.begin("Bidi", request.metadata());
        let md = custom_md(request.metadata());
        let mut s = request.into_inner();
        let me = self.clone();
        let out = async_stream_bidi(move |tx| async move {
            let mut i = 0;
            while let Some(m) = s.next().await {
                let m = match m {
                    Ok(m) => m,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        return;
                    }
                };
                me.record(idx, &m);
                if let Err(e) = control(&m).await {
                    let _ = tx.send(Err(e)).await;
                    return;
                }
                let r = EchoResponse {
                    request: Some(m),
                    metadata: md.clone(),
                    index: i,
                    count: i + 1,
                };
                i += 1;
                if tx.send(Ok(r)).await.is_err() {
                    return;
                }
            }
        });
        Ok(Response::new(out))
    }

    async fn ping(&self, request: Request<()>) -> Result<Response<()>, Status> {
        let idx = self.begin("Ping", request.metadata());
        // google.protobuf.Empty: no fields, zero bytes on the wire.
        let mut c = self.captures.lock().unwrap_or_else(|e| e.into_inner());
        let raw = take_raw();
        if let Some(cap) = c.get_mut(idx) {
            cap.messages.push("{}".into());
            cap.raw
                .push(base64::engine::general_purpose::STANDARD.encode(raw));
        }
        Ok(Response::new(()))
    }
}

fn async_stream(
    req: EchoRequest,
    md: std::collections::HashMap<String, String>,
    n: i32,
) -> RespStream {
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(async move {
        if let Err(e) = control(&EchoRequest {
            delay_ms: 0,
            ..req.clone()
        })
        .await
        {
            let _ = tx.send(Err(e)).await;
            return;
        }
        for i in 0..n {
            if req.delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(req.delay_ms as u64)).await;
            }
            let r = EchoResponse {
                request: Some(req.clone()),
                metadata: md.clone(),
                index: i,
                count: n,
            };
            if tx.send(Ok(r)).await.is_err() {
                return;
            }
        }
    });
    Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
}

fn async_stream_bidi<F, Fut>(f: F) -> RespStream
where
    F: FnOnce(tokio::sync::mpsc::Sender<Result<EchoResponse, Status>>) -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let (tx, rx) = tokio::sync::mpsc::channel(16);
    tokio::spawn(f(tx));
    Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx))
}

pub struct GrpcEcho {
    pub addr: SocketAddr,
    pub captures: Arc<Mutex<Vec<GrpcCapture>>>,
    task: tokio::task::JoinHandle<()>,
}

impl GrpcEcho {
    pub fn target(&self) -> String {
        format!("127.0.0.1:{}", self.addr.port())
    }

    pub fn take(&self) -> Vec<GrpcCapture> {
        self.captures
            .lock()
            .map(|mut c| std::mem::take(&mut *c))
            .unwrap_or_default()
    }
}

impl Drop for GrpcEcho {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts the server with reflection on a random local port.
pub async fn start() -> Result<GrpcEcho, Box<dyn std::error::Error + Send + Sync>> {
    start_on("127.0.0.1:0", true).await
}

pub async fn start_on(
    addr: &str,
    reflection: bool,
) -> Result<GrpcEcho, Box<dyn std::error::Error + Send + Sync>> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let svc = EchoService::default();
    let captures = svc.captures.clone();
    let mut builder = tonic::transport::Server::builder();
    let mut router = builder.add_service(EchoServer::new(svc));
    if reflection {
        let v1 = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(DESCRIPTOR)
            .build_v1()?;
        let v1a = tonic_reflection::server::Builder::configure()
            .register_encoded_file_descriptor_set(DESCRIPTOR)
            .build_v1alpha()?;
        router = router.add_service(v1).add_service(v1a);
    }
    let incoming = tokio_stream::wrappers::TcpListenerStream::new(listener);
    let task = tokio::spawn(async move {
        let _ = router.serve_with_incoming(incoming).await;
    });
    Ok(GrpcEcho {
        addr,
        captures,
        task,
    })
}
