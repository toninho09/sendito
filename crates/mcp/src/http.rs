//! The streamable HTTP transport: one JSON-RPC object per `POST`, the response
//! arriving either as `application/json` or as a `text/event-stream` whose
//! `data:` events carry JSON-RPC messages (per the 2025-03-26 revision).
//!
//! Server-initiated messages on a background `GET` stream are not read; they
//! are listed in `docs/DEBT.md`.

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::Value;
use tokio_stream::StreamExt;

use crate::{McpError, TokenProvider, Wire};

/// Builds a connection. `headers` are sent with every POST (static auth, etc.).
pub(crate) fn connect(
    url: &str,
    headers: &[(String, String)],
    verify_tls: bool,
    proxy: Option<sendito_http::ProxySpec>,
    tls: sendito_http::TlsMaterial,
    auth: Option<Arc<dyn TokenProvider>>,
) -> Result<Box<dyn Wire>, McpError> {
    // Fail early with the URL in the message: an unresolved `{{variable}}`
    // renders to an invalid URL, and reqwest would only say "builder error".
    reqwest::Url::parse(url).map_err(|e| {
        McpError::Transport(format!(
            "invalid MCP server URL {url:?}: {e} (is a variable unresolved?)"
        ))
    })?;
    // Advertise ALPN like the HTTP client: a server that only speaks h2 must
    // still negotiate.
    let tls = sendito_http::tls_config(verify_tls, &[b"h2", b"http/1.1"], &tls)
        .map_err(McpError::Transport)?;
    let mut builder = reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        .redirect(reqwest::redirect::Policy::none());
    if let Some(spec) = &proxy {
        builder = spec.apply(builder).map_err(McpError::Transport)?;
    }
    let client = builder
        .build()
        .map_err(|e| McpError::Transport(e.to_string()))?;
    Ok(Box::new(HttpConn {
        client,
        url: url.to_string(),
        headers: headers.to_vec(),
        auth,
        session_id: None,
        protocol_version: None,
        pending: Pending::None,
    }))
}

type ByteStream = Pin<Box<dyn tokio_stream::Stream<Item = reqwest::Result<bytes::Bytes>> + Send>>;

enum Pending {
    /// No response body (a `202 Accepted` for a notification).
    None,
    /// A complete `application/json` body, parsed on the first `recv`.
    Json(Vec<u8>),
    /// An `text/event-stream` body, read event by event.
    Sse(ByteStream, SseBuf),
}

/// The largest response body (JSON, or one SSE event) read from a server.
const MAX_BODY: usize = 100 * 1024 * 1024;
/// How much of an error body is read for its message.
const MAX_ERROR_BODY: usize = 4096;

fn too_large() -> McpError {
    McpError::Transport(format!(
        "the server sent more than {} MiB",
        MAX_BODY / (1024 * 1024)
    ))
}

/// Reads a body of at most `max` bytes. With `cut`, a longer body is cut at
/// `max`; without, it is an error.
async fn read_body(
    mut resp: reqwest::Response,
    max: usize,
    cut: bool,
) -> Result<Vec<u8>, McpError> {
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?
    {
        if body.len() + chunk.len() > max {
            if !cut {
                return Err(too_large());
            }
            body.extend_from_slice(&chunk[..max - body.len()]);
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

struct HttpConn {
    client: reqwest::Client,
    url: String,
    headers: Vec<(String, String)>,
    auth: Option<Arc<dyn TokenProvider>>,
    /// From the initialize response; sent with every later POST.
    session_id: Option<String>,
    protocol_version: Option<String>,
    pending: Pending,
}

impl HttpConn {
    fn has_explicit_authorization(&self) -> bool {
        self.headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
    }

    fn build_request(&self, line: &str, bearer: Option<&str>) -> reqwest::RequestBuilder {
        let mut req = self
            .client
            .post(&self.url)
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .header(
                reqwest::header::ACCEPT,
                "application/json, text/event-stream",
            )
            .body(line.to_owned());
        for (k, v) in &self.headers {
            req = req.header(k.as_str(), v.as_str());
        }
        // An Authorization header in the configuration wins, as in HTTP
        // requests: two Authorization headers would reach the server.
        if let (Some(token), false) = (bearer, self.has_explicit_authorization()) {
            req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if let Some(sid) = &self.session_id {
            req = req.header("mcp-session-id", sid);
        }
        if let Some(version) = &self.protocol_version {
            req = req.header("mcp-protocol-version", version);
        }
        req
    }

    /// Consumes a response: fails on bad statuses, stores the session id and
    /// parks the body (JSON or SSE) for `recv`.
    async fn take_response(&mut self, resp: reqwest::Response) -> Result<(), McpError> {
        let status = resp.status();
        // A Bearer challenge asks for a sign-in, unless the configuration
        // sends its own Authorization header (then the header is wrong).
        if status == reqwest::StatusCode::UNAUTHORIZED
            && !self.has_explicit_authorization()
            && resp
                .headers()
                .get(reqwest::header::WWW_AUTHENTICATE)
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.trim_start().to_ascii_lowercase().starts_with("bearer"))
        {
            return Err(McpError::NeedsLogin(
                "the server asks you to sign in (HTTP 401)".into(),
            ));
        }
        if !status.is_success() {
            let body = read_body(resp, MAX_ERROR_BODY, true)
                .await
                .unwrap_or_default();
            let preview: String = String::from_utf8_lossy(&body).chars().take(200).collect();
            return Err(McpError::Transport(format!("HTTP {status}: {preview}")));
        }
        if let Some(sid) = resp
            .headers()
            .get("mcp-session-id")
            .and_then(|v| v.to_str().ok())
        {
            self.session_id = Some(sid.to_string());
        }
        let ctype = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_ascii_lowercase();
        if ctype.contains("text/event-stream") {
            self.pending = Pending::Sse(Box::pin(resp.bytes_stream()), SseBuf::default());
        } else {
            let body = read_body(resp, MAX_BODY, false).await?;
            self.pending = if body.is_empty() {
                Pending::None
            } else {
                Pending::Json(body)
            };
        }
        Ok(())
    }
}

impl Wire for HttpConn {
    fn set_protocol_version(&mut self, version: &str) {
        if !version.is_empty() {
            self.protocol_version = Some(version.to_string());
        }
    }

    fn send<'a>(
        &'a mut self,
        line: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        Box::pin(async move {
            let bearer = match &self.auth {
                Some(p) => Some(p.token(false).await?),
                None => None,
            };
            let resp = self
                .build_request(line, bearer.as_deref())
                .send()
                .await
                .map_err(|e| McpError::Transport(e.to_string()))?;
            if resp.status() == reqwest::StatusCode::UNAUTHORIZED
                && let Some(p) = &self.auth
            {
                // The token was refused: force one refresh and retry once.
                let fresh = p.token(true).await?;
                let retry = self
                    .build_request(line, Some(&fresh))
                    .send()
                    .await
                    .map_err(|e| McpError::Transport(e.to_string()))?;
                return self.take_response(retry).await;
            }
            self.take_response(resp).await
        })
    }

    fn recv<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Value, McpError>> + Send + 'a>> {
        Box::pin(async move {
            let msg = match &mut self.pending {
                Pending::None => {
                    return Err(McpError::Message(
                        "the server replied with no body to a request".into(),
                    ));
                }
                Pending::Json(body) => {
                    let body = std::mem::take(body);
                    self.pending = Pending::None;
                    return serde_json::from_slice(&body)
                        .map_err(|e| McpError::Json(e.to_string()));
                }
                Pending::Sse(stream, buf) => loop {
                    match buf.next_event() {
                        Some(Some(data)) => {
                            if let Some(msg) = parse_sse_data(&data) {
                                break msg;
                            }
                        }
                        // A comment or a keep-alive: the buffer may hold more.
                        Some(None) => {}
                        None => match stream.next().await {
                            Some(Ok(chunk)) => buf.push(&chunk)?,
                            Some(Err(e)) => return Err(McpError::Transport(e.to_string())),
                            None => return Err(McpError::Closed),
                        },
                    }
                },
            };
            Ok(msg)
        })
    }
}

/// The unread part of an SSE body. `scanned` is how far the search for an
/// event end got, so each chunk is searched once.
#[derive(Default)]
struct SseBuf {
    buf: Vec<u8>,
    scanned: usize,
}

impl SseBuf {
    fn push(&mut self, chunk: &[u8]) -> Result<(), McpError> {
        if self.buf.len() + chunk.len() > MAX_BODY {
            return Err(too_large());
        }
        self.buf.extend_from_slice(chunk);
        Ok(())
    }

    /// Takes the first complete event (a blank line — `\n\n` or `\r\n\r\n` —
    /// ends one) and joins its `data:` lines with `\n`, as the SSE format
    /// defines. Other fields (`event:`, `id:`, comments) are ignored.
    /// `None`: no complete event yet. `Some(None)`: an event with no data.
    fn next_event(&mut self) -> Option<Option<String>> {
        // A separator can start in the 3 bytes before the unscanned part.
        let from = self.scanned.saturating_sub(3);
        let Some((end, sep)) = event_end(&self.buf[from..]).map(|(e, s)| (e + from, s)) else {
            self.scanned = self.buf.len();
            return None;
        };
        let block: Vec<u8> = self.buf.drain(..end).collect();
        self.scanned = 0;
        let text = String::from_utf8_lossy(&block[..end - sep]);
        let mut data: Vec<&str> = Vec::new();
        for line in text.split('\n') {
            let line = line.strip_suffix('\r').unwrap_or(line);
            if let Some(d) = line.strip_prefix("data:") {
                data.push(d.strip_prefix(' ').unwrap_or(d));
            }
        }
        Some((!data.is_empty()).then(|| data.join("\n")))
    }
}

/// Where the first event in `buf` ends, and the length of its separator.
fn event_end(buf: &[u8]) -> Option<(usize, usize)> {
    // `\n\n` never matches inside `\r\n\r\n`, so the earliest end wins.
    let nl = buf
        .windows(2)
        .position(|w| w == b"\n\n")
        .map(|i| (i + 2, 2));
    let crlf = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i + 4, 4));
    match (nl, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn parse_sse_data(data: &str) -> Option<Value> {
    serde_json::from_str(data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sse(bytes: &[u8]) -> SseBuf {
        let mut b = SseBuf::default();
        b.push(bytes).unwrap();
        b
    }

    #[test]
    fn sse_events_join_data_lines() {
        let mut buf = sse(b": ping\n\n");
        assert_eq!(buf.next_event(), Some(None));
        assert_eq!(buf.next_event(), None);
        buf.push(b"event: message\ndata: {\"id\":1}\ndata: \n\n")
            .unwrap();
        assert_eq!(buf.next_event(), Some(Some("{\"id\":1}\n".into())));
        assert!(buf.buf.is_empty());
    }

    #[test]
    fn sse_handles_crlf_and_prefix_space() {
        let mut buf = sse(b"data:{\"a\":true}\r\n\r\n");
        assert_eq!(buf.next_event(), Some(Some("{\"a\":true}".into())));
    }

    #[test]
    fn sse_finds_a_separator_split_across_chunks() {
        let mut buf = sse(b"data: 1\r\n");
        assert_eq!(buf.next_event(), None);
        buf.push(b"\r").unwrap();
        assert_eq!(buf.next_event(), None);
        buf.push(b"\ndata: 2\n").unwrap();
        assert_eq!(buf.next_event(), Some(Some("1".into())));
        assert_eq!(buf.next_event(), None);
        buf.push(b"\n").unwrap();
        assert_eq!(buf.next_event(), Some(Some("2".into())));
    }

    #[test]
    fn a_keep_alive_before_the_reply_in_one_chunk_is_not_a_closed_stream() {
        let mut buf = sse(b": keep-alive\n\ndata: {}\n\n");
        assert_eq!(buf.next_event(), Some(None));
        assert_eq!(buf.next_event(), Some(Some("{}".into())));
    }
}
