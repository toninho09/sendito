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
    Sse(ByteStream, Vec<u8>),
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
        let explicit = self
            .headers
            .iter()
            .any(|(k, _)| k.eq_ignore_ascii_case("authorization"));
        if let (Some(token), false) = (bearer, explicit) {
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
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            let preview: String = body.chars().take(200).collect();
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
            self.pending = Pending::Sse(Box::pin(resp.bytes_stream()), Vec::new());
        } else {
            let body = resp
                .bytes()
                .await
                .map_err(|e| McpError::Transport(e.to_string()))?;
            self.pending = if body.is_empty() {
                Pending::None
            } else {
                Pending::Json(body.to_vec())
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
                    if let Some(data) = take_sse_event(buf) {
                        if let Some(msg) = parse_sse_data(&data) {
                            break msg;
                        }
                        // A comment or a keep-alive: read on.
                    } else {
                        match stream.next().await {
                            Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
                            Some(Err(e)) => return Err(McpError::Transport(e.to_string())),
                            None => return Err(McpError::Closed),
                        }
                    }
                },
            };
            Ok(msg)
        })
    }
}

/// Takes the first complete event (a blank line — `\n\n` or `\r\n\r\n` —
/// ends one) and joins its `data:` lines with `\n`, as the SSE format defines.
/// Other fields (`event:`, `id:`, comments) are ignored.
fn take_sse_event(buf: &mut Vec<u8>) -> Option<String> {
    // `\n\n` never matches inside `\r\n\r\n`, so the earliest end wins.
    let (end, sep) = {
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
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }?;
    let block: Vec<u8> = buf.drain(..end).collect();
    let text = String::from_utf8_lossy(&block[..end - sep]);
    let mut data: Vec<&str> = Vec::new();
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(d) = line.strip_prefix("data:") {
            data.push(d.strip_prefix(' ').unwrap_or(d));
        }
    }
    if data.is_empty() {
        None
    } else {
        Some(data.join("\n"))
    }
}

fn parse_sse_data(data: &str) -> Option<Value> {
    serde_json::from_str(data).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sse_events_join_data_lines() {
        let mut buf = b": ping\n\n".to_vec();
        assert_eq!(take_sse_event(&mut buf), None);
        buf.extend_from_slice(b"event: message\ndata: {\"id\":1}\ndata: \n\n");
        assert_eq!(take_sse_event(&mut buf), Some("{\"id\":1}\n".into()));
        assert!(buf.is_empty());
    }

    #[test]
    fn sse_handles_crlf_and_prefix_space() {
        let mut buf = b"data:{\"a\":true}\r\n\r\n".to_vec();
        assert_eq!(take_sse_event(&mut buf), Some("{\"a\":true}".into()));
    }
}
