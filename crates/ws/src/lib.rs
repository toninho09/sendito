//! Dynamic WebSocket client built on tokio-tungstenite.
//!
//! One session per request: the handshake opens, frames flow through a
//! command/event pair of channels (like the gRPC call), and the session ends
//! with a close handshake, a deadline or a cancel. TLS uses the same
//! [`sendito_http::tls_config`] as HTTP and gRPC; an HTTP(S) proxy is
//! tunnelled with CONNECT (SOCKS is not supported, see `DEBT.md`).

use std::future::Future;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use rustls::pki_types::ServerName;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::{CloseFrame, Utf8Bytes};

mod proxy;
mod session;
pub use proxy::*;
pub use session::*;

/// The kind of one frame on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameKind {
    Text,
    Binary,
    Ping,
    Pong,
    Close,
}

impl FrameKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            FrameKind::Text => "text",
            FrameKind::Binary => "binary",
            FrameKind::Ping => "ping",
            FrameKind::Pong => "pong",
            FrameKind::Close => "close",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            FrameKind::Text => "Text",
            FrameKind::Binary => "Binary",
            FrameKind::Ping => "Ping",
            FrameKind::Pong => "Pong",
            FrameKind::Close => "Close",
        }
    }

    /// Whether frames of this kind carry a payload shown to the user.
    pub const fn has_payload(self) -> bool {
        matches!(self, FrameKind::Text | FrameKind::Binary)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum WsError {
    #[error("invalid WebSocket URL {0:?}: use ws://host/path or wss://host/path")]
    Url(String),
    #[error("cannot connect to {target}: {msg}")]
    Connect { target: String, msg: String },
    #[error("{0}")]
    Proxy(String),
    #[error("{0}")]
    Send(String),
}

/// Connection settings for one session.
#[derive(Debug, Clone)]
pub struct ConnSettings {
    /// `ws://host:port/path` or `wss://host:port/path`.
    pub url: String,
    /// Extra headers sent with the handshake, in order (user headers only;
    /// the library adds the upgrade headers).
    pub headers: Vec<(String, String)>,
    /// Values for `Sec-WebSocket-Protocol`, in order.
    pub subprotocols: Vec<String>,
    pub verify_tls: bool,
    /// Timeout for TCP connect + TLS + the handshake (`None` = no limit).
    pub handshake_timeout: Option<Duration>,
    /// Whole-session deadline, counted from the connection start
    /// (`None` = unlimited; mirrors the gRPC deadline).
    pub deadline: Option<Duration>,
    /// How long the peer gets to answer our close frame before the session
    /// ends as dropped (1006). [`CLOSE_TIMEOUT`] unless overridden.
    pub close_timeout: Duration,
    /// `[http] tls` from the config (custom CA, client certificate).
    pub tls: sendito_http::TlsMaterial,
    pub proxy: Option<WsProxy>,
}

/// Something that happened during a session.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// The handshake answer arrived (101 on success).
    Handshake {
        status: u16,
        headers: Vec<(String, String)>,
        /// The server's choice from the offered `Sec-WebSocket-Protocol`.
        subprotocol: Option<String>,
        at: Duration,
        wall: SystemTime,
    },
    /// A frame was handed to the transport.
    Sent {
        kind: FrameKind,
        /// UTF-8 text for text frames, base64 for binary frames.
        preview: String,
        size: usize,
        at: Duration,
        wall: SystemTime,
    },
    /// A frame arrived from the server.
    Received {
        kind: FrameKind,
        preview: String,
        size: usize,
        at: Duration,
        wall: SystemTime,
    },
    /// The session ended: `code` is the close code the session ended with
    /// (1006 when the connection dropped without a close handshake).
    Finished {
        code: u16,
        reason: String,
        at: Duration,
    },
}

/// One message parsed from the request's message document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    Text(String),
    Binary(Vec<u8>),
}

/// Parses the message document: an array of strings (text frames) and
/// `{ "binary": "base64" }` objects (binary frames), or one such value
/// alone. An empty array sends nothing. Variables are rendered by the caller
/// before this runs.
pub fn parse_messages(text: &str) -> Result<Vec<Frame>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(trimmed).map_err(|e| format!("messages: {e}"))?;
    let one = |v: &serde_json::Value| -> Result<Frame, String> {
        match v {
            serde_json::Value::String(s) => Ok(Frame::Text(s.clone())),
            serde_json::Value::Object(o) if o.len() == 1 => {
                let Some((k, val)) = o.iter().next() else {
                    return Err("messages: empty object".into());
                };
                if k == "binary" {
                    let text = val
                        .as_str()
                        .ok_or("messages: \"binary\" must be a base64 string")?;
                    use base64::Engine as _;
                    let bytes = base64::engine::general_purpose::STANDARD
                        .decode(text.trim())
                        .map_err(|e| format!("messages: binary must be base64: {e}"))?;
                    Ok(Frame::Binary(bytes))
                } else if k == "text" {
                    let s = val
                        .as_str()
                        .ok_or("messages: \"text\" must be a string")?
                        .to_string();
                    Ok(Frame::Text(s))
                } else {
                    Err(format!(
                        "messages: object keys must be \"text\" or \"binary\", got {k:?}"
                    ))
                }
            }
            other => Err(format!(
                "messages: expected a string or {{\"binary\": …}}, got {other}"
            )),
        }
    };
    match &value {
        serde_json::Value::Array(items) => items.iter().map(one).collect(),
        v => Ok(vec![one(v)?]),
    }
}

/// Opens a session: TCP (+ proxy CONNECT), TLS for `wss://`, then the
/// handshake. The [`Event::Handshake`] is queued first.
pub async fn open(s: &ConnSettings) -> Result<Session, WsError> {
    let start = Instant::now();
    let fail = |msg: String| WsError::Connect {
        target: s.url.clone(),
        msg,
    };
    let uri: http::Uri = s.url.parse().map_err(|_| WsError::Url(s.url.clone()))?;
    let secure = match uri.scheme_str() {
        Some("ws") => false,
        Some("wss") => true,
        _ => return Err(WsError::Url(s.url.clone())),
    };
    let host = uri
        .host()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| WsError::Url(s.url.clone()))?
        .to_string();
    let port = uri.port_u16().unwrap_or(if secure { 443 } else { 80 });
    // `[::1]` keeps its brackets in the CONNECT target; the address lookup
    // and the TLS server name need the bare address.
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(&host)
        .to_string();

    let limit = s.handshake_timeout;
    let raw: Transport = match &s.proxy {
        None => connect_with_timeout((bare.as_str(), port), limit)
            .await
            .map_err(|e| fail(e.to_string()))
            .map(|tcp| Box::new(tcp) as Transport)?,
        Some(p) => through_proxy(p, &host, port, limit, &s.tls)
            .await
            .map_err(WsError::Proxy)?,
    };

    let stream = if secure {
        let cfg = sendito_http::tls_config(s.verify_tls, &[], &s.tls).map_err(fail)?;
        let name = ServerName::try_from(bare.clone())
            .map_err(|e| WsError::Url(format!("{host}: {e}")))?
            .to_owned();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
        let tls = limited(limit, connector.connect(name, raw))
            .await
            .ok_or_else(|| fail("TLS handshake timed out".into()))?
            .map_err(|e| fail(format!("TLS: {e}")))?;
        Box::new(tls) as Transport
    } else {
        raw
    };

    let mut request = uri
        .clone()
        .into_client_request()
        .map_err(|e| fail(e.to_string()))?;
    {
        let headers = request.headers_mut();
        for (n, v) in &s.headers {
            let name = http::HeaderName::from_bytes(n.as_bytes())
                .map_err(|_| WsError::Url(format!("invalid header name {n:?}")))?;
            let value = http::HeaderValue::from_str(v)
                .map_err(|_| WsError::Url(format!("invalid header value for {n:?}")))?;
            headers.append(name, value);
        }
        if !s.subprotocols.is_empty() {
            let value = s
                .subprotocols
                .iter()
                .map(|p| p.trim())
                .filter(|p| !p.is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            if !value.is_empty() {
                let value = http::HeaderValue::from_str(&value)
                    .map_err(|_| WsError::Url("invalid subprotocol value".into()))?;
                headers.append("Sec-WebSocket-Protocol", value);
            }
        }
    }

    let (ws, resp) = limited(limit, tokio_tungstenite::client_async(request, stream))
        .await
        .ok_or_else(|| fail("handshake timed out".into()))?
        .map_err(|e| fail(error_chain(&e)))?;

    let subprotocol = resp
        .headers()
        .get("Sec-WebSocket-Protocol")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let (etx, erx) = mpsc::unbounded_channel();
    let _ = etx.send(Event::Handshake {
        status: resp.status().as_u16(),
        headers: resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_string(),
                    v.to_str().map(str::to_string).unwrap_or_default(),
                )
            })
            .collect(),
        subprotocol,
        at: start.elapsed(),
        wall: SystemTime::now(),
    });

    let (ctx, crx) = mpsc::unbounded_channel::<Cmd>();
    let task = tokio::spawn(run(
        ws,
        crx,
        etx.clone(),
        start,
        s.deadline,
        s.close_timeout,
    ));
    Ok(Session {
        events: erx,
        event_tx: etx,
        cmd: Some(ctx),
        task,
        start,
    })
}

/// The stream under the WebSocket: a TCP stream or any number of TLS layers
/// over one. A boxed trait object keeps the type flat, so an `https://` proxy
/// can put its own TLS between the TCP stream and the session's TLS.
trait Io: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> Io for T {}

type Transport = Box<dyn Io>;

/// Runs `fut` under `limit`; `None` waits forever and the outer `None` means
/// the limit expired.
async fn limited<T>(limit: Option<Duration>, fut: impl Future<Output = T>) -> Option<T> {
    match limit {
        Some(d) => tokio::time::timeout(d, fut).await.ok(),
        None => Some(fut.await),
    }
}

/// TCP connect with an optional total timeout (DNS included).
async fn connect_with_timeout(
    target: (&str, u16),
    timeout: Option<Duration>,
) -> Result<TcpStream, WsError> {
    let (host, port) = target;
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| WsError::Connect {
            target: format!("{host}:{port}"),
            msg: format!("name resolution: {e}"),
        })?
        .collect();
    if addrs.is_empty() {
        return Err(WsError::Connect {
            target: format!("{host}:{port}"),
            msg: "name resolved to no addresses".into(),
        });
    }
    let mut last = "no address worked".to_string();
    for addr in addrs {
        match limited(timeout, TcpStream::connect(addr)).await {
            Some(Ok(s)) => {
                s.set_nodelay(true).ok();
                return Ok(s);
            }
            Some(Err(e)) => last = e.to_string(),
            None => {
                last = match timeout {
                    Some(d) => format!("connect timed out after {d:?}"),
                    None => "connect timed out".into(),
                }
            }
        }
    }
    Err(WsError::Connect {
        target: format!("{host}:{port}"),
        msg: last,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(url: &str) -> ConnSettings {
        ConnSettings {
            url: url.into(),
            headers: vec![],
            subprotocols: vec![],
            handshake_timeout: Some(Duration::from_secs(5)),
            deadline: None,
            close_timeout: CLOSE_TIMEOUT,
            verify_tls: true,
            tls: Default::default(),
            proxy: None,
        }
    }

    #[tokio::test]
    async fn echo_roundtrip() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        let mut s = open(&settings(&echo.url("/echo"))).await.unwrap();
        s.send_text("hello".into()).unwrap();
        let mut got_handshake = false;
        while let Some(ev) = s.events.recv().await {
            match ev {
                Event::Handshake { status, .. } => {
                    got_handshake = true;
                    assert_eq!(status, 101);
                }
                Event::Received { kind, preview, .. } => {
                    assert_eq!(kind, FrameKind::Text);
                    assert_eq!(preview, "hello");
                    break;
                }
                Event::Finished { .. } => panic!("session ended early"),
                _ => {}
            }
        }
        assert!(got_handshake);
        s.close();
        let last = loop {
            match s.events.recv().await {
                Some(Event::Finished { code, reason, .. }) => break (code, reason),
                Some(_) => {}
                None => panic!("session ended without Finished"),
            }
        };
        assert_eq!(last, (1000, String::new()), "close reason in the message");
    }

    #[tokio::test]
    async fn binary_and_ping() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        let mut s = open(&settings(&echo.url("/echo"))).await.unwrap();
        s.send_binary(vec![0xff, 0x00, 0xab]).unwrap();
        s.ping("probe".as_bytes().to_vec()).unwrap();
        let mut saw_binary = false;
        let mut saw_pong = false;
        while let Some(ev) = s.events.recv().await {
            match ev {
                Event::Received { kind, preview, .. } => {
                    if kind == FrameKind::Binary {
                        assert_eq!(preview, "/wCr");
                        saw_binary = true;
                    }
                    if kind == FrameKind::Pong && preview == "probe" {
                        saw_pong = true;
                    }
                }
                Event::Finished { .. } => break,
                _ => {}
            }
            if saw_binary && saw_pong {
                break;
            }
        }
        assert!(saw_binary && saw_pong);
    }

    #[tokio::test]
    async fn server_ping_answers_with_pong() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        let mut s = open(&settings(&echo.url("/ping"))).await.unwrap();
        let mut saw_ping = false;
        let mut saw_pong_sent = false;
        while let Some(ev) = s.events.recv().await {
            match ev {
                Event::Received { kind, preview, .. } => {
                    if kind == FrameKind::Ping {
                        assert_eq!(preview, "srv-ping");
                        saw_ping = true;
                    }
                }
                Event::Sent { kind, .. } => {
                    if kind == FrameKind::Pong {
                        saw_pong_sent = true;
                    }
                }
                Event::Finished { .. } => break,
                _ => {}
            }
            if saw_ping && saw_pong_sent {
                break;
            }
        }
        assert!(saw_ping && saw_pong_sent);
    }

    #[tokio::test]
    async fn handshake_refusal_and_bad_url() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        // A path that is not an echo endpoint answers plain 404.
        let Err(e) = open(&settings(&echo.url("/nope"))).await else {
            panic!("the handshake should have failed");
        };
        assert!(e.to_string().contains("404"), "{e}");
        assert!(open(&settings("http://127.0.0.1:1/x")).await.is_err());
    }

    #[tokio::test]
    async fn server_initiated_close() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        let mut s = open(&settings(&echo.url("/close/4321/server%20closing")))
            .await
            .unwrap();
        s.send_text("trigger".into()).unwrap();
        let finished = loop {
            match s.events.recv().await {
                Some(Event::Finished { code, reason, .. }) => break (code, reason),
                Some(_) => {}
                None => panic!("session ended without Finished"),
            }
        };
        assert_eq!(finished.0, 4321);
        assert_eq!(finished.1, "server closing");
    }

    #[tokio::test]
    async fn tls_session() {
        let certs = sendito_testkit::tls::generate().unwrap();
        let echo = sendito_testkit::ws_echo::start_tls(&certs).await.unwrap();
        let mut s = settings(&echo.url("/echo"));
        s.verify_tls = false;
        let mut session = open(&s).await.unwrap();
        session.send_text("over tls".into()).unwrap();
        let mut got = false;
        while let Some(ev) = session.events.recv().await {
            if let Event::Received { preview, .. } = ev {
                assert_eq!(preview, "over tls");
                got = true;
                break;
            }
        }
        assert!(got);
    }

    #[test]
    fn message_document_parsing() {
        // An array of text and binary frames.
        let f = parse_messages(r#"["a", {"binary": "AAEC"}, {"text": "b"}]"#).unwrap();
        assert_eq!(
            f,
            vec![
                Frame::Text("a".into()),
                Frame::Binary(vec![0, 1, 2]),
                Frame::Text("b".into()),
            ]
        );
        // One value alone, an empty document and an empty array.
        assert_eq!(
            parse_messages(r#""only""#).unwrap(),
            vec![Frame::Text("only".into())]
        );
        assert!(parse_messages("").unwrap().is_empty());
        assert!(parse_messages("  \n ").unwrap().is_empty());
        assert!(parse_messages("[]").unwrap().is_empty());
        // Errors name the problem.
        for (doc, needle) in [
            ("[", "messages:"),
            ("[1]", "expected a string"),
            (r#"[{"binary": "!!"}]"#, "base64"),
            (r#"[{"binary": 1}]"#, "base64 string"),
            (r#"[{"text": 1}]"#, "must be a string"),
            (r#"[{"other": "x"}]"#, "must be \"text\" or \"binary\""),
            ("[{}]", "expected a string"),
        ] {
            let e = parse_messages(doc).unwrap_err();
            assert!(e.contains(needle), "{doc}: {e}");
        }
    }

    #[test]
    fn proxy_spec_parsing() {
        use sendito_http::ProxySpec;
        assert!(WsProxy::of(None).unwrap().is_none());
        // An HTTP(S) proxy is a CONNECT tunnel.
        for u in ["http://p:8080", "https://p:443"] {
            let p = WsProxy::of(Some(&ProxySpec::Url(u.into())))
                .unwrap()
                .unwrap();
            assert_eq!(p, WsProxy::Connect { url: u.into() });
        }
        // SOCKS and environment discovery are refused with a reason.
        for spec in [ProxySpec::Url("socks5://p:1080".into()), ProxySpec::Env] {
            let e = WsProxy::of(Some(&spec)).unwrap_err();
            assert!(
                e.contains("not supported") || e.contains("proxy = \"env\""),
                "{e}"
            );
        }
        // Credentials are decoded from the URL.
        let p = WsProxy::Connect {
            url: "http://us%40er:p%3Ass@127.0.0.1:3128".into(),
        };
        assert_eq!(p.credentials().as_deref(), Some("us@er:p:ss"));
        assert_eq!(p.endpoint().unwrap(), ("127.0.0.1".into(), 3128, false));
        let p = WsProxy::Connect {
            url: "https://127.0.0.1".into(),
        };
        assert_eq!(p.endpoint().unwrap(), ("127.0.0.1".into(), 443, true));
        assert!(
            WsProxy::Connect {
                url: "http://".into()
            }
            .endpoint()
            .is_err()
        );
        assert_eq!(percent_decode("%41%42x%"), "ABx%");
    }

    #[tokio::test]
    async fn extra_headers_and_frame_kinds() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        let mut s = settings(&echo.url("/echo"));
        s.headers = vec![
            ("x-one".into(), "1".into()),
            ("origin".into(), "https://app.example".into()),
        ];
        let mut session = open(&s).await.unwrap();
        let head = loop {
            if let Ok(Event::Handshake { .. }) = session.events.try_recv() {
                break echo.last().map(|c| c.head()).unwrap_or_default();
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        };
        assert!(head.contains("x-one: 1"), "{head}");
        assert!(
            head.to_ascii_lowercase()
                .contains("origin: https://app.example")
        );
        assert_eq!(FrameKind::Text.as_str(), "text");
        assert_eq!(FrameKind::Binary.label(), "Binary");
        assert!(FrameKind::Text.has_payload());
        assert!(!FrameKind::Ping.has_payload());
    }

    #[tokio::test]
    async fn deadline_and_cancel() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        // A session deadline ends the session with the deadline message.
        let mut s = settings(&echo.url("/echo"));
        s.deadline = Some(Duration::from_millis(80));
        let mut session = open(&s).await.unwrap();
        let finished = loop {
            match session.events.recv().await {
                Some(Event::Finished { code, reason, .. }) => break (code, reason),
                Some(_) => {}
                None => panic!("no Finished"),
            }
        };
        assert_eq!(finished.0, 1000);
        assert!(finished.1.contains("deadline"), "{}", finished.1);

        // Cancel aborts and reports it once.
        let mut session = open(&settings(&echo.url("/echo"))).await.unwrap();
        session.cancel();
        let mut saw = 0;
        while let Ok(ev) = session.events.try_recv() {
            if matches!(ev, Event::Finished { .. }) {
                saw += 1;
            }
        }
        assert_eq!(saw, 1);
        // A cancelled session cannot send.
        assert!(session.send_text("x".into()).is_err());
    }

    #[tokio::test]
    async fn tls_with_custom_ca() {
        let certs = sendito_testkit::tls::generate().unwrap();
        let echo = sendito_testkit::ws_echo::start_tls(&certs).await.unwrap();
        let ca = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(ca.path(), &certs.ca_pem).unwrap();
        // The custom CA replaces the platform roots, so verification succeeds.
        let mut s = settings(&echo.url("/echo"));
        s.tls.ca = Some(ca.path().to_path_buf());
        let mut session = open(&s).await.unwrap();
        session.send_text("via ca".into()).unwrap();
        let mut got = false;
        while let Some(ev) = session.events.recv().await {
            if let Event::Received { preview, .. } = ev {
                assert_eq!(preview, "via ca");
                got = true;
                break;
            }
        }
        assert!(got);
        // Verification on with the platform roots refuses the test certificate.
        let Err(e) = open(&settings(&echo.url("/echo"))).await else {
            panic!("the test certificate must be refused by the platform roots");
        };
        assert!(e.to_string().contains("cannot connect"), "{e}");
    }

    #[tokio::test]
    async fn through_a_connect_proxy() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        let proxy = sendito_testkit::proxy::start_connect().await.unwrap();
        let mut s = settings(&echo.url("/echo"));
        s.proxy = Some(WsProxy::Connect {
            url: format!("http://user:pass@127.0.0.1:{}", proxy.addr.port()),
        });
        let mut session = open(&s).await.unwrap();
        session.send_text("tunnelled".into()).unwrap();
        let mut got = false;
        while let Some(ev) = session.events.recv().await {
            if let Event::Received { preview, .. } = ev {
                assert_eq!(preview, "tunnelled");
                got = true;
                break;
            }
        }
        assert!(got);
        let targets = proxy.take();
        assert!(
            targets
                .iter()
                .any(|t| t == &format!("CONNECT 127.0.0.1:{}", echo.addr.port())),
            "{targets:?}"
        );
        // The proxy saw the credentials as a Basic header.
        assert_eq!(
            proxy.take_auth(),
            vec![format!(
                "Basic {}",
                base64::engine::general_purpose::STANDARD.encode("user:pass")
            )]
        );
    }

    #[tokio::test]
    async fn proxy_refusal_is_reported() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        // The plain proxy refuses CONNECT with 502.
        let proxy = sendito_testkit::proxy::start().await.unwrap();
        let mut s = settings(&echo.url("/echo"));
        s.proxy = Some(WsProxy::Connect { url: proxy.url() });
        let Err(e) = open(&s).await else {
            panic!("the proxy must refuse CONNECT");
        };
        assert!(e.to_string().contains("refused CONNECT (502)"), "{e}");
    }

    #[tokio::test]
    async fn through_a_tls_connect_proxy() {
        let certs = sendito_testkit::tls::generate().unwrap();
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        let proxy = sendito_testkit::proxy::start_connect_tls(&certs)
            .await
            .unwrap();
        assert!(proxy.url().starts_with("https://"));
        let ca = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(ca.path(), &certs.ca_pem).unwrap();
        let mut s = settings(&echo.url("/echo"));
        // The proxy's TLS is verified against the test CA; the session is plain
        // ws:// so there is only the proxy's TLS in play.
        s.tls.ca = Some(ca.path().to_path_buf());
        s.proxy = Some(WsProxy::Connect { url: proxy.url() });
        let mut session = open(&s).await.unwrap();
        session.send_text("via tls proxy".into()).unwrap();
        let mut got = false;
        while let Some(ev) = session.events.recv().await {
            if let Event::Received { preview, .. } = ev {
                assert_eq!(preview, "via tls proxy");
                got = true;
                break;
            }
        }
        assert!(got);
        assert!(
            proxy
                .take()
                .iter()
                .any(|t| t == &format!("CONNECT 127.0.0.1:{}", echo.addr.port()))
        );
    }

    #[tokio::test]
    async fn a_peer_that_never_answers_the_close_is_bounded() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        let mut s = settings(&echo.url("/mute"));
        s.close_timeout = Duration::from_millis(80);
        let mut session = open(&s).await.unwrap();
        session.send_text("bye".into()).unwrap();
        session.close();
        let finished = loop {
            match session.events.recv().await {
                Some(Event::Finished { code, reason, .. }) => break (code, reason),
                Some(_) => {}
                None => panic!("session ended without Finished"),
            }
        };
        // The peer never answers: the limit ends the session instead of hanging.
        assert_eq!(finished.0, 1006);
        assert!(finished.1.contains("close handshake"), "{}", finished.1);
        assert_eq!(CLOSE_TIMEOUT, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn no_handshake_timeout_waits_for_the_server() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        let mut s = settings(&echo.url("/echo"));
        s.handshake_timeout = None;
        let mut session = open(&s).await.unwrap();
        session.send_text("no limit".into()).unwrap();
        let mut got = false;
        while let Some(ev) = session.events.recv().await {
            if let Event::Received { preview, .. } = ev {
                assert_eq!(preview, "no limit");
                got = true;
                break;
            }
        }
        assert!(got);
    }

    #[tokio::test]
    async fn subprotocol_negotiated() {
        let echo = sendito_testkit::ws_echo::start().await.unwrap();
        let mut s = settings(&echo.url("/echo"));
        s.subprotocols = vec!["chat.v2".into(), "chat.v1".into()];
        let mut session = open(&s).await.unwrap();
        while let Some(ev) = session.events.recv().await {
            if let Event::Handshake { subprotocol, .. } = ev {
                assert_eq!(subprotocol.as_deref(), Some("chat.v2"));
                break;
            }
        }
    }
}
