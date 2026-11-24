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
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, NamedGroup, SignatureScheme};
use sendito_core::resolve::{HeaderOrigin, PartData, ResolvedBody, ResolvedHttp};

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
    /// Outbound proxy from `[http] proxy` in the config. `None` keeps the
    /// client's default proxy discovery: the proxy environment variables,
    /// including `NO_PROXY` (`reqwest` is built without `system-proxy`, so the
    /// OS settings are not read). `env` reads the variables explicitly. Applies
    /// to HTTP requests and MCP over Streamable HTTP, not to gRPC (FORMAT.md).
    pub proxy: Option<ProxySpec>,
    /// PEM material from `[http] tls` in the config (custom CA, client
    /// certificate); empty uses the platform roots and no client auth.
    pub tls: TlsMaterial,
}

impl Default for SendOptions {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            max_body: 100 * 1024 * 1024,
            proxy: None,
            tls: TlsMaterial::default(),
        }
    }
}

/// Outbound proxy from `[http] proxy` in the config.
#[derive(Clone, PartialEq, Eq)]
pub enum ProxySpec {
    /// `proxy = "env"`: `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and
    /// `NO_PROXY` (lowercase too), with curl's precedence — the scheme
    /// variable beats `ALL_PROXY`, and `NO_PROXY` exempts hosts.
    Env,
    /// A proxy URL such as `"http://host:port"` (also `https://`,
    /// `socks4://`, `socks4a://`, `socks5://`, `socks5h://`); credentials may
    /// be part of the URL.
    Url(String),
}

/// Credentials in a proxy URL never reach logs: `Debug` shows them redacted.
impl std::fmt::Debug for ProxySpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxySpec::Env => f.write_str("Env"),
            ProxySpec::Url(u) => f.debug_tuple("Url").field(&redact_url(u)).finish(),
        }
    }
}

impl ProxySpec {
    /// Parses the `[http] proxy` value.
    pub fn parse(value: &str) -> Result<Self, String> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("env") {
            return Ok(ProxySpec::Env);
        }
        const SCHEMES: [&str; 6] = [
            "http://",
            "https://",
            "socks4://",
            "socks4a://",
            "socks5://",
            "socks5h://",
        ];
        if SCHEMES.iter().any(|s| v.starts_with(s)) {
            return Ok(ProxySpec::Url(v.to_string()));
        }
        Err(format!(
            "invalid [http] proxy {:?}: use \"env\" or a URL such as \"http://host:port\" or \"socks5://host:port\"",
            redact_url(v)
        ))
    }

    /// Adds the proxy to a client builder. Reads the environment only for
    /// [`ProxySpec::Env`], at send time.
    pub fn apply(&self, mut b: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder, String> {
        match self {
            ProxySpec::Url(u) => {
                let p = reqwest::Proxy::all(u.clone())
                    .map_err(|e| proxy_error("[http] proxy", u, e))?;
                Ok(b.proxy(p))
            }
            ProxySpec::Env => {
                let read = |k: &str| std::env::var(k).ok();
                let (https, http) = env_proxy_urls(&read);
                if let Some(u) = https {
                    let p = reqwest::Proxy::https(u.clone())
                        .map_err(|e| proxy_error("HTTPS_PROXY", &u, e))?
                        .no_proxy(reqwest::NoProxy::from_env());
                    b = b.proxy(p);
                }
                if let Some(u) = http {
                    let p = reqwest::Proxy::http(u.clone())
                        .map_err(|e| proxy_error("HTTP_PROXY", &u, e))?
                        .no_proxy(reqwest::NoProxy::from_env());
                    b = b.proxy(p);
                }
                Ok(b)
            }
        }
    }
}

/// Curl precedence: `HTTPS_PROXY` and `HTTP_PROXY` beat `ALL_PROXY`; the
/// result is the proxy for `https` and `http` requests respectively.
fn env_proxy_urls(get: &impl Fn(&str) -> Option<String>) -> (Option<String>, Option<String>) {
    let all = env_proxy("ALL_PROXY", get);
    let https = env_proxy("HTTPS_PROXY", get).or_else(|| all.clone());
    let http = env_proxy("HTTP_PROXY", get).or(all);
    (https, http)
}

/// One proxy variable, uppercase before lowercase, empty values skipped.
fn env_proxy(name: &str, get: &impl Fn(&str) -> Option<String>) -> Option<String> {
    for key in [name, &name.to_ascii_lowercase()] {
        if let Some(v) = get(key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Builds a proxy error message with the credentials in `raw` redacted. The
/// underlying error may repeat the URL, so the same redaction is applied to
/// its text.
fn proxy_error(what: &str, raw: &str, err: impl std::fmt::Display) -> String {
    let hidden = redact_url(raw);
    let msg = err.to_string();
    let msg = if raw == hidden {
        msg
    } else {
        msg.replace(raw, &hidden)
    };
    format!("invalid {what} {hidden:?}: {msg}")
}

/// A proxy URL without its userinfo (`scheme://user:pass@host` becomes
/// `scheme://host`), so error messages never print credentials.
fn redact_url(raw: &str) -> String {
    let Some((scheme, rest)) = raw.split_once("://") else {
        return raw.to_string();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let Some((_, host)) = rest[..end].rsplit_once('@') else {
        return raw.to_string();
    };
    format!("{scheme}://{host}{}", &rest[end..])
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

/// PEM material from `[http.tls]`: a certificate authority that replaces the
/// platform roots and a client certificate for mutual TLS. Paths are
/// resolved by the caller (relative to the collection root, `~/` expanded)
/// and read when the connection is set up, so a missing file fails the send
/// with its name in the message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsMaterial {
    /// PEM file with CA certificates; the file may hold several.
    pub ca: Option<std::path::PathBuf>,
    /// PEM file with the client certificate chain.
    pub client_cert: Option<std::path::PathBuf>,
    /// PEM file with the client private key.
    pub client_key: Option<std::path::PathBuf>,
}

impl TlsMaterial {
    pub fn is_empty(&self) -> bool {
        self.ca.is_none() && self.client_cert.is_none() && self.client_key.is_none()
    }

    /// `Some(message)` when `client_cert` and `client_key` are not both set
    /// or both unset (they are a pair).
    pub fn pair_error(&self) -> Option<String> {
        match (&self.client_cert, &self.client_key) {
            (None, None) | (Some(_), Some(_)) => None,
            (Some(c), None) => Some(format!(
                "[http.tls] client_cert {} is set without client_key",
                c.display()
            )),
            (None, Some(k)) => Some(format!(
                "[http.tls] client_key {} is set without client_cert",
                k.display()
            )),
        }
    }
}

/// A rustls client config with the given verification mode, PEM material and
/// ALPN protocols. `verify: false` disables server verification, so `ca` is
/// unused there; a client certificate still authenticates the client.
pub fn tls_config(
    verify: bool,
    alpn: &[&[u8]],
    material: &TlsMaterial,
) -> Result<rustls::ClientConfig, String> {
    let builder = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let verifier: Arc<dyn ServerCertVerifier> = if verify {
        match &material.ca {
            None => platform_verifier()?,
            Some(ca) => root_verifier(ca)?,
        }
    } else {
        Arc::new(NoVerify(provider()))
    };
    let builder = builder
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    let mut cfg = match (&material.client_cert, &material.client_key) {
        (Some(cert), Some(key)) => {
            let chain = read_cert_chain(cert)?;
            let key = read_key(key)?;
            builder
                .with_client_auth_cert(chain, key)
                .map_err(|e| e.to_string())?
        }
        _ => builder.with_no_client_auth(),
    };
    cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(cfg)
}

/// A verifier that trusts only the certificates in `ca` (they replace the
/// platform roots). This is the `[http.tls] ca` behaviour.
fn root_verifier(ca: &std::path::Path) -> Result<Arc<dyn ServerCertVerifier>, String> {
    let pem = read_pem(ca)?;
    let mut roots = rustls::RootCertStore::empty();
    let mut n = 0;
    for cert in CertificateDer::pem_slice_iter(&pem) {
        let cert = cert.map_err(|e| format!("{}: {e}", ca.display()))?;
        roots
            .add(cert)
            .map_err(|e| format!("{}: certificate cannot be a trust root: {e}", ca.display()))?;
        n += 1;
    }
    if n == 0 {
        return Err(format!("{} holds no certificates", ca.display()));
    }
    let verifier =
        rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider())
            .build()
            .map_err(|e| e.to_string())?;
    Ok(verifier)
}

fn read_cert_chain(path: &std::path::Path) -> Result<Vec<CertificateDer<'static>>, String> {
    let pem = read_pem(path)?;
    let chain: Vec<_> = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if chain.is_empty() {
        return Err(format!("{} holds no certificates", path.display()));
    }
    Ok(chain)
}

/// A PEM file's bytes. Each send builds its TLS configuration, so the bytes
/// are kept and read again only when the file's size or time changes.
fn read_pem(path: &std::path::Path) -> Result<Arc<[u8]>, String> {
    type Key = (std::path::PathBuf, Option<std::time::SystemTime>, u64);
    type Cache = std::collections::HashMap<std::path::PathBuf, (Key, Arc<[u8]>)>;
    static PEMS: OnceLock<Mutex<Cache>> = OnceLock::new();
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let meta = std::fs::metadata(path).map_err(err)?;
    let key: Key = (path.to_path_buf(), meta.modified().ok(), meta.len());
    let pems = PEMS.get_or_init(Default::default);
    if let Ok(map) = pems.lock()
        && let Some((k, bytes)) = map.get(path)
        && *k == key
    {
        return Ok(bytes.clone());
    }
    let bytes: Arc<[u8]> = std::fs::read(path).map_err(err)?.into();
    if let Ok(mut map) = pems.lock() {
        map.insert(path.to_path_buf(), (key, bytes.clone()));
    }
    Ok(bytes)
}

fn read_key(path: &std::path::Path) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    let pem = read_pem(path)?;
    rustls::pki_types::PrivateKeyDer::from_pem_slice(&pem)
        .map_err(|e| format!("{}: {e}", path.display()))
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

/// A file as a body that is read while it is sent, and its size. The file is
/// opened again for each send, so a redirect hop sends it again.
async fn file_body(path: &std::path::Path) -> Result<(reqwest::Body, u64), HttpError> {
    let err = |e: std::io::Error| HttpError::File {
        path: path.display().to_string(),
        msg: e.to_string(),
    };
    let file = tokio::fs::File::open(path).await.map_err(err)?;
    let len = file.metadata().await.map_err(err)?.len();
    let body = FileBody {
        stream: tokio_util::io::ReaderStream::new(file),
        len,
    };
    Ok((reqwest::Body::wrap(body), len))
}

/// A file read in chunks while it is sent. Its exact size goes to the HTTP
/// layer, which frames it with `Content-Length` as it frames bytes.
struct FileBody {
    stream: tokio_util::io::ReaderStream<tokio::fs::File>,
    len: u64,
}

impl http_body::Body for FileBody {
    type Data = bytes::Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        use tokio_stream::Stream;
        Pin::new(&mut self.get_mut().stream)
            .poll_next(cx)
            .map(|chunk| chunk.map(|r| r.map(http_body::Frame::data)))
    }

    fn size_hint(&self) -> http_body::SizeHint {
        http_body::SizeHint::with_exact(self.len)
    }
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

const MAX_REDIRECTS: usize = 10;

/// The method and body of the next redirect hop (RFC 9110, 15.4): a 303
/// turns the request into a GET (a HEAD stays a HEAD), and so do 301 and 302
/// for a POST, as browsers do; the body is dropped then. 307 and 308 keep the
/// method and the body.
fn next_hop_method(
    status: reqwest::StatusCode,
    method: &reqwest::Method,
) -> Option<(reqwest::Method, bool)> {
    use reqwest::{Method, StatusCode};
    match status {
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND if *method == Method::POST => {
            Some((Method::GET, true))
        }
        StatusCode::SEE_OTHER if *method == Method::HEAD => Some((Method::HEAD, true)),
        StatusCode::SEE_OTHER => Some((Method::GET, true)),
        StatusCode::MOVED_PERMANENTLY
        | StatusCode::FOUND
        | StatusCode::TEMPORARY_REDIRECT
        | StatusCode::PERMANENT_REDIRECT => Some((method.clone(), false)),
        _ => None,
    }
}

/// The collection cookie jar, seen from the redirect loop: it stores the
/// cookies a redirect response sets and gives the `Cookie` header for the
/// next hop's URL.
pub trait RedirectCookies: Send + Sync {
    fn hop<'a>(
        &'a self,
        from: &'a str,
        set_cookies: Vec<String>,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send + 'a>>;
}

fn same_origin(a: &reqwest::Url, b: &reqwest::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

fn redirect_target(resp: &reqwest::Response) -> Option<reqwest::Url> {
    let location = resp.headers().get(reqwest::header::LOCATION)?;
    resp.url().join(location.to_str().ok()?).ok()
}

/// The headers of a redirect hop. After the chain has left the request's
/// origin, credentials stay behind: auth headers and `Authorization`,
/// `Cookie` and the like. Without a body, the headers that describe it go too.
/// The jar's `Cookie` is replaced by `jar` (the jar's header for this hop).
fn hop_headers(
    req: &ResolvedHttp,
    left_origin: bool,
    body_dropped: bool,
    jar: Option<Option<String>>,
) -> Result<HeaderMap, HttpError> {
    const PAYLOAD: [&str; 4] = [
        "content-type",
        "content-length",
        "content-encoding",
        "transfer-encoding",
    ];
    let mut kept: Vec<(String, String)> = req
        .headers
        .iter()
        .enumerate()
        .filter(|(i, (name, _))| {
            let lower = name.to_ascii_lowercase();
            let origin = req.header_origins.get(*i);
            let auth = origin == Some(&HeaderOrigin::Auth);
            let from_jar = origin == Some(&HeaderOrigin::Jar);
            let sensitive = matches!(
                lower.as_str(),
                "authorization" | "cookie" | "cookie2" | "proxy-authorization" | "www-authenticate"
            );
            !(jar.is_some() && from_jar)
                && !(left_origin && (auth || sensitive))
                && !(body_dropped && PAYLOAD.contains(&lower.as_str()))
        })
        .map(|(_, h)| h.clone())
        .collect();
    if let Some(Some(cookie)) = jar {
        kept.push(("Cookie".into(), cookie));
    }
    header_map(&kept)
}

fn send_error(e: reqwest::Error, timeout: Duration) -> HttpError {
    if e.is_timeout() {
        HttpError::Timeout(timeout.as_millis())
    } else {
        HttpError::Send(err_chain(&e))
    }
}

/// Sends one hop of a redirect chain with the time the whole chain has left,
/// so following redirects does not restart the request's timeout.
async fn execute_hop(
    client: &reqwest::Client,
    mut request: reqwest::Request,
    deadline: Option<Instant>,
    timeout: Duration,
) -> Result<reqwest::Response, HttpError> {
    let remaining = deadline.map_or(timeout, |d| d.saturating_duration_since(Instant::now()));
    if remaining.is_zero() {
        return Err(HttpError::Timeout(timeout.as_millis()));
    }
    *request.timeout_mut() = Some(remaining);
    client
        .execute(request)
        .await
        .map_err(|e| send_error(e, timeout))
}

/// Follows the redirects of `resp`, at most [`MAX_REDIRECTS`], each hop
/// within the time the chain has left. A 307 or 308 resends the body (a file
/// is read again); a multipart body is not resent, so its redirect response
/// is returned.
async fn follow_redirects(
    client: &reqwest::Client,
    req: &ResolvedHttp,
    mut resp: reqwest::Response,
    deadline: Option<Instant>,
    timeout: Duration,
    cookies: Option<&dyn RedirectCookies>,
) -> Result<reqwest::Response, HttpError> {
    let mut method = reqwest::Method::from_bytes(req.method.as_bytes())
        .map_err(|_| HttpError::Method(req.method.clone()))?;
    let (mut dropped, mut left_origin) = (false, false);
    for hop in 0.. {
        let Some((next, drop_body)) = next_hop_method(resp.status(), &method) else {
            return Ok(resp);
        };
        let Some(target) = redirect_target(&resp) else {
            return Ok(resp);
        };
        dropped |= drop_body;
        if !dropped && matches!(req.body, ResolvedBody::Multipart(_)) {
            return Ok(resp);
        }
        if hop >= MAX_REDIRECTS {
            return Err(HttpError::Send(format!(
                "too many redirects (more than {MAX_REDIRECTS})"
            )));
        }
        left_origin |= !same_origin(&target, resp.url());
        let jar = match cookies {
            Some(c) => {
                let sets: Vec<String> = resp
                    .headers()
                    .get_all(reqwest::header::SET_COOKIE)
                    .iter()
                    .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
                    .collect();
                Some(
                    c.hop(resp.url().as_str(), sets, target.as_str())
                        .await
                        .map_err(HttpError::Send)?,
                )
            }
            None => None,
        };
        method = next;
        let headers = hop_headers(req, left_origin, dropped, jar)?;
        let body = if dropped {
            &ResolvedBody::None
        } else {
            &req.body
        };
        let request = build_request(client, method.clone(), target, headers, body).await?;
        resp = execute_hop(client, request, deadline, timeout).await?;
    }
    Ok(resp)
}

async fn build_request(
    client: &reqwest::Client,
    method: reqwest::Method,
    url: reqwest::Url,
    headers: HeaderMap,
    body: &ResolvedBody,
) -> Result<reqwest::Request, HttpError> {
    // POST, PUT and PATCH define a meaning for content, so even without a body
    // they carry `Content-Length: 0` (RFC 9110, 8.6) unless the request frames
    // itself.
    let wants_length = matches!(method.as_str(), "POST" | "PUT" | "PATCH");
    let framed = headers.contains_key(reqwest::header::CONTENT_LENGTH)
        || headers.contains_key(reqwest::header::TRANSFER_ENCODING);
    let mut rb = client.request(method, url).headers(headers);
    // A defined but empty body is framed explicitly with Content-Length: 0.
    let empty_len = |rb: reqwest::RequestBuilder| {
        if framed {
            rb
        } else {
            rb.header(reqwest::header::CONTENT_LENGTH, "0")
        }
    };
    rb = match body {
        ResolvedBody::None if wants_length => empty_len(rb),
        ResolvedBody::None => rb,
        ResolvedBody::Bytes(b) if b.is_empty() => empty_len(rb),
        ResolvedBody::Bytes(b) => rb.body(b.clone()),
        ResolvedBody::File(p) => {
            let (body, len) = file_body(p).await?;
            if len == 0 {
                empty_len(rb)
            } else {
                rb.body(body)
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
                        let (body, len) = file_body(path).await?;
                        let ct = p
                            .content_type
                            .clone()
                            .unwrap_or_else(|| guess_mime(filename));
                        reqwest::multipart::Part::stream_with_length(body, len)
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
    rb.build().map_err(|e| HttpError::Send(err_chain(&e)))
}

/// Sends a resolved request and reads the whole response.
pub async fn send(req: &ResolvedHttp, opts: &SendOptions) -> Result<HttpResponse, HttpError> {
    send_with(req, opts, None).await
}

/// [`send`], with the cookie jar called on each redirect hop.
pub async fn send_with(
    req: &ResolvedHttp,
    opts: &SendOptions,
    cookies: Option<&dyn RedirectCookies>,
) -> Result<HttpResponse, HttpError> {
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
    let method = reqwest::Method::from_bytes(req.method.as_bytes())
        .map_err(|_| HttpError::Method(req.method.clone()))?;
    let headers = header_map(&req.headers)?;
    let marks: Shared = Arc::new(Mutex::new(Marks::default()));
    // PEM files are only read for TLS: a plain `http://` request must not fail
    // because `[http.tls]` points at a missing file, and a folder run must not
    // re-read them once per request.
    let plain = TlsMaterial::default();
    let material = if url.scheme() == "https" {
        &opts.tls
    } else {
        &plain
    };
    let mut tls =
        tls_config(req.verify_tls, &[b"h2", b"http/1.1"], material).map_err(HttpError::Tls)?;
    tls.resumption = Resumption::store(Arc::new(TimedSessions {
        inner: ClientSessionMemoryCache::new(8),
        marks: marks.clone(),
    }));
    let timeout = req
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(opts.default_timeout);
    let mut builder = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .dns_resolver(Arc::new(TimedResolver(marks.clone())))
        .connector_layer(TimingLayer(marks.clone()))
        // Redirects are followed by `follow_redirects`.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        // No implicit Referer on redirect hops.
        .referer(false)
        // HTTP/1 header names are written in Title-Case (`x-api-key` is sent as
        // `X-Api-Key`); names are case-insensitive. HTTP/2 always sends lowercase.
        .http1_title_case_headers();
    if let Some(spec) = &opts.proxy {
        builder = spec.apply(builder).map_err(HttpError::Send)?;
    }
    let client = builder
        .build()
        .map_err(|e| HttpError::Send(err_chain(&e)))?;

    let request = build_request(&client, method, url, headers, &req.body).await?;
    let start = Instant::now();
    let deadline = start.checked_add(timeout);
    let mut resp = client
        .execute(request)
        .await
        .map_err(|e| send_error(e, timeout))?;
    if req.follow_redirects {
        resp = follow_redirects(&client, req, resp, deadline, timeout, cookies).await?;
    }
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
        assert!(tls_config(false, &[b"h2"], &TlsMaterial::default()).is_ok());
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

    #[test]
    fn proxy_spec_parse() {
        assert_eq!(ProxySpec::parse("env").unwrap(), ProxySpec::Env);
        assert_eq!(ProxySpec::parse(" ENV ").unwrap(), ProxySpec::Env);
        for ok in [
            "http://proxy:3128",
            "https://proxy:3128",
            "socks5://proxy",
            "socks5h://proxy:1080",
            "socks4://proxy",
            "http://user:pass@proxy:3128",
        ] {
            assert_eq!(ProxySpec::parse(ok).unwrap(), ProxySpec::Url(ok.into()));
        }
        for bad in ["proxy:3128", "ftp://proxy", "", "localhost"] {
            assert!(ProxySpec::parse(bad).is_err(), "{bad:?} should not parse");
        }
    }

    #[test]
    fn proxy_env_variables() {
        let get = |k: &str| match k {
            "ALL_PROXY" => Some("http://all:9".to_string()),
            "http_proxy" => Some("http://low:1".to_string()),
            "HTTPS_PROXY" => Some("  ".to_string()), // empty: skipped
            _ => None,
        };
        assert_eq!(env_proxy("HTTP_PROXY", &get), Some("http://low:1".into()));
        assert_eq!(env_proxy("HTTPS_PROXY", &get), None);
        assert_eq!(env_proxy("ALL_PROXY", &get), Some("http://all:9".into()));
        assert_eq!(env_proxy("NO_PROXY", &get), None);
    }

    #[test]
    fn proxy_env_routing_precedence() {
        let pairs = |vars: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                vars.iter()
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| (*v).to_string())
            }
        };
        // The scheme variable beats ALL_PROXY, for its own scheme only.
        let (https, http) = env_proxy_urls(&pairs(&[
            ("ALL_PROXY", "http://all:9"),
            ("HTTPS_PROXY", "http://secure:8"),
            ("HTTP_PROXY", "http://plain:7"),
        ]));
        assert_eq!(https.as_deref(), Some("http://secure:8"));
        assert_eq!(http.as_deref(), Some("http://plain:7"));

        // ALL_PROXY fills the scheme that has no variable.
        let (https, http) = env_proxy_urls(&pairs(&[
            ("ALL_PROXY", "http://all:9"),
            ("HTTP_PROXY", "http://p:7"),
        ]));
        assert_eq!(https.as_deref(), Some("http://all:9"));
        assert_eq!(http.as_deref(), Some("http://p:7"));

        // Lowercase is read, and an empty uppercase does not hide it.
        let (https, http) = env_proxy_urls(&pairs(&[
            ("HTTPS_PROXY", ""),
            ("https_proxy", "http://low:1"),
        ]));
        assert_eq!(https.as_deref(), Some("http://low:1"));
        assert_eq!(http, None);

        // Nothing set: both direct.
        let (https, http) = env_proxy_urls(&pairs(&[]));
        assert_eq!(https, None);
        assert_eq!(http, None);
    }

    #[test]
    fn proxy_errors_hide_credentials() {
        assert_eq!(
            redact_url("http://user:secret@proxy:3128"),
            "http://proxy:3128"
        );
        assert_eq!(
            redact_url("socks5h://u:p@proxy/path?q=1"),
            "socks5h://proxy/path?q=1"
        );
        assert_eq!(redact_url("http://proxy:3128"), "http://proxy:3128");
        assert_eq!(redact_url("proxy:3128"), "proxy:3128");

        let e = ProxySpec::parse("ftp://user:secret@proxy:1").unwrap_err();
        assert!(e.contains("proxy:1"), "{e}");
        assert!(!e.contains("secret") && !e.contains("user:"), "{e}");

        // The underlying error may repeat the URL: it is redacted too.
        let e = proxy_error(
            "HTTPS_PROXY",
            "http://user:secret@proxy:1",
            "unsupported proxy http://user:secret@proxy:1",
        );
        assert!(!e.contains("secret"), "{e}");
        assert!(e.contains("http://proxy:1"), "{e}");
    }

    #[test]
    fn proxy_debug_hides_credentials() {
        let spec = ProxySpec::Url("http://user:secret@proxy:3128".into());
        let dbg = format!("{spec:?}");
        assert!(!dbg.contains("secret") && !dbg.contains("user:"), "{dbg}");
        assert!(dbg.contains("http://proxy:3128"), "{dbg}");
        assert_eq!(format!("{:?}", ProxySpec::Env), "Env");
    }

    #[test]
    fn tls_material_pair_rules() {
        assert!(TlsMaterial::default().is_empty());
        assert_eq!(TlsMaterial::default().pair_error(), None);
        let d = tempfile::tempdir().unwrap();
        let m = |c: bool, k: bool| TlsMaterial {
            ca: None,
            client_cert: c.then(|| d.path().join("c.pem")),
            client_key: k.then(|| d.path().join("k.pem")),
        };
        assert_eq!(m(true, true).pair_error(), None);
        assert!(
            m(true, false)
                .pair_error()
                .is_some_and(|e| e.contains("client_cert"))
        );
        assert!(
            m(false, true)
                .pair_error()
                .is_some_and(|e| e.contains("client_key"))
        );
    }

    #[test]
    fn tls_config_reports_bad_material() {
        let d = tempfile::tempdir().unwrap();
        let missing = d.path().join("missing.pem");
        let m = TlsMaterial {
            ca: Some(missing.clone()),
            client_cert: None,
            client_key: None,
        };
        let e = tls_config(true, &[], &m).unwrap_err();
        assert!(e.contains("missing.pem"), "{e}");

        let empty = d.path().join("empty.pem");
        std::fs::write(&empty, b"nothing here\n").unwrap();
        let m = TlsMaterial {
            ca: Some(empty.clone()),
            client_cert: None,
            client_key: None,
        };
        let e = tls_config(true, &[], &m).unwrap_err();
        assert!(e.contains("no certificates"), "{e}");

        // An unparseable client key is reported with its path.
        let bad = d.path().join("bad.key");
        let cert = d.path().join("bad.pem");
        std::fs::write(&bad, b"garbage").unwrap();
        std::fs::write(
            &cert,
            b"-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        )
        .unwrap();
        let m = TlsMaterial {
            ca: None,
            client_cert: Some(cert),
            client_key: Some(bad.clone()),
        };
        let e = tls_config(true, &[], &m).unwrap_err();
        assert!(e.contains("bad.key"), "{e}");
    }

    async fn read_request(s: &mut tokio::net::TcpStream) -> Vec<u8> {
        let mut buf = vec![0u8; 65536];
        let mut got = Vec::new();
        loop {
            let head = got.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4);
            let text = String::from_utf8_lossy(&got).to_ascii_lowercase();
            let len = text
                .split("\r\n")
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if head.is_some_and(|h| got.len() >= h + len) {
                break;
            }
            let n = s.read(&mut buf).await.unwrap();
            if n == 0 {
                break;
            }
            got.extend_from_slice(&buf[..n]);
        }
        got
    }

    async fn chain(responses: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        delayed_chain(responses.into_iter().map(|r| (Duration::ZERO, r)).collect()).await
    }

    /// Each response is written after its own pause.
    async fn delayed_chain(
        responses: Vec<(Duration, String)>,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        let h = tokio::spawn(async move {
            let mut all = Vec::new();
            for (delay, response) in responses {
                let Ok(Ok((mut s, _))) =
                    tokio::time::timeout(Duration::from_secs(5), l.accept()).await
                else {
                    break;
                };
                let got = read_request(&mut s).await;
                tokio::time::sleep(delay).await;
                let _ = s.write_all(response.as_bytes()).await;
                s.shutdown().await.ok();
                all.push(String::from_utf8_lossy(&got).into_owned());
            }
            all
        });
        (format!("http://localhost:{}", addr.port()), h)
    }

    async fn replier(response: String) -> (String, tokio::task::JoinHandle<String>) {
        let (url, h) = chain(vec![response]).await;
        let h = tokio::spawn(async move { h.await.unwrap().remove(0) });
        (url, h)
    }

    fn with_api_key(r: &mut ResolvedHttp) {
        r.headers = vec![
            ("X-Api-Key".into(), "k".into()),
            ("X-Trace".into(), "t".into()),
        ];
        r.header_origins = vec![HeaderOrigin::Auth, HeaderOrigin::User];
    }

    const OK: &str = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok";

    #[tokio::test]
    async fn redirect_to_another_origin_drops_auth_headers() {
        let (b, hb) = replier(OK.into()).await;
        let b_ip = b.replacen("localhost", "127.0.0.1", 1);
        let (a, ha) = replier(format!(
            "HTTP/1.1 302 Found\r\nLocation: {b_ip}/next\r\nContent-Length: 0\r\n\r\n"
        ))
        .await;
        let mut r = req(a);
        with_api_key(&mut r);
        let resp = send(&r, &SendOptions::default()).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.url, format!("{b_ip}/next"));
        let first = ha.await.unwrap();
        assert!(first.contains("X-Api-Key: k"), "{first}");
        let second = hb.await.unwrap();
        assert!(second.starts_with("GET /next HTTP/1.1"), "{second}");
        assert!(second.contains("X-Trace: t"), "{second}");
        assert!(
            !second.to_ascii_lowercase().contains("x-api-key"),
            "{second}"
        );

        let (u, hc) = chain(vec![
            "HTTP/1.1 302 Found\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n".into(),
            OK.into(),
        ])
        .await;
        let mut r = req(u.clone());
        with_api_key(&mut r);
        let resp = send(&r, &SendOptions::default()).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.url, format!("{u}/next"));
        let caps = hc.await.unwrap();
        assert!(caps[0].contains("X-Api-Key: k"), "{}", caps[0]);
        assert!(
            caps[1].starts_with("GET /next HTTP/1.1") && caps[1].contains("X-Api-Key: k"),
            "{}",
            caps[1]
        );

        let (b, hb) = replier(OK.into()).await;
        let (a, _) = replier(format!(
            "HTTP/1.1 302 Found\r\nLocation: {b}/next\r\nContent-Length: 0\r\n\r\n"
        ))
        .await;
        let mut r = req(a);
        r.follow_redirects = false;
        with_api_key(&mut r);
        let resp = send(&r, &SendOptions::default()).await.unwrap();
        assert_eq!(resp.status, 302);
        assert!(resp.header("location").is_some());
        drop(hb);
    }

    #[tokio::test]
    async fn redirect_to_another_origin_keeps_the_method_rules() {
        let (b, hb) = replier(OK.into()).await;
        let b_ip = b.replacen("localhost", "127.0.0.1", 1);
        let (a, _) = replier(format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {b_ip}/next\r\nContent-Length: 0\r\n\r\n"
        ))
        .await;
        let mut r = req(a);
        r.method = "POST".into();
        with_api_key(&mut r);
        r.body = ResolvedBody::Bytes(b"hello".to_vec());
        let resp = send(&r, &SendOptions::default()).await.unwrap();
        assert_eq!(resp.status, 200);
        let second = hb.await.unwrap();
        assert!(second.starts_with("POST /next HTTP/1.1"), "{second}");
        assert!(second.ends_with("hello"), "{second}");
        assert!(second.contains("Content-Length: 5"), "{second}");
        assert!(
            !second.to_ascii_lowercase().contains("x-api-key"),
            "{second}"
        );

        let (b, hb) = replier(OK.into()).await;
        let b_ip = b.replacen("localhost", "127.0.0.1", 1);
        let (a, _) = replier(format!(
            "HTTP/1.1 303 See Other\r\nLocation: {b_ip}/next\r\nContent-Length: 0\r\n\r\n"
        ))
        .await;
        let mut r = req(a);
        r.method = "POST".into();
        r.headers = vec![
            ("Content-Type".into(), "application/json".into()),
            ("X-Api-Key".into(), "k".into()),
        ];
        r.header_origins = vec![HeaderOrigin::Body, HeaderOrigin::Auth];
        r.body = ResolvedBody::Bytes(b"{}".to_vec());
        let resp = send(&r, &SendOptions::default()).await.unwrap();
        assert_eq!(resp.status, 200);
        let second = hb.await.unwrap();
        assert!(second.starts_with("GET /next HTTP/1.1"), "{second}");
        let lower = second.to_ascii_lowercase();
        for absent in ["content-type", "content-length", "x-api-key"] {
            assert!(!lower.contains(absent), "{absent} in {second}");
        }
        assert!(second.ends_with("\r\n\r\n"), "{second}");
    }

    #[tokio::test]
    async fn redirect_does_not_resend_a_multipart_body() {
        let (b, hb) = replier(OK.into()).await;
        let b_ip = b.replacen("localhost", "127.0.0.1", 1);
        let (a, _) = replier(format!(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: {b_ip}/next\r\nContent-Length: 0\r\n\r\n"
        ))
        .await;
        let mut r = req(a);
        r.method = "POST".into();
        with_api_key(&mut r);
        r.body = ResolvedBody::Multipart(vec![sendito_core::resolve::ResolvedPart {
            name: "t".into(),
            data: PartData::Text("v".into()),
            content_type: None,
        }]);
        let resp = tokio::time::timeout(Duration::from_secs(5), send(&r, &SendOptions::default()))
            .await
            .expect("send did not return")
            .unwrap();
        assert_eq!(resp.status, 307);
        assert!(
            tokio::time::timeout(Duration::from_millis(300), hb)
                .await
                .is_err(),
            "the other origin received a request"
        );

        let (u, _) = replier(
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n"
                .into(),
        )
        .await;
        r.url = u;
        let resp = tokio::time::timeout(Duration::from_secs(5), send(&r, &SendOptions::default()))
            .await
            .expect("send did not return")
            .unwrap();
        assert_eq!(resp.status, 307);
    }

    #[tokio::test]
    async fn a_307_sends_a_file_body_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("body.bin");
        std::fs::write(&file, b"file-bytes").unwrap();
        let (u, hc) = chain(vec![
            "HTTP/1.1 307 Temporary Redirect\r\nLocation: /next\r\nContent-Length: 0\r\n\r\n"
                .into(),
            OK.into(),
        ])
        .await;
        let mut r = req(u);
        r.method = "PUT".into();
        r.body = ResolvedBody::File(file);
        let resp = send(&r, &SendOptions::default()).await.unwrap();
        assert_eq!(resp.status, 200);
        let caps = hc.await.unwrap();
        for cap in &caps {
            assert!(cap.contains("Content-Length: 10"), "{cap}");
            assert!(cap.ends_with("file-bytes"), "{cap}");
        }
        assert!(caps[1].starts_with("PUT /next HTTP/1.1"), "{}", caps[1]);
    }

    #[tokio::test]
    async fn the_timeout_covers_the_whole_redirect_chain() {
        let pause = Duration::from_millis(150);
        let hop = |to: &str| {
            (
                pause,
                format!("HTTP/1.1 302 Found\r\nLocation: {to}/next\r\nContent-Length: 0\r\n\r\n"),
            )
        };
        // Three other origins, each answering after 150 ms.
        let (c, hc) = delayed_chain(vec![(pause, OK.into())]).await;
        let (b, hb) = delayed_chain(vec![hop(&c)]).await;
        let (a, _) = delayed_chain(vec![hop(&b)]).await;
        let mut r = req(a);
        with_api_key(&mut r);
        r.timeout_ms = Some(400);
        assert!(matches!(
            send(&r, &SendOptions::default()).await,
            Err(HttpError::Timeout(400))
        ));
        // Both hops were followed: the time ran out on the third origin.
        for reached in [hb, hc] {
            assert_eq!(reached.await.unwrap().len(), 1);
        }

        let (c, _) = delayed_chain(vec![(pause, OK.into())]).await;
        let (b, _) = delayed_chain(vec![hop(&c)]).await;
        let (a, _) = delayed_chain(vec![hop(&b)]).await;
        let mut r = req(a);
        with_api_key(&mut r);
        r.timeout_ms = Some(2_000);
        let resp = send(&r, &SendOptions::default()).await.unwrap();
        assert_eq!(resp.status, 200);
        assert_eq!(resp.url, format!("{c}/next"));
    }

    #[tokio::test]
    async fn a_hop_with_no_time_left_is_not_sent() {
        let (url, seen) = replier(OK.into()).await;
        let tls = tls_config(false, &[b"h2"], &TlsMaterial::default()).unwrap();
        let client = reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .build()
            .unwrap();
        let request = client.get(&url).build().unwrap();
        let spent = Instant::now() - Duration::from_millis(1);
        let err = execute_hop(&client, request, Some(spent), Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(matches!(err, HttpError::Timeout(1000)));
        assert!(
            tokio::time::timeout(Duration::from_millis(300), seen)
                .await
                .is_err(),
            "the server received the request"
        );
    }
}
