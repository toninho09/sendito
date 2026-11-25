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

pub mod auth;
mod body;
pub mod graphql;
mod proxy;
mod redirect;
pub mod sse;
mod timing;
mod tls;
pub use body::*;
pub use proxy::*;
pub use redirect::*;
pub use timing::*;
pub use tls::*;

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
        sendito_core::script::headers_size(&self.headers)
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

fn send_error(e: reqwest::Error, timeout: Duration) -> HttpError {
    if e.is_timeout() {
        HttpError::Timeout(timeout.as_millis())
    } else {
        HttpError::Send(err_chain(&e))
    }
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
        ResolvedBody::Invalid(msg) => return Err(HttpError::Send(msg.clone())),
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
    open(req, opts, cookies).await?.read_body().await
}

/// Opens an HTTP response with the usual whole-response timeout.
pub async fn open(
    req: &ResolvedHttp,
    opts: &SendOptions,
    cookies: Option<&dyn RedirectCookies>,
) -> Result<Opened, HttpError> {
    open_with(req, opts, cookies, false).await
}

/// Opens a live response. The timeout ends when the final headers arrive.
pub async fn open_stream(
    req: &ResolvedHttp,
    opts: &SendOptions,
    cookies: Option<&dyn RedirectCookies>,
) -> Result<Opened, HttpError> {
    open_with(req, opts, cookies, true).await
}

async fn open_with(
    req: &ResolvedHttp,
    opts: &SendOptions,
    cookies: Option<&dyn RedirectCookies>,
    streaming: bool,
) -> Result<Opened, HttpError> {
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
        // No implicit Referer on redirect hops.
        .referer(false)
        // HTTP/1 header names are written in Title-Case (`x-api-key` is sent as
        // `X-Api-Key`); names are case-insensitive. HTTP/2 always sends lowercase.
        .http1_title_case_headers();
    if !streaming {
        builder = builder.timeout(timeout);
    }
    if let Some(spec) = &opts.proxy {
        builder = spec.apply(builder).map_err(HttpError::Send)?;
    }
    let client = builder
        .build()
        .map_err(|e| HttpError::Send(err_chain(&e)))?;

    let request = build_request(&client, method, url, headers, &req.body).await?;
    let start = Instant::now();
    let deadline = start.checked_add(timeout);
    let opening = async {
        let mut resp = client
            .execute(request)
            .await
            .map_err(|e| send_error(e, timeout))?;
        if req.follow_redirects {
            resp = if streaming {
                redirect::follow_redirects_with_mode(
                    &client, req, resp, deadline, timeout, cookies, true,
                )
                .await?
            } else {
                follow_redirects(&client, req, resp, deadline, timeout, cookies).await?
            };
        }
        Ok::<_, HttpError>(resp)
    };
    let resp = if streaming {
        tokio::time::timeout(timeout, opening)
            .await
            .map_err(|_| HttpError::Timeout(timeout.as_millis()))??
    } else {
        opening.await?
    };
    let headers_at = Instant::now();
    Ok(Opened {
        resp,
        marks,
        start,
        headers_at,
        timeout,
        max_body: opts.max_body,
    })
}

/// An HTTP response whose body is still available for reading.
pub struct Opened {
    resp: reqwest::Response,
    marks: Shared,
    start: Instant,
    headers_at: Instant,
    timeout: Duration,
    max_body: usize,
}

impl Opened {
    /// Reads the response with the normal HTTP size and decoding rules.
    pub async fn read_body(self) -> Result<HttpResponse, HttpError> {
        let Self {
            mut resp,
            marks,
            start,
            headers_at,
            timeout,
            max_body,
        } = self;
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
                    if body.len() + c.len() > max_body {
                        let room = max_body.saturating_sub(body.len());
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
            decode_body(&headers, body, max_body)
        };
        truncated |= decoded_truncated;

        let timing = measured_timing(&marks, start, headers_at, end);
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

    /// Gives the body to a live consumer without collecting it.
    pub fn into_stream(self) -> HttpStream {
        HttpStream {
            response: self.resp,
            timing: measured_timing(&self.marks, self.start, self.headers_at, self.headers_at),
        }
    }
}

fn measured_timing(marks: &Shared, start: Instant, headers_at: Instant, end: Instant) -> Timing {
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
    Timing {
        dns,
        connect,
        tls: tls_d,
        ttfb: headers_at.saturating_duration_since(ready),
        download: end.saturating_duration_since(headers_at),
        total: end.saturating_duration_since(start),
    }
}

/// A live HTTP body. Dropping it closes the response.
pub struct HttpStream {
    response: reqwest::Response,
    pub timing: Timing,
}

impl HttpStream {
    pub fn head(&self) -> HttpResponse {
        HttpResponse {
            status: self.status(),
            reason: self
                .response
                .status()
                .canonical_reason()
                .unwrap_or("")
                .into(),
            version: format!("{:?}", self.response.version()),
            url: self.url().into(),
            remote_addr: self.response.remote_addr(),
            headers: self.headers(),
            body: Vec::new(),
            truncated: false,
            decoded: None,
            timing: self.timing,
        }
    }
    pub fn status(&self) -> u16 {
        self.response.status().as_u16()
    }
    pub fn url(&self) -> &str {
        self.response.url().as_str()
    }
    pub fn headers(&self) -> Vec<(String, String)> {
        self.response
            .headers()
            .iter()
            .map(|(name, value)| {
                (
                    name.to_string(),
                    String::from_utf8_lossy(value.as_bytes()).into_owned(),
                )
            })
            .collect()
    }
    pub async fn chunk(&mut self) -> Result<Option<bytes::Bytes>, HttpError> {
        self.response
            .chunk()
            .await
            .map_err(|e| HttpError::Send(format!("reading stream: {}", err_chain(&e))))
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
            // Read the whole request first: a reply sent while the client
            // still writes the body breaks the client's pipe.
            loop {
                let n = s.read(&mut buf).await.unwrap();
                got.extend_from_slice(&buf[..n]);
                if n == 0 || request_complete(&got) {
                    break;
                }
            }
            s.write_all(response).await.unwrap();
            s.shutdown().await.ok();
            got
        });
        (format!("http://localhost:{}", addr.port()), h)
    }

    /// True when `got` holds the head and the whole body of one request.
    fn request_complete(got: &[u8]) -> bool {
        let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") else {
            return false;
        };
        let head = String::from_utf8_lossy(&got[..end]).to_ascii_lowercase();
        let body = &got[end + 4..];
        if head.contains("transfer-encoding: chunked") {
            return body.ends_with(b"0\r\n\r\n");
        }
        let len = head
            .lines()
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok())
            .unwrap_or(0);
        body.len() >= len
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
            signed: None,
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
