//! Sends resolved HTTP requests with reqwest + rustls and measures a
//! DNS / connect / TLS / TTFB / download breakdown.
//!
//! Each send uses a fresh client so per-request options (timeout, redirects,
//! TLS verification) apply exactly and every timing phase is observed.

use std::fmt;
use std::future::Future;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::client::{ClientSessionMemoryCache, ClientSessionStore, Resumption};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, NamedGroup, SignatureScheme};
use sendito_core::resolve::{PartData, ResolvedBody, ResolvedHttp};

/// Phase durations. `None` when the phase did not happen (e.g. no DNS for an IP
/// literal, no TLS for `http://`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    pub dns: Option<Duration>,
    pub connect: Option<Duration>,
    pub tls: Option<Duration>,
    /// From connection ready (or request start) to response headers.
    pub ttfb: Duration,
    pub download: Duration,
    pub total: Duration,
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: String,
    pub version: String,
    /// Final URL after redirects.
    pub url: String,
    pub remote_addr: Option<SocketAddr>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// True when the body exceeded the size limit and was cut.
    pub truncated: bool,
    /// Set when the body arrived with a known `Content-Encoding` and was
    /// transparently decompressed. `body` then holds the decoded bytes.
    pub decoded: Option<Decoded>,
    pub timing: Timing,
}

/// A response body that was received compressed and decoded for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The `Content-Encoding` value that was applied (`gzip`, `deflate`, `br`).
    pub encoding: String,
    /// Body size as received on the wire, in bytes.
    pub raw_len: u64,
}

impl HttpResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    /// Approximate size of the header block in bytes.
    pub fn headers_size(&self) -> usize {
        self.headers
            .iter()
            .map(|(n, v)| n.len() + v.len() + 4)
            .sum()
    }

    /// Size of the body as received on the wire: the compressed size when the
    /// body was decompressed, else the body length.
    pub fn wire_body_len(&self) -> u64 {
        match &self.decoded {
            Some(d) => d.raw_len,
            None => self.body.len() as u64,
        }
    }
}

/// Decompresses a response body according to its `Content-Encoding` header.
/// Only whole-body encodings Sendito can decode are touched: `gzip`,
/// `deflate` (zlib, with a raw-deflate fallback) and `br`. Anything else
/// (multiple codings, `zstd`, unknown names) and a stream that does not
/// decompress cleanly is returned untouched, so what is shown is what was
/// received. Sendito never advertises `Accept-Encoding`, so this only decodes
/// what the server chose to send.
fn decode_body(
    headers: &[(String, String)],
    body: Vec<u8>,
    max_body: usize,
) -> (Vec<u8>, Option<Decoded>, bool) {
    let mut encodings = headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("content-encoding"));
    let enc = encodings
        .next()
        .map(|(_, v)| v.trim().to_ascii_lowercase())
        .unwrap_or_default();
    // Multiple header fields form a list of codings; do not partially decode.
    if body.is_empty() || encodings.next().is_some() {
        return (body, None, false);
    }
    let raw_len = body.len() as u64;
    let decoded = match enc.as_str() {
        "gzip" => read_limited(flate2::read::MultiGzDecoder::new(&body[..]), max_body),
        "deflate" => read_limited(flate2::read::ZlibDecoder::new(&body[..]), max_body)
            .or_else(|| read_limited(flate2::read::DeflateDecoder::new(&body[..]), max_body)),
        "br" => read_limited(brotli::Decompressor::new(&body[..], 4096), max_body),
        _ => None,
    };
    match decoded {
        Some((b, truncated)) => (
            b,
            Some(Decoded {
                encoding: enc,
                raw_len,
            }),
            truncated,
        ),
        None => (body, None, false),
    }
}

fn read_limited(r: impl std::io::Read, max_body: usize) -> Option<(Vec<u8>, bool)> {
    use std::io::Read;
    let limit = (max_body as u64).saturating_add(1);
    let mut out = Vec::new();
    r.take(limit).read_to_end(&mut out).ok()?;
    let truncated = out.len() > max_body;
    out.truncate(max_body);
    Some((out, truncated))
}

#[derive(Debug, Clone)]
pub struct SendOptions {
    /// Timeout used when the request sets none.
    pub default_timeout: Duration,
    /// Bodies larger than this are truncated.
    pub max_body: usize,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            max_body: 100 * 1024 * 1024,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("invalid method {0:?}")]
    Method(String),
    #[error("invalid URL {url:?}: {msg}")]
    Url { url: String, msg: String },
    #[error("invalid header {name:?}: {msg}")]
    Header { name: String, msg: String },
    #[error("cannot read {path}: {msg}")]
    File { path: String, msg: String },
    #[error("TLS setup failed: {0}")]
    Tls(String),
    #[error("request timed out after {0} ms")]
    Timeout(u128),
    #[error("{0}")]
    Send(String),
}

#[derive(Debug, Default)]
struct Marks {
    dns_start: Option<Instant>,
    dns_end: Option<Instant>,
    connect_start: Option<Instant>,
    tls_start: Option<Instant>,
    connect_end: Option<Instant>,
}

type Shared = Arc<Mutex<Marks>>;

fn mark(m: &Shared, f: impl FnOnce(&mut Marks) -> &mut Option<Instant>) {
    if let Ok(mut g) = m.lock() {
        let slot = f(&mut g);
        if slot.is_none() {
            *slot = Some(Instant::now());
        }
    }
}

/// DNS resolver that records when resolution starts and ends.
struct TimedResolver(Shared);

impl reqwest::dns::Resolve for TimedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let marks = self.0.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            mark(&marks, |m| &mut m.dns_start);
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(
                        e.kind(),
                        format!("DNS lookup for {host} failed: {e}"),
                    ))
                })?
                .collect();
            mark(&marks, |m| &mut m.dns_end);
            let it: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(it)
        })
    }
}

/// Connector layer recording when connection establishment finishes.
#[derive(Clone)]
struct TimingLayer(Shared);

impl<S> tower::Layer<S> for TimingLayer {
    type Service = TimingService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TimingService {
            inner,
            marks: self.0.clone(),
        }
    }
}

#[derive(Clone)]
struct TimingService<S> {
    inner: S,
    marks: Shared,
}

impl<S, R> tower_service::Service<R> for TimingService<S>
where
    S: tower_service::Service<R>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        mark(&self.marks, |m| &mut m.connect_start);
        let fut = self.inner.call(req);
        let marks = self.marks.clone();
        Box::pin(async move {
            let r = fut.await;
            mark(&marks, |m| &mut m.connect_end);
            r
        })
    }
}

/// Session store wrapper: rustls consults it while building the ClientHello,
/// right after the TCP connection is up, which marks the start of the handshake.
#[derive(Debug)]
struct TimedSessions {
    inner: ClientSessionMemoryCache,
    marks: Shared,
}

impl ClientSessionStore for TimedSessions {
    fn set_kx_hint(&self, server_name: ServerName<'static>, group: NamedGroup) {
        self.inner.set_kx_hint(server_name, group)
    }
    fn kx_hint(&self, server_name: &ServerName<'_>) -> Option<NamedGroup> {
        mark(&self.marks, |m| &mut m.tls_start);
        self.inner.kx_hint(server_name)
    }
    fn set_tls12_session(
        &self,
        server_name: ServerName<'static>,
        value: rustls::client::Tls12ClientSessionValue,
    ) {
        self.inner.set_tls12_session(server_name, value)
    }
    fn tls12_session(
        &self,
        server_name: &ServerName<'_>,
    ) -> Option<rustls::client::Tls12ClientSessionValue> {
        mark(&self.marks, |m| &mut m.tls_start);
        self.inner.tls12_session(server_name)
    }
    fn remove_tls12_session(&self, server_name: &ServerName<'static>) {
        self.inner.remove_tls12_session(server_name)
    }
    fn insert_tls13_ticket(
        &self,
        server_name: ServerName<'static>,
        value: rustls::client::Tls13ClientSessionValue,
    ) {
        self.inner.insert_tls13_ticket(server_name, value)
    }
    fn take_tls13_ticket(
        &self,
        server_name: &ServerName<'static>,
    ) -> Option<rustls::client::Tls13ClientSessionValue> {
        mark(&self.marks, |m| &mut m.tls_start);
        self.inner.take_tls13_ticket(server_name)
    }
}

/// Accepts any server certificate (TLS verification turned off by the user).
#[derive(Debug)]
pub struct NoVerify(pub Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub fn provider() -> Arc<CryptoProvider> {
    static P: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    P.get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// The platform verifier (system trust store), built once.
pub fn platform_verifier() -> Result<Arc<dyn ServerCertVerifier>, String> {
    static V: OnceLock<Result<Arc<rustls_platform_verifier::Verifier>, String>> = OnceLock::new();
    V.get_or_init(|| {
        rustls_platform_verifier::Verifier::new(provider())
            .map(Arc::new)
            .map_err(|e| e.to_string())
    })
    .clone()
    .map(|v| v as Arc<dyn ServerCertVerifier>)
}

/// A rustls client config with the given verification mode and ALPN protocols.
pub fn tls_config(verify: bool, alpn: &[&[u8]]) -> Result<rustls::ClientConfig, String> {
    let builder = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let verifier: Arc<dyn ServerCertVerifier> = if verify {
        platform_verifier()?
    } else {
        Arc::new(NoVerify(provider()))
    };
    let mut cfg = builder
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(cfg)
}

fn err_chain(e: &(dyn std::error::Error + 'static)) -> String {
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

/// Guesses a MIME type from a file name.
pub fn guess_mime(filename: &str) -> String {
    mime_guess::from_path(filename)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string()
}

async fn read_file(path: &std::path::Path) -> Result<Vec<u8>, HttpError> {
    tokio::fs::read(path).await.map_err(|e| HttpError::File {
        path: path.display().to_string(),
        msg: e.to_string(),
    })
}

/// Builds the header map, preserving order and duplicates.
pub fn header_map(headers: &[(String, String)]) -> Result<HeaderMap, HttpError> {
    let mut map = HeaderMap::new();
    for (n, v) in headers {
        let name = HeaderName::from_bytes(n.as_bytes()).map_err(|e| HttpError::Header {
            name: n.clone(),
            msg: e.to_string(),
        })?;
        let value = HeaderValue::from_bytes(v.as_bytes()).map_err(|_| HttpError::Header {
            name: n.clone(),
            msg: "value contains control characters (such as a line break)".into(),
        })?;
        map.append(name, value);
    }
    Ok(map)
}

/// Sends a resolved request and reads the whole response.
pub async fn send(req: &ResolvedHttp, opts: &SendOptions) -> Result<HttpResponse, HttpError> {
    let marks: Shared = Arc::new(Mutex::new(Marks::default()));
    let mut tls = tls_config(req.verify_tls, &[b"h2", b"http/1.1"]).map_err(HttpError::Tls)?;
    tls.resumption = Resumption::store(Arc::new(TimedSessions {
        inner: ClientSessionMemoryCache::new(8),
        marks: marks.clone(),
    }));
    let timeout = req
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(opts.default_timeout);
    let client = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .dns_resolver(Arc::new(TimedResolver(marks.clone())))
        .connector_layer(TimingLayer(marks.clone()))
        .redirect(if req.follow_redirects {
            reqwest::redirect::Policy::limited(10)
        } else {
            reqwest::redirect::Policy::none()
        })
        .timeout(timeout)
        // No implicit Referer on redirect hops.
        .referer(false)
        // HTTP/1 header names are written in Title-Case (`x-api-key` is sent as
        // `X-Api-Key`); names are case-insensitive. HTTP/2 always sends lowercase.
        .http1_title_case_headers()
        .build()
        .map_err(|e| HttpError::Send(err_chain(&e)))?;

    let method = reqwest::Method::from_bytes(req.method.as_bytes())
        .map_err(|_| HttpError::Method(req.method.clone()))?;
    let url = reqwest::Url::parse(&req.url).map_err(|e| HttpError::Url {
        url: req.url.clone(),
        msg: e.to_string(),
    })?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(HttpError::Url {
            url: req.url.clone(),
            msg: "scheme must be http or https".into(),
        });
    }
    let headers = header_map(&req.headers)?;
    // POST, PUT and PATCH define a meaning for content, so even without a body
    // they carry `Content-Length: 0` (RFC 9110, 8.6) unless the request frames
    // itself.
    let wants_length = matches!(method.as_str(), "POST" | "PUT" | "PATCH");
    let framed = req.headers.iter().any(|(n, _)| {
        n.eq_ignore_ascii_case("content-length") || n.eq_ignore_ascii_case("transfer-encoding")
    });
    let mut rb = client.request(method, url).headers(headers);
    // A defined but empty body is framed explicitly with Content-Length: 0.
    let empty_len = |rb: reqwest::RequestBuilder| {
        if framed {
            rb
        } else {
            rb.header(reqwest::header::CONTENT_LENGTH, "0")
        }
    };
    rb = match &req.body {
        ResolvedBody::None if wants_length => empty_len(rb),
        ResolvedBody::None => rb,
        ResolvedBody::Bytes(b) if b.is_empty() => empty_len(rb),
        ResolvedBody::Bytes(b) => rb.body(b.clone()),
        ResolvedBody::File(p) => {
            let data = read_file(p).await?;
            if data.is_empty() {
                empty_len(rb)
            } else {
                rb.body(data)
            }
        }
        ResolvedBody::Multipart(parts) => {
            let mut form = reqwest::multipart::Form::new();
            for p in parts {
                let part = match &p.data {
                    PartData::Text(t) => {
                        let part = reqwest::multipart::Part::text(t.clone());
                        match &p.content_type {
                            Some(ct) => part.mime_str(ct).map_err(|e| HttpError::Header {
                                name: format!("multipart part {} content type", p.name),
                                msg: e.to_string(),
                            })?,
                            None => part,
                        }
                    }
                    PartData::File { path, filename } => {
                        let bytes = read_file(path).await?;
                        let ct = p
                            .content_type
                            .clone()
                            .unwrap_or_else(|| guess_mime(filename));
                        reqwest::multipart::Part::bytes(bytes)
                            .file_name(filename.clone())
                            .mime_str(&ct)
                            .map_err(|e| HttpError::Header {
                                name: format!("multipart part {} content type", p.name),
                                msg: e.to_string(),
                            })?
                    }
                };
                form = form.part(p.name.clone(), part);
            }
            rb.multipart(form)
        }
    };

    let request = rb.build().map_err(|e| HttpError::Send(err_chain(&e)))?;
    let start = Instant::now();
    let mut resp = client.execute(request).await.map_err(|e| {
        if e.is_timeout() {
            HttpError::Timeout(timeout.as_millis())
        } else {
            HttpError::Send(err_chain(&e))
        }
    })?;
    let headers_at = Instant::now();
    let status = resp.status();
    let version = format!("{:?}", resp.version());
    let final_url = resp.url().to_string();
    let remote_addr = resp.remote_addr();
    let headers: Vec<(String, String)> = resp
        .headers()
        .iter()
        .map(|(n, v)| {
            (
                n.as_str().to_string(),
                String::from_utf8_lossy(v.as_bytes()).into_owned(),
            )
        })
        .collect();
    let mut body = Vec::new();
    let mut truncated = false;
    loop {
        match resp.chunk().await {
            Ok(Some(c)) => {
                if body.len() + c.len() > opts.max_body {
                    let room = opts.max_body.saturating_sub(body.len());
                    body.extend_from_slice(&c[..room]);
                    truncated = true;
                    break;
                }
                body.extend_from_slice(&c);
            }
            Ok(None) => break,
            Err(e) => {
                return Err(if e.is_timeout() {
                    HttpError::Timeout(timeout.as_millis())
                } else {
                    HttpError::Send(format!("reading body: {}", err_chain(&e)))
                });
            }
        }
    }
    let end = Instant::now();
    // A body cut at the size limit is kept as received: decompressing half a
    // stream would fail or lie.
    let (body, decoded, decoded_truncated) = if truncated {
        (body, None, false)
    } else {
        decode_body(&headers, body, opts.max_body)
    };
    truncated |= decoded_truncated;

    let m = marks.lock().map(|m| Marks { ..*m }).unwrap_or_default();
    let dns = match (m.dns_start, m.dns_end) {
        (Some(a), Some(b)) => Some(b.saturating_duration_since(a)),
        _ => None,
    };
    let tcp_from = m.dns_end.or(m.connect_start);
    let tcp_to = m.tls_start.or(m.connect_end);
    let connect = match (tcp_from, tcp_to) {
        (Some(a), Some(b)) => Some(b.saturating_duration_since(a)),
        _ => None,
    };
    let tls_d = match (m.tls_start, m.connect_end) {
        (Some(a), Some(b)) => Some(b.saturating_duration_since(a)),
        _ => None,
    };
    let ready = m.connect_end.unwrap_or(start).max(start);
    let timing = Timing {
        dns,
        connect,
        tls: tls_d,
        ttfb: headers_at.saturating_duration_since(ready),
        download: end.saturating_duration_since(headers_at),
        total: end.saturating_duration_since(start),
    };
    Ok(HttpResponse {
        status: status.as_u16(),
        reason: status.canonical_reason().unwrap_or("").to_string(),
        version,
        url: final_url,
        remote_addr,
        headers,
        body,
        truncated,
        decoded,
        timing,
    })
}

impl fmt::Display for Timing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |d: Option<Duration>| {
            d.map(|d| format!("{:.1} ms", d.as_secs_f64() * 1000.0))
                .unwrap_or_else(|| "-".into())
        };
        write!(
            f,
            "dns {} · connect {} · tls {} · ttfb {} · download {} · total {}",
            ms(self.dns),
            ms(self.connect),
            ms(self.tls),
            ms(Some(self.ttfb)),
            ms(Some(self.download)),
            ms(Some(self.total))
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn one_shot_server(
        response: &'static [u8],
    ) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let h = tokio::spawn(async move {
            let (mut s, _) = l.accept().await.unwrap();
            let mut buf = vec![0u8; 65536];
            let mut got = Vec::new();
            loop {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                if n == 0 || got.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            s.write_all(response).await.unwrap();
            s.shutdown().await.ok();
            got
        });
        (format!("http://localhost:{}", addr.port()), h)
    }

    fn req(url: String) -> ResolvedHttp {
        ResolvedHttp {
            method: "GET".into(),
            url,
            headers: vec![("X-A".into(), "1".into()), ("X-A".into(), "2".into())],
            header_origins: vec![],
            body: ResolvedBody::None,
            timeout_ms: Some(5000),
            follow_redirects: true,
            verify_tls: true,
            unresolved: vec![],
            notes: vec![],
        }
    }

    #[tokio::test]
    async fn sends_exact_headers() {
        let (url, h) =
            one_shot_server(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nX-R: a\r\n\r\nok").await;
        let mut rq = req(format!("{url}/p?q=1"));
        rq.headers.push(("x-lower".into(), "v".into()));
        rq.headers.push(("X-API-Key".into(), "k".into()));
        let r = send(&rq, &SendOptions::default()).await.unwrap();
        assert_eq!(r.status, 200);
        assert_eq!(r.body, b"ok");
        assert_eq!(r.header("x-r"), Some("a"));
        assert!(r.timing.dns.is_some());
        assert!(r.timing.connect.is_some());
        assert!(r.timing.tls.is_none());
        assert!(r.headers_size() > 0);
        assert!(r.timing.to_string().contains("total"));
        let raw = String::from_utf8(h.await.unwrap()).unwrap();
        let lines: Vec<&str> = raw.split("\r\n").collect();
        assert_eq!(lines[0], "GET /p?q=1 HTTP/1.1");
        assert!(lines.contains(&"X-A: 1"));
        // HTTP/1 names are Title-Case on the wire (docs/DEBT.md).
        assert!(lines.contains(&"X-Lower: v"), "{raw}");
        assert!(lines.contains(&"X-Api-Key: k"), "{raw}");
        assert!(
            lines.contains(&"Host: localhost:".to_string().as_str())
                || raw.contains("Host: localhost:")
        );
        assert!(lines.contains(&"X-A: 2"));
        // reqwest adds Accept: */* when the request defines none (docs/DEBT.md).
        assert!(lines.contains(&"Accept: */*"), "{raw}");
        assert!(!raw.to_ascii_lowercase().contains("user-agent"), "{raw}");
    }

    #[tokio::test]
    async fn truncates_and_errors() {
        let (url, _h) =
            one_shot_server(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n0123456789").await;
        let r = send(
            &req(url),
            &SendOptions {
                max_body: 4,
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(r.truncated);
        assert_eq!(r.body, b"0123");

        let mut bad = req("http://127.0.0.1:1/".into());
        assert!(matches!(
            send(&bad, &SendOptions::default()).await,
            Err(HttpError::Send(_))
        ));
        bad.url = "ftp://x".into();
        assert!(matches!(
            send(&bad, &SendOptions::default()).await,
            Err(HttpError::Url { .. })
        ));
        bad.url = "::".into();
        assert!(matches!(
            send(&bad, &SendOptions::default()).await,
            Err(HttpError::Url { .. })
        ));
        bad.url = "http://x".into();
        bad.method = "G ET".into();
        assert!(matches!(
            send(&bad, &SendOptions::default()).await,
            Err(HttpError::Method(_))
        ));
        bad.method = "GET".into();
        bad.headers = vec![("Bad Name".into(), "x".into())];
        assert!(matches!(
            send(&bad, &SendOptions::default()).await,
            Err(HttpError::Header { .. })
        ));
        bad.headers = vec![("X".into(), "a\r\nb".into())];
        assert!(matches!(
            send(&bad, &SendOptions::default()).await,
            Err(HttpError::Header { .. })
        ));
        bad.headers = vec![];
        bad.body = ResolvedBody::File("/definitely/missing".into());
        assert!(matches!(
            send(&bad, &SendOptions::default()).await,
            Err(HttpError::File { .. })
        ));
    }

    #[tokio::test]
    async fn bodies_multipart_and_file() {
        let dir = std::env::temp_dir().join(format!("sendito-http-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), b"{}").unwrap();
        std::fs::write(dir.join("empty.bin"), b"").unwrap();
        let (url, h) = one_shot_server(b"HTTP/1.1 204 No Content\r\n\r\n").await;
        let mut r = req(url);
        r.method = "POST".into();
        r.headers.clear();
        r.body = ResolvedBody::Multipart(vec![
            sendito_core::resolve::ResolvedPart {
                name: "t".into(),
                data: PartData::Text("v".into()),
                content_type: Some("text/plain".into()),
            },
            sendito_core::resolve::ResolvedPart {
                name: "f".into(),
                data: PartData::File {
                    path: dir.join("a.json"),
                    filename: "a.json".into(),
                },
                content_type: None,
            },
        ]);
        let resp = send(&r, &SendOptions::default()).await.unwrap();
        assert_eq!(resp.status, 204);
        let raw = String::from_utf8_lossy(&h.await.unwrap()).into_owned();
        assert!(raw.contains("multipart/form-data; boundary="), "{raw}");
        let (url, h) = one_shot_server(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
        let mut r = req(url);
        r.method = "PUT".into();
        r.body = ResolvedBody::File(dir.join("empty.bin"));
        send(&r, &SendOptions::default()).await.unwrap();
        let raw = String::from_utf8_lossy(&h.await.unwrap()).into_owned();
        assert!(raw.contains("Content-Length: 0"), "{raw}");
        let mut bad = req("http://127.0.0.1:1/".into());
        bad.body = ResolvedBody::Multipart(vec![sendito_core::resolve::ResolvedPart {
            name: "t".into(),
            data: PartData::Text("v".into()),
            content_type: Some("not a mime".into()),
        }]);
        assert!(matches!(
            send(&bad, &SendOptions::default()).await,
            Err(HttpError::Header { .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn timeout() {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = l.local_addr().unwrap().port();
        let _h = tokio::spawn(async move {
            let (_s, _) = l.accept().await.unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let mut r = req(format!("http://127.0.0.1:{port}/"));
        r.timeout_ms = Some(100);
        assert!(matches!(
            send(&r, &SendOptions::default()).await,
            Err(HttpError::Timeout(100))
        ));
    }

    #[test]
    fn helpers() {
        assert_eq!(guess_mime("a.png"), "image/png");
        assert_eq!(guess_mime("a.unknownext"), "application/octet-stream");
        assert!(tls_config(false, &[b"h2"]).is_ok());
        let m = header_map(&[("A".into(), "é".into())]).unwrap();
        assert_eq!(m.get("a").map(|v| v.as_bytes()), Some("é".as_bytes()));
    }

    fn gzipped(data: &[u8]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::read::GzEncoder;
        use std::io::Read as _;
        let mut e = GzEncoder::new(data, Compression::default());
        let mut out = Vec::new();
        e.read_to_end(&mut out).unwrap();
        out
    }

    fn zipped(data: &[u8]) -> Vec<u8> {
        use flate2::Compression;
        use flate2::read::ZlibEncoder;
        use std::io::Read as _;
        let mut e = ZlibEncoder::new(data, Compression::default());
        let mut out = Vec::new();
        e.read_to_end(&mut out).unwrap();
        out
    }

    fn brotlied(data: &[u8]) -> Vec<u8> {
        use brotli::enc::BrotliEncoderParams;
        use std::io::Read as _;
        let mut out = Vec::new();
        let params = BrotliEncoderParams::default();
        let mut r = brotli::CompressorReader::new(
            data,
            4096,
            u32::try_from(params.quality).unwrap_or(11),
            u32::try_from(params.lgwin).unwrap_or(22),
        );
        r.read_to_end(&mut out).unwrap();
        out
    }

    fn hdr(enc: &str) -> Vec<(String, String)> {
        vec![("Content-Encoding".into(), enc.into())]
    }

    #[test]
    fn decodes_known_encodings() {
        let text = b"{\"hello\": \"compressed world\"}";
        for (enc, bytes) in [
            ("gzip", gzipped(text)),
            ("deflate", zipped(text)),
            ("br", brotlied(text)),
        ] {
            let (body, d, truncated) = decode_body(&hdr(enc), bytes, 1024);
            assert!(!truncated);
            assert_eq!(body, text, "{enc}");
            let d = d.unwrap_or_else(|| panic!("{enc} not decoded"));
            assert_eq!(d.encoding, enc);
            assert!(d.raw_len > 0);
        }
        // Raw deflate (no zlib wrapper) is served by some servers.
        let raw = {
            use flate2::Compression;
            use flate2::read::DeflateEncoder;
            use std::io::Read as _;
            let mut e = DeflateEncoder::new(&text[..], Compression::default());
            let mut out = Vec::new();
            e.read_to_end(&mut out).unwrap();
            out
        };
        let (body, d, truncated) = decode_body(&hdr("deflate"), raw, 1024);
        assert!(!truncated);
        assert_eq!(body, text);
        assert_eq!(d.map(|d| d.encoding).as_deref(), Some("deflate"));
    }

    #[test]
    fn decode_leaves_unhandled_cases_alone() {
        let text = b"plain";
        // No Content-Encoding, identity, unknown coding, stacked codings.
        for hdrs in [Vec::new(), hdr("identity"), hdr("zstd"), hdr("gzip, br")] {
            let (body, d, truncated) = decode_body(&hdrs, text.to_vec(), 1024);
            assert!(!truncated);
            assert_eq!(body, text);
            assert!(d.is_none());
        }
        // A corrupt stream and an empty body stay as received.
        let (body, d, truncated) = decode_body(&hdr("gzip"), vec![1, 2, 3, 4, 5], 1024);
        assert!(!truncated);
        assert_eq!(body, vec![1, 2, 3, 4, 5]);
        assert!(d.is_none());
        let (body, d, truncated) = decode_body(&hdr("gzip"), Vec::new(), 1024);
        assert!(!truncated);
        assert!(body.is_empty());
        assert!(d.is_none());
    }

    #[test]
    fn decoded_body_is_bounded_and_stacked_headers_are_untouched() {
        let original = vec![b'x'; 100_000];
        let compressed = gzipped(&original);
        let (body, decoded, truncated) = decode_body(&hdr("gzip"), compressed.clone(), 32);
        assert_eq!(body, vec![b'x'; 32]);
        assert!(decoded.is_some());
        assert!(truncated);
        let headers = vec![
            ("content-encoding".into(), "gzip".into()),
            ("content-encoding".into(), "br".into()),
        ];
        let (body, decoded, truncated) = decode_body(&headers, compressed.clone(), 32);
        assert_eq!(body, compressed);
        assert!(decoded.is_none());
        assert!(!truncated);
    }

    #[tokio::test]
    async fn decodes_and_reports_in_send() {
        let text = b"compressed body";
        let gz = gzipped(text);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Encoding: gzip\r\nContent-Length: {}\r\n\r\n",
            gz.len()
        );
        let mut raw = head.into_bytes();
        raw.extend_from_slice(&gz);
        // Leak: the one-shot server takes a `&'static [u8]`.
        let raw: &'static [u8] = Box::leak(raw.into_boxed_slice());
        let (url, h) = one_shot_server(raw).await;
        let r = send(&req(url), &SendOptions::default()).await.unwrap();
        assert_eq!(r.body, text);
        let d = r.decoded.as_ref().unwrap();
        assert_eq!(d.encoding, "gzip");
        assert_eq!(d.raw_len, gz.len() as u64);
        assert_eq!(r.wire_body_len(), gz.len() as u64);
        // The request itself never advertises Accept-Encoding.
        let sent = String::from_utf8(h.await.unwrap()).unwrap();
        assert!(
            !sent.to_ascii_lowercase().contains("accept-encoding"),
            "{sent}"
        );
    }
}
