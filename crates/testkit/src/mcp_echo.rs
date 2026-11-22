//! A minimal MCP echo server for streamable-HTTP tests.
//!
//! One JSON-RPC object per `POST`. Responses are `application/json`, or
//! `text/event-stream` when the target asks for it (`…/mcp?format=sse` or
//! `…/mcp/sse`), so both client paths are exercised. Every request is captured
//! verbatim (request line, headers in wire order, body) for assertions and the
//! correctness corpus.
//!
//! Behaviour: `initialize` assigns a `Mcp-Session-Id` that later requests must
//! send; `tools/list` offers `add` and `greet`; `tools/call` echoes the
//! arguments in the text content and in `structuredContent`, `fail*` tools
//! answer `isError: true`, unknown tools a JSON-RPC error.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

/// One request exactly as received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub request_line: String,
    /// Header lines in wire order (name as sent, value trimmed).
    pub headers: Vec<(String, String)>,
    pub body: String,
}

impl Captured {
    pub fn header(&self, name: &str) -> Option<String> {
        let lname = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(n, _)| n.to_ascii_lowercase() == lname)
            .map(|(_, v)| v.clone())
    }

    pub fn method_of_body(&self) -> String {
        serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|v| v.get("method").and_then(|m| m.as_str()).map(str::to_string))
            .unwrap_or_default()
    }
}

pub struct McpEcho {
    addr: SocketAddr,
    captured: Arc<Mutex<Vec<Captured>>>,
    task: JoinHandle<()>,
}

impl McpEcho {
    /// The base URL (`http://addr`); endpoints are `/mcp` and `/mcp/sse`.
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn captured(&self) -> Vec<Captured> {
        match self.captured.lock() {
            Ok(v) => v.clone(),
            Err(_) => Vec::new(),
        }
    }
}

impl Drop for McpEcho {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn start() -> std::io::Result<McpEcho> {
    start_on("127.0.0.1:0").await
}

pub async fn start_on(addr: &str) -> std::io::Result<McpEcho> {
    let listener = TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let captured: Arc<Mutex<Vec<Captured>>> = Arc::default();
    let c2 = captured.clone();
    let task = tokio::spawn(async move {
        let next_session = AtomicU32::new(1);
        while let Ok((sock, _)) = listener.accept().await {
            let c = c2.clone();
            let s = next_session.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let _ = serve(sock, c, s).await;
            });
        }
    });
    Ok(McpEcho {
        addr,
        captured,
        task,
    })
}

async fn serve(
    mut sock: TcpStream,
    captured: Arc<Mutex<Vec<Captured>>>,
    session_no: u32,
) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = vec![0u8; 64 * 1024];
    loop {
        // Read the head.
        let head_end = loop {
            if let Some(i) = find(&buf, b"\r\n\r\n") {
                break i + 4;
            }
            let n = sock.read(&mut tmp).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.len() > 64 * 1024 {
                return Ok(());
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default().to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.to_string(), v.trim().to_string()))
            .collect();
        let len: usize = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        // Read the body.
        while buf.len() < head_end + len {
            let n = sock.read(&mut tmp).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let body = String::from_utf8_lossy(&buf[head_end..head_end + len]).into_owned();
        buf.drain(..head_end + len);

        if let Ok(mut c) = captured.lock() {
            c.push(Captured {
                request_line: request_line.clone(),
                headers: headers.clone(),
                body: body.clone(),
            });
        }

        if !request_line.starts_with("POST ") {
            respond(&mut sock, 405, "text/plain", b"only POST\n", "").await?;
            continue;
        }
        let target = request_line.split(' ').nth(1).unwrap_or_default();
        if !target.starts_with("/mcp") {
            respond(&mut sock, 404, "text/plain", b"unknown endpoint\n", "").await?;
            continue;
        }
        let msg: serde_json::Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_) => {
                let e = error_body(&serde_json::Value::Null, -32700, "parse error");
                respond(&mut sock, 200, "application/json", e.as_bytes(), "").await?;
                continue;
            }
        };
        let method = msg
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or_default()
            .to_string();
        let id = msg.get("id").cloned();
        let session_sent = headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case("mcp-session-id"))
            .map(|(_, v)| v.clone());
        if method != "initialize" && session_sent.is_none() {
            respond(
                &mut sock,
                400,
                "text/plain",
                b"missing Mcp-Session-Id\n",
                "",
            )
            .await?;
            continue;
        }
        let Some(id) = id else {
            // A notification: accepted, no body.
            respond(&mut sock, 202, "text/plain", b"", "").await?;
            continue;
        };
        let (status, out, is_initialize) =
            answer(&method, &msg, &id, target.contains("paginate=1"));
        let extra = session_id_header(is_initialize, session_no);
        let sse = target.contains("sse") || target.contains("format=sse");
        if sse {
            let framed = format!("data: {out}\n\n");
            respond_sse(&mut sock, status, framed.as_bytes(), extra.as_str()).await?;
        } else {
            respond(
                &mut sock,
                status,
                "application/json",
                out.as_bytes(),
                extra.as_str(),
            )
            .await?;
        }
    }
}

fn answer(
    method: &str,
    msg: &serde_json::Value,
    id: &serde_json::Value,
    paginate: bool,
) -> (u16, String, bool) {
    let result = match method {
        "initialize" => serde_json::json!({
            "protocolVersion": msg
                .pointer("/params/protocolVersion")
                .and_then(|v| v.as_str())
                .unwrap_or("2025-06-18"),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "mcp-echo", "version": "0.1.0" }
        }),
        "tools/list" if paginate && msg.pointer("/params/cursor").is_none() => serde_json::json!({
            "tools": [{"name": "add", "inputSchema": {"type": "object"}}],
            "nextCursor": "page-two"
        }),
        "tools/list" if paginate => serde_json::json!({
            "tools": [{"name": "greet", "inputSchema": {"type": "object"}}]
        }),
        "tools/list" => serde_json::json!({
            "tools": [
                { "name": "add", "description": "Adds two numbers",
                  "inputSchema": {
                      "type": "object",
                      "properties": { "a": { "type": "number" }, "b": { "type": "number" } }
                  } },
                { "name": "greet", "description": "Greets someone",
                  "inputSchema": {
                      "type": "object",
                      "properties": { "name": { "type": "string" } }
                  } }
            ]
        }),
        "tools/call" => {
            let name = msg
                .pointer("/params/name")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if name.starts_with("fail") {
                serde_json::json!({
                    "content": [ { "type": "text", "text": "boom" } ],
                    "isError": true
                })
            } else if name == "add" || name == "greet" {
                let args = msg.pointer("/params/arguments").cloned();
                let plain = args.clone().unwrap_or(serde_json::Value::Null);
                serde_json::json!({
                    "content": [ {
                        "type": "text",
                        "text": format!("called {name} {plain}")
                    } ],
                    "structuredContent": plain
                })
            } else {
                let out = error_body(id, -32602, "unknown tool");
                return (200, out, false);
            }
        }
        _ => {
            let out = error_body(id, -32601, &format!("no {method}"));
            return (200, out, false);
        }
    };
    let out = serde_json::json!({
        "jsonrpc": "2.0", "id": id, "result": result
    });
    (200, out.to_string(), method == "initialize")
}

/// The `Mcp-Session-Id` response header for initialize answers.
fn session_id_header(is_initialize: bool, session_no: u32) -> String {
    if is_initialize {
        format!("Mcp-Session-Id: sess-{session_no}\r\n")
    } else {
        String::new()
    }
}

fn error_body(id: &serde_json::Value, code: i64, message: &str) -> String {
    serde_json::json!({
        "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message }
    })
    .to_string()
}

async fn respond(
    sock: &mut TcpStream,
    status: u16,
    ctype: &str,
    body: &[u8],
    extra_headers: &str,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\n{}Content-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        status,
        reason(status),
        ctype,
        extra_headers,
        body.len()
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body).await
}

async fn respond_sse(
    sock: &mut TcpStream,
    status: u16,
    body: &[u8],
    extra_headers: &str,
) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: text/event-stream\r\n{}Content-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        status,
        reason(status),
        extra_headers,
        body.len()
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body).await
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        202 => "Accepted",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        _ => "Error",
    }
}

fn find(h: &[u8], pat: &[u8]) -> Option<usize> {
    h.windows(pat.len()).position(|w| w == pat)
}
