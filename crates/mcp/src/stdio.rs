//! The stdio transport: newline-delimited JSON-RPC over a child process.
//!
//! The server's stdout carries JSON-RPC messages (one per line); its stderr is
//! forwarded as `Event::Log` so the UI can show the server's own logs. When
//! the connection is dropped, the child's stdin is closed (the MCP way to ask
//! a stdio server to stop) and the child is killed if it has not exited after
//! a short wait.

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc::UnboundedSender;

use crate::{Event, McpError, Wire};

/// The longest JSON-RPC line read from stdout; a longer one ends the session.
const MAX_LINE: u64 = 100 * 1024 * 1024;
/// A stderr log line is split into parts of at most this many bytes.
const MAX_LOG_LINE: u64 = 64 * 1024;

/// Spawns the server process and wires its pipes. `start` timestamps the log
/// events; `events` receives one `Event::Log` per stderr line.
pub(crate) fn spawn(
    command: &str,
    args: &[String],
    env: &[(String, String)],
    events: UnboundedSender<Event>,
    start: Instant,
) -> Result<Box<dyn Wire>, McpError> {
    let mut cmd = Command::new(command);
    cmd.args(args)
        .envs(env.iter().cloned())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| McpError::Spawn {
        command: command.to_string(),
        msg: e.to_string(),
    })?;
    let stdin = child.stdin.take().ok_or_else(|| pipe("stdin"))?;
    let stdout = child.stdout.take().ok_or_else(|| pipe("stdout"))?;
    if let Some(stderr) = child.stderr.take() {
        let tx = events.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(stderr);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match (&mut reader)
                    .take(MAX_LOG_LINE)
                    .read_until(b'\n', &mut buf)
                    .await
                {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf).trim_end().to_string();
                        let _ = tx.send(Event::Log {
                            line,
                            at: start.elapsed(),
                        });
                    }
                }
            }
        });
    }
    Ok(Box::new(StdioConn {
        reader: BufReader::new(stdout),
        writer: Some(stdin),
        events,
        start,
        child: Some(child),
    }))
}

fn pipe(which: &'static str) -> McpError {
    McpError::Message(format!("the server process has no {which} pipe"))
}

struct StdioConn {
    reader: BufReader<ChildStdout>,
    /// `None` once closed, when the connection is dropped.
    writer: Option<ChildStdin>,
    /// Receives stdout lines that are not JSON-RPC, as logs.
    events: UnboundedSender<Event>,
    start: Instant,
    /// Held so the child is stopped when the session ends.
    child: Option<Child>,
}

/// How long a server may take to exit after its stdin closes.
const EXIT_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

impl Drop for StdioConn {
    fn drop(&mut self) {
        // Closing stdin asks the server to exit.
        self.writer.take();
        let Some(mut child) = self.child.take() else {
            return;
        };
        // Without a runtime the child is dropped, and `kill_on_drop` kills it.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move {
                if tokio::time::timeout(EXIT_GRACE, child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.kill().await;
                }
            });
        }
    }
}

impl Wire for StdioConn {
    fn send<'a>(
        &'a mut self,
        line: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        Box::pin(async move {
            let transport = |e: std::io::Error| McpError::Transport(e.to_string());
            let writer = self.writer.as_mut().ok_or(McpError::Closed)?;
            writer.write_all(line.as_bytes()).await.map_err(transport)?;
            writer.write_all(b"\n").await.map_err(transport)?;
            writer.flush().await.map_err(transport)
        })
    }

    fn recv<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Value, McpError>> + Send + 'a>> {
        Box::pin(async move {
            let mut line = String::new();
            loop {
                line.clear();
                let n = (&mut self.reader)
                    .take(MAX_LINE)
                    .read_line(&mut line)
                    .await
                    .map_err(|e| McpError::Transport(e.to_string()))?;
                if n == 0 {
                    return Err(McpError::Closed);
                }
                if n as u64 == MAX_LINE && !line.ends_with('\n') {
                    return Err(McpError::Message(format!(
                        "the server wrote a line longer than {} MiB",
                        MAX_LINE / (1024 * 1024)
                    )));
                }
                // A blank line carries no message: read on.
                let t = line.trim();
                if t.is_empty() {
                    continue;
                }
                match serde_json::from_str(t) {
                    Ok(v) => return Ok(v),
                    // Servers that print to stdout (against the spec) do not
                    // break the session: the line is shown as a log.
                    Err(_) => {
                        let _ = self.events.send(Event::Log {
                            line: format!("stdout: {t}"),
                            at: self.start.elapsed(),
                        });
                    }
                }
            }
        })
    }
}
