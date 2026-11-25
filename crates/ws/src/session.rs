//! An open WebSocket session and the task that drives it.

use super::*;

/// Commands to a running session.
#[derive(Debug)]
pub(super) enum Cmd {
    Text(String),
    Binary(Vec<u8>),
    Ping(Vec<u8>),
    Close(u16, String),
}

/// A running session. Drop or [`Session::cancel`] to abort it.
pub struct Session {
    pub events: mpsc::UnboundedReceiver<Event>,
    pub(super) event_tx: mpsc::UnboundedSender<Event>,
    pub(super) cmd: Option<mpsc::UnboundedSender<Cmd>>,
    pub(super) task: tokio::task::JoinHandle<()>,
    pub(super) start: Instant,
}

impl Session {
    /// Whether the run task is still alive.
    pub fn is_finished(&self) -> bool {
        self.task.is_finished()
    }

    pub(super) fn send(&self, cmd: Cmd) -> Result<(), WsError> {
        self.cmd
            .as_ref()
            .ok_or_else(|| WsError::Send("the session is closed".into()))?
            .send(cmd)
            .map_err(|_| WsError::Send("the session has ended".into()))
    }

    pub fn send_text(&self, text: String) -> Result<(), WsError> {
        self.send(Cmd::Text(text))
    }

    pub fn send_binary(&self, bytes: Vec<u8>) -> Result<(), WsError> {
        self.send(Cmd::Binary(bytes))
    }

    pub fn ping(&self, payload: Vec<u8>) -> Result<(), WsError> {
        self.send(Cmd::Ping(payload))
    }

    /// Starts the close handshake (1000). The session ends when the peer
    /// answers its close frame.
    pub fn close(&mut self) {
        if let Some(tx) = self.cmd.take() {
            let _ = tx.send(Cmd::Close(1000, String::new()));
        }
    }

    /// Aborts the session without a close handshake.
    pub fn cancel(&mut self) {
        self.cmd = None;
        if !self.task.is_finished() {
            self.task.abort();
            let _ = self.event_tx.send(Event::Finished {
                code: 1006,
                reason: "cancelled by user".into(),
                at: self.start.elapsed(),
            });
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// How long the close handshake may take after our close frame went out
/// before the session gives up and reports the connection as dropped (1006).
pub const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

/// Reads and writes frames until the session ends: the peer's close frame, a
/// protocol error, a drop of the connection, or a cancel (task abort).
pub(super) async fn run(
    ws: WebSocketStream<Transport>,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
    events: mpsc::UnboundedSender<Event>,
    start: Instant,
    deadline: Option<Duration>,
    close_timeout: Duration,
) {
    let (mut sink, mut read) = ws.split();
    let sent = |kind: FrameKind, preview: String, size: usize| Event::Sent {
        kind,
        preview,
        size,
        at: start.elapsed(),
        wall: SystemTime::now(),
    };
    let received = |kind: FrameKind, preview: String, size: usize| Event::Received {
        kind,
        preview,
        size,
        at: start.elapsed(),
        wall: SystemTime::now(),
    };
    let finish = |code: u16, reason: String| {
        let _ = events.send(Event::Finished {
            code,
            reason,
            at: start.elapsed(),
        });
    };
    let preview_of = |m: &Message| -> (FrameKind, String, usize) {
        match m {
            Message::Text(t) => (FrameKind::Text, t.to_string(), t.len()),
            Message::Binary(b) => (
                FrameKind::Binary,
                base64::engine::general_purpose::STANDARD.encode(b.iter()),
                b.len(),
            ),
            Message::Ping(p) => (
                FrameKind::Ping,
                String::from_utf8_lossy(p).into_owned(),
                p.len(),
            ),
            Message::Pong(p) => (
                FrameKind::Pong,
                String::from_utf8_lossy(p).into_owned(),
                p.len(),
            ),
            Message::Close(f) => (
                FrameKind::Close,
                f.as_ref()
                    .map(|c| format!("{} {}", u16::from(c.code), c.reason))
                    .unwrap_or_default(),
                0,
            ),
            Message::Frame(_) => (FrameKind::Close, String::new(), 0),
        }
    };
    let mut rx_open = true;
    let mut our_close: Option<u16> = None;
    let mut deadline_done = false;
    let deadline_at = deadline.map(|d| tokio::time::Instant::from_std(start + d));
    let mut deadline_sleep = std::pin::pin!(tokio::time::sleep_until(
        deadline_at.unwrap_or_else(tokio::time::Instant::now)
    ));
    // Armed when our close frame goes out: the peer gets `close_timeout` to
    // answer, then the session ends as dropped instead of hanging forever.
    let mut close_sleep = std::pin::pin!(tokio::time::sleep_until(tokio::time::Instant::now()));
    loop {
        tokio::select! {
            _ = &mut deadline_sleep, if deadline.is_some() && !deadline_done && our_close.is_none() => {
                deadline_done = true;
                let reason = format!(
                    "deadline of {} ms reached",
                    deadline.map(|d| d.as_millis()).unwrap_or_default()
                );
                // The deadline is a normal end of what the request asked for,
                // so the session closes with 1000 (RFC 6455) instead of
                // dropping the connection; the reason names why.
                rx_open = false;
                our_close = Some(1000);
                let close_at = tokio::time::Instant::now() + close_timeout;
                close_sleep.as_mut().reset(close_at);
                let close = sink.send(Message::Close(Some(CloseFrame {
                    code: 1000.into(),
                    reason: Utf8Bytes::from(reason),
                })));
                if let Err(e) = write_within(close, Some(close_at)).await {
                    finish(1006, e);
                    return;
                }
                let _ = events.send(sent(FrameKind::Close, String::new(), 0));
            }
            _ = &mut close_sleep, if our_close.is_some() => {
                finish(
                    1006,
                    format!(
                        "the peer did not answer the close handshake within {} ms",
                        close_timeout.as_millis()
                    ),
                );
                return;
            }
            cmd = rx.recv(), if rx_open => {
                let Some(cmd) = cmd else {
                    // The close request or a dropped owner closed the channel.
                    rx_open = false;
                    continue;
                };
                // Data frames must go out before the deadline.
                let until = deadline_at.filter(|_| !deadline_done);
                let out = match cmd {
                    Cmd::Text(t) => {
                        let len = t.len();
                        write_within(sink.send(Message::Text(t.clone().into())), until).await
                            .map(|_| { let _ = events.send(sent(FrameKind::Text, t, len)); })
                    }
                    Cmd::Binary(b) => {
                        let preview = base64::engine::general_purpose::STANDARD.encode(b.iter());
                        let len = b.len();
                        write_within(sink.send(Message::Binary(b.into())), until).await
                            .map(|_| { let _ = events.send(sent(FrameKind::Binary, preview, len)); })
                    }
                    Cmd::Ping(p) => {
                        let preview = String::from_utf8_lossy(&p).into_owned();
                        let len = p.len();
                        write_within(sink.send(Message::Ping(p.into())), until).await
                            .map(|_| { let _ = events.send(sent(FrameKind::Ping, preview, len)); })
                    }
                    Cmd::Close(code, reason) => {
                        // After our close, only the peer's answer matters.
                        rx_open = false;
                        our_close = Some(code);
                        let close_at = tokio::time::Instant::now() + close_timeout;
                        close_sleep.as_mut().reset(close_at);
                        let close = sink.send(Message::Close(Some(CloseFrame {
                            code: code.into(),
                            reason: Utf8Bytes::from(reason),
                        })));
                        write_within(close, Some(close_at)).await
                            .map(|_| { let _ = events.send(sent(FrameKind::Close, String::new(), 0)); })
                    }
                };
                if let Err(e) = out {
                    finish(1006, e);
                    return;
                }
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(m @ (Message::Text(_) | Message::Binary(_) | Message::Pong(_)))) => {
                        let (kind, preview, size) = preview_of(&m);
                        let _ = events.send(received(kind, preview, size));
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = events.send(received(
                            FrameKind::Ping,
                            String::from_utf8_lossy(&p).into_owned(),
                            p.len(),
                        ));
                        // The library queued the pong reply; flush drives it out.
                        let preview = String::from_utf8_lossy(&p).into_owned();
                        let until = if our_close.is_some() {
                            Some(close_sleep.deadline())
                        } else {
                            deadline_at.filter(|_| !deadline_done)
                        };
                        match write_within(sink.flush(), until).await {
                            Ok(()) => {
                                let _ = events.send(sent(FrameKind::Pong, preview, 0));
                            }
                            Err(e) => {
                                finish(1006, e);
                                return;
                            }
                        }
                    }
                    Some(Ok(Message::Close(f))) => {
                        // The library queued the close echo; flush drives it out.
                        let _ = write_within(
                            sink.flush(),
                            Some(tokio::time::Instant::now() + close_timeout),
                        )
                        .await;
                        let (code, reason) = f
                            .map(|c| (u16::from(c.code), c.reason.to_string()))
                            .unwrap_or((1005, String::new()));
                        finish(code, reason);
                        return;
                    }
                    Some(Ok(Message::Frame(_))) => {}
                    Some(Err(e)) => {
                        // After our close request, ConnectionClosed or a TCP
                        // drop is the session ending as asked, not an error.
                        if our_close.is_some()
                            && matches!(
                                e,
                                tokio_tungstenite::tungstenite::Error::ConnectionClosed
                            )
                        {
                            finish(our_close.unwrap_or(1000), String::new());
                        } else {
                            finish(1006, error_chain(&e));
                        }
                        return;
                    }
                    None => {
                        if our_close.is_some() {
                            finish(our_close.unwrap_or(1000), String::new());
                        } else {
                            finish(1006, "the connection closed".into());
                        }
                        return;
                    }
                }
            }
        }
    }
}

/// Runs one write with a time limit. A peer that stops reading blocks a
/// write, and while it waits the session loop runs no timer, so the write
/// itself must end when the deadline or the close timer is due.
pub(super) async fn write_within(
    write: impl std::future::Future<Output = Result<(), tokio_tungstenite::tungstenite::Error>>,
    until: Option<tokio::time::Instant>,
) -> Result<(), String> {
    let r = match until {
        Some(t) => tokio::time::timeout_at(t, write)
            .await
            .map_err(|_| "the peer stopped reading; a write did not finish in time".to_string())?,
        None => write.await,
    };
    r.map_err(|e| error_chain(&e))
}
