//! The stdio transport: newline-delimited JSON-RPC over a child process.
//!
//! The server's stdout carries JSON-RPC messages (one per line); its stderr is
//! forwarded as `Event::Log` so the UI can show the server's own logs. The
//! child is killed when the connection is dropped.

use std::future::Future;
use std::pin::Pin;
use std::time::Instant;

use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::mpsc::UnboundedSender;

use crate::{Event, McpError, Wire};

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
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(Event::Log {
                    line,
                    at: start.elapsed(),
                });
            }
        });
    }
    Ok(Box::new(StdioConn {
        reader: BufReader::new(stdout),
        writer: stdin,
        _child: child,
    }))
}

fn pipe(which: &'static str) -> McpError {
    McpError::Message(format!("the server process has no {which} pipe"))
}

struct StdioConn {
    reader: BufReader<ChildStdout>,
    writer: ChildStdin,
    /// Held so the child is killed when the session ends.
    _child: Child,
}

impl Wire for StdioConn {
    fn send<'a>(
        &'a mut self,
        line: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), McpError>> + Send + 'a>> {
        Box::pin(async move {
            let transport = |e: std::io::Error| McpError::Transport(e.to_string());
            self.writer
                .write_all(line.as_bytes())
                .await
                .map_err(transport)?;
            self.writer.write_all(b"\n").await.map_err(transport)?;
            self.writer.flush().await.map_err(transport)
        })
    }

    fn recv<'a>(
        &'a mut self,
    ) -> Pin<Box<dyn Future<Output = Result<Value, McpError>> + Send + 'a>> {
        Box::pin(async move {
            let mut line = String::new();
            loop {
                line.clear();
                let n = self
                    .reader
                    .read_line(&mut line)
                    .await
                    .map_err(|e| McpError::Transport(e.to_string()))?;
                if n == 0 {
                    return Err(McpError::Closed);
                }
                // A blank line carries no message: read on.
                let t = line.trim();
                if !t.is_empty() {
                    return serde_json::from_str(t).map_err(|e| McpError::Json(e.to_string()));
                }
            }
        })
    }
}
