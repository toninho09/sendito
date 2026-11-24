//! A WebSocket echo server that records the exact handshake bytes and frames.
//!
//! Routes:
//! * `/echo` — echo text and binary frames back; answers pings with a pong
//!   carrying the same payload; selects the first offered subprotocol
//! * `/proto/<name>` — like `/echo`, but the handshake answers with
//!   `Sec-WebSocket-Protocol: <name>` regardless of what was offered
//! * `/close/<code>/<reason>` — on the first received frame, close with that code
//! * `/once/<msg>` — right after the handshake, one text frame then a clean close
//! * `/ping` — right after the handshake, a `Ping` with payload `srv-ping`, then echo
//! * `/big/<n>` — right after the handshake, one text frame of `n` bytes, then close
//! * `/mute` — echoes frames but never answers a close (the client must time out)
//! * anything else — a plain `404` (not an upgrade)

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use futures_util::{SinkExt, StreamExt};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

/// One session exactly as the server saw it.
#[derive(Debug, Clone, Default)]
pub struct Captured {
    /// The handshake request head, byte for byte.
    pub raw: Vec<u8>,
    /// Frames in order: `true` = the server sent it, `false` = it received it.
    pub frames: Vec<(bool, String, Vec<u8>)>,
}

impl Captured {
    pub fn head(&self) -> String {
        String::from_utf8_lossy(&self.raw).into_owned()
    }

    /// Human-readable dump: the handshake head verbatim, then one line per
    /// frame (`<` received, `>` sent; text frames as text, others as base64).
    pub fn dump(&self) -> String {
        let mut s = self.head();
        for (out, kind, payload) in &self.frames {
            let dir = if *out { '>' } else { '<' };
            match std::str::from_utf8(payload) {
                Ok(t) => s.push_str(&format!("{dir} {kind} {} {t}\n", payload.len())),
                Err(_) => s.push_str(&format!(
                    "{dir} {kind} {} {}\n",
                    payload.len(),
                    base64::engine::general_purpose::STANDARD.encode(payload)
                )),
            }
        }
        s
    }
}

pub struct WsEcho {
    pub addr: SocketAddr,
    pub captures: Arc<Mutex<Vec<Captured>>>,
    /// `true` when the listener speaks TLS (`start_tls`).
    pub secure: bool,
    task: tokio::task::JoinHandle<()>,
}

impl WsEcho {
    pub fn url(&self, path: &str) -> String {
        let scheme = if self.secure { "wss" } else { "ws" };
        format!("{scheme}://127.0.0.1:{}{}", self.addr.port(), path)
    }

    pub fn take(&self) -> Vec<Captured> {
        self.captures
            .lock()
            .map(|mut c| std::mem::take(&mut *c))
            .unwrap_or_default()
    }

    pub fn last(&self) -> Option<Captured> {
        self.captures.lock().ok().and_then(|c| c.last().cloned())
    }
}

impl Drop for WsEcho {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts the echo server on a random local port.
pub async fn start() -> std::io::Result<WsEcho> {
    start_on("127.0.0.1:0").await
}

/// Starts the echo server over TLS with the test certificate.
pub async fn start_tls(
    certs: &crate::tls::TestCerts,
) -> Result<WsEcho, Box<dyn std::error::Error + Send + Sync>> {
    let cfg = crate::tls::server_config(certs, false, false)?;
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(cfg));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let captures: Arc<Mutex<Vec<Captured>>> = Arc::default();
    let c2 = captures.clone();
    let task = tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let c = c2.clone();
            let a = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(sock) = a.accept(sock).await {
                    let _ = serve(sock, c).await;
                }
            });
        }
    });
    Ok(WsEcho {
        addr,
        captures,
        secure: true,
        task,
    })
}

pub async fn start_on(addr: &str) -> std::io::Result<WsEcho> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let captures: Arc<Mutex<Vec<Captured>>> = Arc::default();
    let c2 = captures.clone();
    let task = tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let c = c2.clone();
            tokio::spawn(async move {
                let _ = serve(sock, c).await;
            });
        }
    });
    Ok(WsEcho {
        addr,
        captures,
        secure: false,
        task,
    })
}

/// A stream that replays buffered bytes before the underlying socket, so the
/// server can capture the raw handshake head and still let the library parse it.
struct Prefixed<S> {
    prefix: std::io::Cursor<Vec<u8>>,
    inner: S,
}

impl<S: AsyncRead + Unpin> AsyncRead for Prefixed<S> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let filled = self.prefix.get_ref().len();
        let pos = self.prefix.position() as usize;
        if pos < filled {
            let end = (pos + buf.remaining()).min(filled);
            buf.put_slice(&self.prefix.get_ref()[pos..end]);
            self.prefix.set_position(end as u64);
            return std::task::Poll::Ready(Ok(()));
        }
        std::pin::Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for Prefixed<S> {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        std::pin::Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}

async fn close_session<S>(
    ws: &mut tokio_tungstenite::WebSocketStream<Prefixed<S>>,
    code: u16,
    reason: &str,
    cap: &Arc<Mutex<Vec<Captured>>>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let payload = format!("{code} {reason}").into_bytes();
    let _ = ws
        .send(Message::Close(Some(
            tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: code.into(),
                reason: reason.into(),
            },
        )))
        .await;
    if let Ok(mut c) = cap.lock()
        && let Some(last) = c.last_mut()
    {
        last.frames.push((true, "close".into(), payload));
    }
}

async fn serve<S>(sock: S, captures: Arc<Mutex<Vec<Captured>>>) -> std::io::Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    // Read the handshake head byte by byte so no frame byte is consumed.
    let mut sock = sock;
    let mut raw = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        let n = sock.read(&mut byte).await?;
        if n == 0 {
            return Ok(());
        }
        raw.push(byte[0]);
        if raw.ends_with(b"\r\n\r\n") {
            break;
        }
        if raw.len() > 16 * 1024 {
            return Ok(());
        }
    }
    let head = String::from_utf8_lossy(&raw).into_owned();
    let path = head
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .unwrap_or("")
        .to_string();
    let path_part = path.split('?').next().unwrap_or("").to_string();
    let segments: Vec<&str> = path_part.trim_start_matches('/').split('/').collect();
    let route = segments.first().copied().unwrap_or("");

    if route != "echo"
        && route != "proto"
        && route != "close"
        && route != "once"
        && route != "ping"
        && route != "big"
        && route != "mute"
    {
        sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n")
            .await?;
        return Ok(());
    }

    let proto_choice = if route == "proto" {
        segments
            .get(1)
            .map(|s| percent_decode(s))
            .filter(|s| !s.is_empty())
    } else {
        None
    };
    // The callback signature is fixed by the library (large Err variant).
    #[allow(clippy::result_large_err)]
    let cb = move |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
        let wanted = req
            .headers()
            .get("Sec-WebSocket-Protocol")
            .and_then(|v| v.to_str().ok())
            .map(|v| v.split(',').map(str::trim).collect::<Vec<_>>());
        let pick = proto_choice
            .clone()
            .or_else(|| wanted.and_then(|w| w.first().map(|s| s.to_string())));
        if let Some(p) = pick
            && let Ok(v) = p.parse()
        {
            resp.headers_mut().append("Sec-WebSocket-Protocol", v);
        }
        Ok(resp)
    };
    let prefixed = Prefixed {
        prefix: std::io::Cursor::new(raw.clone()),
        inner: sock,
    };
    let mut ws = tokio_tungstenite::accept_hdr_async(prefixed, cb)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let record = |cap: &Arc<Mutex<Vec<Captured>>>, out: bool, kind: &str, payload: Vec<u8>| {
        if let Ok(mut c) = cap.lock()
            && let Some(last) = c.last_mut()
        {
            last.frames.push((out, kind.to_string(), payload));
        }
    };
    if let Ok(mut c) = captures.lock() {
        c.push(Captured {
            raw: raw.clone(),
            frames: Vec::new(),
        });
    }

    // Right-after-handshake behaviours.
    match route {
        "once" => {
            let msg = segments
                .get(1)
                .map(|s| percent_decode(s))
                .unwrap_or_default();
            if ws.send(Message::Text(msg.clone().into())).await.is_ok() {
                record(&captures, true, "text", msg.into_bytes());
            }
            close_session(&mut ws, 1000, "bye", &captures).await;
            return Ok(());
        }
        "ping" => {
            let payload = b"srv-ping".to_vec();
            if ws.send(Message::Ping(payload.clone().into())).await.is_ok() {
                record(&captures, true, "ping", payload);
            }
        }
        "big" => {
            let n: usize = segments.get(1).and_then(|s| s.parse().ok()).unwrap_or(1024);
            let payload = vec![b'x'; n];
            let text = String::from_utf8_lossy(&payload).into_owned();
            if ws.send(Message::Text(text.into())).await.is_ok() {
                record(&captures, true, "text", payload);
            }
            close_session(&mut ws, 1000, "bye", &captures).await;
            return Ok(());
        }
        _ => {}
    }

    while let Some(msg) = ws.next().await {
        let msg = match msg {
            Ok(m) => m,
            Err(_) => break,
        };
        match msg {
            Message::Text(t) => {
                let bytes = t.as_bytes().to_vec();
                record(&captures, false, "text", bytes.clone());
                if route == "close" {
                    let code: u16 = segments.get(1).and_then(|s| s.parse().ok()).unwrap_or(1000);
                    let reason = segments
                        .get(2)
                        .map(|s| percent_decode(s))
                        .unwrap_or_default();
                    close_session(&mut ws, code, &reason, &captures).await;
                    break;
                }
                if ws.send(Message::Text(t)).await.is_ok() {
                    record(&captures, true, "text", bytes);
                }
            }
            Message::Binary(b) => {
                let bytes = b.as_ref().to_vec();
                record(&captures, false, "binary", bytes.clone());
                if route == "close" {
                    let code: u16 = segments.get(1).and_then(|s| s.parse().ok()).unwrap_or(1000);
                    let reason = segments
                        .get(2)
                        .map(|s| percent_decode(s))
                        .unwrap_or_default();
                    close_session(&mut ws, code, &reason, &captures).await;
                    break;
                }
                if ws.send(Message::Binary(b)).await.is_ok() {
                    record(&captures, true, "binary", bytes);
                }
            }
            Message::Ping(p) => {
                let bytes = p.as_ref().to_vec();
                record(&captures, false, "ping", bytes.clone());
                // The library queued the pong reply; flush drives it out.
                if ws.flush().await.is_ok() {
                    record(&captures, true, "pong", bytes);
                }
            }
            Message::Pong(_) => {}
            Message::Close(f) => {
                let payload = f
                    .as_ref()
                    .map(|c| format!("{} {}", u16::from(c.code), c.reason))
                    .unwrap_or_default()
                    .into_bytes();
                record(&captures, false, "close", payload.clone());
                // `/mute` records the close but never answers it; the socket
                // stays open so only the client's own timeout ends the session.
                if route == "mute" {
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    return Ok(());
                }
                // RFC 6455: the library queued the close echo; flush drives it.
                if ws.flush().await.is_ok() {
                    record(&captures, true, "close", payload);
                }
                break;
            }
            Message::Frame(_) => {}
        }
    }
    Ok(())
}

fn percent_decode(s: &str) -> String {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(v) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Waits until the server has captured at least one session (tests).
pub async fn wait_capture(echo: &WsEcho) -> Captured {
    for _ in 0..100 {
        if let Some(c) = echo.last()
            && (!c.frames.is_empty() || !c.raw.is_empty())
        {
            return c;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Captured::default()
}
