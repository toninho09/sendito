//! Minimal MCP (Model Context Protocol) client: JSON-RPC 2.0 over stdio or
//! streamable HTTP, speaking only what the protocol needs to call tools.
//!
//! Every entry point opens a session (spawn/connect, `initialize`,
//! `notifications/initialized`), does its work and drops the connection; the
//! whole call runs under one timeout. Each message sent and received is
//! reported as an [`Event`] so the caller can show the conversation.

pub mod http;
pub mod oauth;
pub mod protocol;
pub mod stdio;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use serde_json::{Value, json};
use tokio::sync::mpsc::UnboundedSender;

pub use oauth::RefreshingProvider;

pub use protocol::PROTOCOL_VERSION;

/// Supplies the `Authorization: Bearer` token for HTTP servers. Called before
/// every POST, and again (with `force_refresh`) after a `401`.
pub trait TokenProvider: Send + Sync {
    fn token<'a>(
        &'a self,
        force_refresh: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>>;
}

/// A fixed token (a static bearer from the config, or a test double).
#[derive(Clone)]
pub struct StaticToken(pub String);

impl TokenProvider for StaticToken {
    fn token<'a>(
        &'a self,
        _force_refresh: bool,
    ) -> Pin<Box<dyn Future<Output = Result<String, McpError>> + Send + 'a>> {
        Box::pin(std::future::ready(Ok(self.0.clone())))
    }
}

/// Everything that can go wrong while talking to an MCP server.
#[derive(Debug, thiserror::Error)]
pub enum McpError {
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("cannot start {command:?}: {msg}")]
    Spawn { command: String, msg: String },
    #[error("transport: {0}")]
    Transport(String),
    #[error("the server closed the connection")]
    Closed,
    #[error("invalid JSON from the server: {0}")]
    Json(String),
    #[error("the arguments must be a JSON object: {0}")]
    Arguments(String),
    #[error("JSON-RPC error {code}: {message}")]
    Rpc { code: i64, message: String },
    #[error("{0}")]
    Message(String),
}

/// Where and how to reach a server. Built from the collection config.
#[derive(Clone)]
pub enum Transport {
    /// A child process speaking NDJSON on stdio.
    Stdio {
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    /// A streamable HTTP endpoint.
    Http {
        url: String,
        headers: Vec<(String, String)>,
        verify_tls: bool,
        /// `[http] proxy` from the config; `None` connects directly.
        proxy: Option<sendito_http::ProxySpec>,
        /// `[http] tls` from the config (custom CA, client certificate).
        tls: sendito_http::TlsMaterial,
        /// OAuth (or a fixed token): adds `Authorization` and retries once
        /// after a `401`.
        auth: Option<Arc<dyn TokenProvider>>,
    },
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Transport::Stdio { command, args, env } => f
                .debug_struct("Transport::Stdio")
                .field("command", command)
                .field("args", args)
                .field("env", env)
                .finish(),
            Transport::Http {
                url,
                headers,
                verify_tls,
                proxy,
                tls,
                auth,
            } => f
                .debug_struct("Transport::Http")
                .field("url", url)
                .field("headers", headers)
                .field("verify_tls", verify_tls)
                .field("proxy", proxy)
                .field("tls", tls)
                .field("auth", &auth.is_some())
                .finish(),
        }
    }
}

/// Call settings; the config provides the defaults.
#[derive(Debug, Clone)]
pub struct Options {
    /// Timeout for the whole call, handshake included.
    pub timeout: Duration,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
        }
    }
}

/// Something that happened during a call, in order. Mirrors the gRPC events so
/// the UI renders both the same way.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// A JSON-RPC message was written to the transport (compact, one line).
    Sent {
        json: String,
        at: Duration,
        wall: SystemTime,
    },
    /// A JSON-RPC message arrived (compact, one line).
    Received {
        json: String,
        at: Duration,
        wall: SystemTime,
    },
    /// A stderr line from a stdio server, or a session-level note.
    Log { line: String, at: Duration },
    /// The call ended: `ok` with an empty message, or the error text.
    Finished {
        ok: bool,
        message: String,
        at: Duration,
    },
}

/// A tool from `tools/list`.
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub name: String,
    pub description: Option<String>,
    /// The tool's `inputSchema` as received (a JSON Schema object).
    pub input_schema: Value,
}

/// The outcome of `tools/call`. `is_error` is the protocol flag: the call
/// itself succeeded, the tool reported failure.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolResult {
    pub is_error: bool,
    /// The `content` array as received (text, image, resource items).
    pub content: Value,
    /// The tool's structured output, when it sends one.
    pub structured: Option<Value>,
    /// The full result object (what scripts and assertions read).
    pub raw: Value,
}

impl ToolResult {
    /// Every text item's text joined with `\n`.
    pub fn text(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(items) = self.content.as_array() {
            for item in items {
                if item.get("type").and_then(Value::as_str) == Some("text")
                    && let Some(t) = item.get("text").and_then(Value::as_str)
                {
                    parts.push(t.to_string());
                }
            }
        }
        parts.join("\n")
    }
}

/// One connection to a server. The trait is crate-private; the two transports
/// implement it and tests build in-process fakes.
pub(crate) trait Wire: Send {
    /// Record the negotiated version for subsequent transport requests.
    fn set_protocol_version(&mut self, _version: &str) {}

    fn send<'a>(
        &'a mut self,
        line: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>>;
    fn recv<'a>(&'a mut self)
    -> Pin<Box<dyn Future<Output = Result<Value, McpError>> + Send + 'a>>;
}

struct Session {
    wire: Box<dyn Wire>,
    events: UnboundedSender<Event>,
    start: Instant,
    next_id: u64,
}

impl Session {
    #[cfg(test)]
    fn mock(wire: Box<dyn Wire>, events: UnboundedSender<Event>) -> Self {
        Self {
            wire,
            events,
            start: Instant::now(),
            next_id: 1,
        }
    }
    async fn open(
        transport: &Transport,
        events: UnboundedSender<Event>,
        start: Instant,
    ) -> Result<Self, McpError> {
        let wire = match transport {
            Transport::Stdio { command, args, env } => {
                stdio::spawn(command, args, env, events.clone(), start)?
            }
            Transport::Http {
                url,
                headers,
                verify_tls,
                proxy,
                tls,
                auth,
            } => http::connect(
                url,
                headers,
                *verify_tls,
                proxy.clone(),
                tls.clone(),
                auth.clone(),
            )?,
        };
        let mut s = Self {
            wire,
            events,
            start,
            next_id: 1,
        };
        s.initialize().await?;
        Ok(s)
    }

    async fn initialize(&mut self) -> Result<(), McpError> {
        let id = self.take_id();
        let result = self
            .exchange(id, "initialize", protocol::initialize_params())
            .await?;
        let version = result
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if !protocol::SUPPORTED_VERSIONS.contains(&version) {
            self.note(format!(
                "the server wants protocol version {version:?}; continuing"
            ));
        }
        self.wire.set_protocol_version(version);
        let note = protocol::notification("notifications/initialized");
        let text = compact(&note)?;
        self.emit(Event::Sent {
            json: text.clone(),
            at: self.start.elapsed(),
            wall: SystemTime::now(),
        });
        self.wire.send(&text).await
    }

    /// Sends a request and reads messages until the response with its `id`
    /// arrives; notifications in between are reported and skipped.
    async fn exchange(&mut self, id: u64, method: &str, params: Value) -> Result<Value, McpError> {
        let text = compact(&protocol::request(id, method, params))?;
        self.emit(Event::Sent {
            json: text.clone(),
            at: self.start.elapsed(),
            wall: SystemTime::now(),
        });
        self.wire.send(&text).await?;
        loop {
            let v = self.wire.recv().await?;
            self.emit(Event::Received {
                json: compact(&v)?,
                at: self.start.elapsed(),
                wall: SystemTime::now(),
            });
            if protocol::is_response_to(&v, id) {
                if let Some(err) = v.get("error") {
                    return Err(McpError::Rpc {
                        code: err.get("code").and_then(Value::as_i64).unwrap_or(-32603),
                        message: err
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("unknown error")
                            .to_string(),
                    });
                }
                return Ok(v.get("result").cloned().unwrap_or(Value::Null));
            }
        }
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn list(&mut self) -> Result<Vec<Tool>, McpError> {
        let mut tools: Vec<Tool> = Vec::new();
        let mut cursor: Option<String> = None;
        // Pagination safety cap: a server that always answers a cursor stops
        // the call instead of looping forever.
        for _ in 0..100 {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let id = self.take_id();
            let result = self.exchange(id, "tools/list", params).await?;
            if let Some(list) = result.get("tools").and_then(Value::as_array) {
                for t in list {
                    tools.push(Tool {
                        name: t
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        description: t
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        input_schema: t.get("inputSchema").cloned().unwrap_or(json!({})),
                    });
                }
            }
            cursor = result
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_string);
            if cursor.is_none() {
                return Ok(tools);
            }
        }
        Err(McpError::Message(
            "the server keeps sending list cursors".into(),
        ))
    }

    async fn call(&mut self, tool: &str, arguments: Value) -> Result<ToolResult, McpError> {
        let params = if arguments.is_null() {
            json!({ "name": tool })
        } else {
            json!({ "name": tool, "arguments": arguments })
        };
        let id = self.take_id();
        let result = self.exchange(id, "tools/call", params).await?;
        Ok(ToolResult {
            is_error: result
                .get("isError")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            content: result.get("content").cloned().unwrap_or(json!([])),
            structured: result.get("structuredContent").cloned(),
            raw: result,
        })
    }

    fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    fn note(&self, line: String) {
        self.emit(Event::Log {
            line,
            at: self.start.elapsed(),
        });
    }
}

fn compact(v: &Value) -> Result<String, McpError> {
    serde_json::to_string(v).map_err(|e| McpError::Json(e.to_string()))
}

/// Connects, handshakes and lists the server's tools.
pub async fn list_tools(
    transport: &Transport,
    opts: &Options,
    events: UnboundedSender<Event>,
) -> Result<Vec<Tool>, McpError> {
    run(
        transport,
        opts,
        events,
        |mut s| async move { s.list().await },
    )
    .await
}

/// Connects, handshakes and calls one tool. `arguments_json` is the request
/// file's JSON text; an empty string sends no `arguments` field.
pub async fn call_tool(
    transport: &Transport,
    tool: &str,
    arguments_json: &str,
    opts: &Options,
    events: UnboundedSender<Event>,
) -> Result<ToolResult, McpError> {
    let arguments = parse_arguments(arguments_json)?;
    run(transport, opts, events, |mut s| async move {
        s.call(tool, arguments).await
    })
    .await
}

fn parse_arguments(text: &str) -> Result<Value, McpError> {
    let t = text.trim();
    if t.is_empty() {
        return Ok(Value::Null);
    }
    let v = serde_json::from_str::<Value>(t).map_err(|e| McpError::Arguments(e.to_string()))?;
    if !v.is_object() {
        return Err(McpError::Arguments("it must be a JSON object".into()));
    }
    Ok(v)
}

/// Runs one session under the whole-call timeout and closes it with a
/// `Finished` event, success or failure.
async fn run<T, F, Fut>(
    transport: &Transport,
    opts: &Options,
    events: UnboundedSender<Event>,
    f: F,
) -> Result<T, McpError>
where
    F: FnOnce(Session) -> Fut,
    Fut: Future<Output = Result<T, McpError>>,
{
    let start = Instant::now();
    let work = async {
        let s = Session::open(transport, events.clone(), start).await?;
        f(s).await
    };
    let outcome = tokio::time::timeout(opts.timeout, work).await;
    match outcome {
        Ok(Ok(v)) => {
            let _ = events.send(Event::Finished {
                ok: true,
                message: String::new(),
                at: start.elapsed(),
            });
            Ok(v)
        }
        Ok(Err(e)) => {
            let _ = events.send(Event::Finished {
                ok: false,
                message: e.to_string(),
                at: start.elapsed(),
            });
            Err(e)
        }
        Err(_) => {
            let _ = events.send(Event::Finished {
                ok: false,
                message: format!("timeout after {:?}", opts.timeout),
                at: start.elapsed(),
            });
            Err(McpError::Timeout(opts.timeout))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::mpsc;

    #[test]
    fn arguments_must_be_an_object() {
        assert_eq!(parse_arguments("").ok(), Some(Value::Null));
        assert!(parse_arguments("{\"a\":1}").is_ok());
        assert!(parse_arguments("[1]").is_err());
        assert!(parse_arguments("nope").is_err());
    }

    #[test]
    fn text_joins_only_text_items() {
        let r = ToolResult {
            is_error: false,
            content: json!([
                { "type": "text", "text": "a" },
                { "type": "image", "data": "AAAA", "mimeType": "image/png" },
                { "type": "text", "text": "b" }
            ]),
            structured: None,
            raw: json!({}),
        };
        assert_eq!(r.text(), "a\nb");
    }

    /// The client half of a duplex pair, speaking NDJSON like a stdio server.
    struct MockWire {
        reader: BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>,
        writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    }

    impl Wire for MockWire {
        fn send<'a>(
            &'a mut self,
            line: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
            Box::pin(async move {
                self.writer
                    .write_all(line.as_bytes())
                    .await
                    .map_err(|e| McpError::Transport(e.to_string()))?;
                self.writer
                    .write_all(b"\n")
                    .await
                    .map_err(|e| McpError::Transport(e.to_string()))?;
                self.writer
                    .flush()
                    .await
                    .map_err(|e| McpError::Transport(e.to_string()))
            })
        }

        fn recv<'a>(
            &'a mut self,
        ) -> Pin<Box<dyn Future<Output = Result<Value, McpError>> + Send + 'a>> {
            Box::pin(async move {
                let mut line = String::new();
                let n = self
                    .reader
                    .read_line(&mut line)
                    .await
                    .map_err(|e| McpError::Transport(e.to_string()))?;
                if n == 0 {
                    return Err(McpError::Closed);
                }
                serde_json::from_str(line.trim()).map_err(|e| McpError::Json(e.to_string()))
            })
        }
    }

    #[derive(Clone, Default)]
    struct MockOpts {
        /// Answer `tools/list` with this many extra pages before the last.
        pages: usize,
        /// Push a notification before every response.
        notify_first: bool,
        /// Answer `initialize` with this protocol version.
        version: Option<&'static str>,
    }

    async fn write_line(
        w: &mut tokio::io::WriteHalf<tokio::io::DuplexStream>,
        v: &Value,
    ) -> std::io::Result<()> {
        let mut s = serde_json::to_string(v).unwrap_or_default();
        s.push('\n');
        w.write_all(s.as_bytes()).await?;
        w.flush().await
    }

    fn response(id: &Value, result: Value) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "result": result })
    }

    fn error_response(id: &Value, code: i64, message: &str) -> Value {
        json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
    }

    /// Spawns a fake MCP server on the other end of a duplex pair and returns
    /// the wire plus the exact lines the server received. Tools: `add` and
    /// `greet` echo their arguments, `fail` answers `isError`, anything else
    /// is an unknown tool.
    fn mock_server(opts: MockOpts) -> (Box<dyn Wire>, Arc<Mutex<Vec<String>>>) {
        let (client, server) = tokio::io::duplex(4096);
        let (r, w) = tokio::io::split(client);
        let wire: Box<dyn Wire> = Box::new(MockWire {
            reader: BufReader::new(r),
            writer: w,
        });
        let seen: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            let (sr, mut sw) = tokio::io::split(server);
            let mut lines = BufReader::new(sr).lines();
            let mut page = 0;
            while let Ok(Some(line)) = lines.next_line().await {
                if let Ok(mut v) = seen2.lock() {
                    v.push(line.clone());
                }
                let Ok(v) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let Some(id) = v.get("id").cloned() else {
                    continue; // a notification: nothing to answer
                };
                let method = v.get("method").and_then(Value::as_str).unwrap_or_default();
                let msg = match method {
                    "initialize" => response(
                        &id,
                        json!({
                            "protocolVersion":
                                opts.version.unwrap_or(protocol::PROTOCOL_VERSION),
                            "capabilities": { "tools": {} },
                            "serverInfo": { "name": "mock", "version": "1" }
                        }),
                    ),
                    "tools/list" => {
                        if page < opts.pages {
                            page += 1;
                            response(
                                &id,
                                json!({
                                    "tools": [ {
                                        "name": format!("p{page}"),
                                        "inputSchema": {}
                                    } ],
                                    "nextCursor": format!("c{page}")
                                }),
                            )
                        } else {
                            response(
                                &id,
                                json!({
                                    "tools": [
                                        { "name": "add", "description": "Adds numbers",
                                          "inputSchema": { "type": "object" } },
                                        { "name": "greet", "inputSchema": { "type": "object" } }
                                    ]
                                }),
                            )
                        }
                    }
                    "tools/call" => {
                        let name = v
                            .pointer("/params/name")
                            .and_then(Value::as_str)
                            .unwrap_or_default();
                        let args = v
                            .pointer("/params/arguments")
                            .cloned()
                            .unwrap_or(Value::Null);
                        match name {
                            "fail" => response(
                                &id,
                                json!({
                                    "content": [ { "type": "text", "text": "boom" } ],
                                    "isError": true
                                }),
                            ),
                            "add" | "greet" => response(
                                &id,
                                json!({
                                    "content": [ {
                                        "type": "text",
                                        "text": format!("called {name} {args}")
                                    } ],
                                    "structuredContent": args
                                }),
                            ),
                            _ => error_response(&id, -32602, "unknown tool"),
                        }
                    }
                    _ => error_response(&id, -32601, &format!("no {method}")),
                };
                if opts.notify_first {
                    let note = json!({
                        "jsonrpc": "2.0", "method": "notifications/message",
                        "params": { "level": "info" }
                    });
                    if write_line(&mut sw, &note).await.is_err() {
                        break;
                    }
                }
                if write_line(&mut sw, &msg).await.is_err() {
                    break;
                }
            }
        });
        (wire, seen)
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
        let mut out = Vec::new();
        while let Ok(e) = rx.try_recv() {
            out.push(e);
        }
        out
    }

    fn lines_of(seen: &Arc<Mutex<Vec<String>>>) -> Vec<String> {
        match seen.lock() {
            Ok(v) => v.clone(),
            Err(_) => Vec::new(),
        }
    }

    #[tokio::test]
    async fn session_writes_the_protocol_lines() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (wire, seen) = mock_server(MockOpts::default());
        let mut s = Session::mock(wire, tx);
        s.initialize().await.unwrap_or_else(|e| panic!("{e}"));
        let r = s
            .call("add", json!({"a":1}))
            .await
            .unwrap_or_else(|e| panic!("{e}"));

        let lines = lines_of(&seen);
        let init = format!(
            "{{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":\
             {{\"protocolVersion\":\"{}\",\"capabilities\":{{}},\
             \"clientInfo\":{{\"name\":\"sendito\",\"version\":\"{}\"}}}}}}",
            PROTOCOL_VERSION,
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], init);
        assert_eq!(
            lines[1],
            "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\"}"
        );
        assert_eq!(
            lines[2],
            "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/call\",\
             \"params\":{\"name\":\"add\",\"arguments\":{\"a\":1}}}"
        );
        assert_eq!(r.text(), "called add {\"a\":1}");
        assert_eq!(r.structured, Some(json!({"a":1})));

        let kinds: Vec<bool> = drain(&mut rx)
            .into_iter()
            .map(|e| matches!(e, Event::Received { .. }))
            .collect();
        assert!(kinds.iter().any(|r| *r));
    }

    #[tokio::test]
    async fn json_rpc_error_maps_to_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (wire, _seen) = mock_server(MockOpts::default());
        let mut s = Session::mock(wire, tx);
        s.initialize().await.unwrap_or_else(|e| panic!("{e}"));
        match s.call("nope", Value::Null).await {
            Err(McpError::Rpc { code, message }) => {
                assert_eq!(code, -32602);
                assert_eq!(message, "unknown tool");
            }
            other => panic!("expected an Rpc error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn tool_error_is_a_result_not_an_error() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (wire, _seen) = mock_server(MockOpts::default());
        let mut s = Session::mock(wire, tx);
        s.initialize().await.unwrap_or_else(|e| panic!("{e}"));
        let r = s
            .call("fail", Value::Null)
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(r.is_error);
        assert_eq!(r.text(), "boom");
    }

    #[tokio::test]
    async fn notifications_are_reported_and_skipped() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (wire, _seen) = mock_server(MockOpts {
            notify_first: true,
            ..MockOpts::default()
        });
        let mut s = Session::mock(wire, tx);
        s.initialize().await.unwrap_or_else(|e| panic!("{e}"));
        let tools = s.list().await.unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "add");
        assert_eq!(tools[0].input_schema, json!({ "type": "object" }));
        let events = drain(&mut rx);
        let notes = events
            .iter()
            .filter(|e| {
                matches!(e, Event::Received { json, .. }
                if json.contains("notifications/message"))
            })
            .count();
        assert!(notes >= 1);
    }

    #[tokio::test]
    async fn list_follows_cursors() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let (wire, seen) = mock_server(MockOpts {
            pages: 2,
            ..MockOpts::default()
        });
        let mut s = Session::mock(wire, tx);
        s.initialize().await.unwrap_or_else(|e| panic!("{e}"));
        let tools = s.list().await.unwrap_or_else(|e| panic!("{e}"));
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["p1", "p2", "add", "greet"]);
        let lines = lines_of(&seen);
        assert_eq!(lines.len(), 5);
        assert!(lines[3].contains("\"cursor\":\"c1\""));
        assert!(lines[4].contains("\"cursor\":\"c2\""));
    }

    #[tokio::test]
    async fn unknown_protocol_version_only_warns() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let (wire, _seen) = mock_server(MockOpts {
            version: Some("1999-01-01"),
            ..MockOpts::default()
        });
        let mut s = Session::mock(wire, tx);
        s.initialize().await.unwrap_or_else(|e| panic!("{e}"));
        let warnings = drain(&mut rx)
            .into_iter()
            .filter_map(|e| match e {
                Event::Log { line, .. } => Some(line),
                _ => None,
            })
            .filter(|l| l.contains("1999-01-01"))
            .count();
        assert_eq!(warnings, 1);
    }
}
