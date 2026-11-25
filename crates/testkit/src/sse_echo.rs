//! SSE routes on the HTTP echo listener, including TLS listeners.
use crate::http_echo::Captured;
use std::time::Duration;
use tokio::io::{AsyncWrite, AsyncWriteExt};

pub(crate) async fn serve<S: AsyncWrite + Unpin>(
    socket: &mut S,
    request: &Captured,
) -> std::io::Result<bool> {
    let target = request.target();
    let path = target.split('?').next().unwrap_or(&target);
    let Some(route) = path.strip_prefix("/sse/") else {
        return Ok(false);
    };
    if route == "digest"
        && !crate::http_echo::digest_ok(
            &request.header("authorization").unwrap_or_default(),
            "user",
            "pass",
            &request.method(),
        )
    {
        socket.write_all(format!("HTTP/1.1 401 Unauthorized\r\nWWW-Authenticate: Digest realm=\"{}\", qop=\"auth\", algorithm=SHA-256, nonce=\"{}\"\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", crate::http_echo::DIGEST_REALM, crate::http_echo::DIGEST_NONCE).as_bytes()).await?;
        return Ok(true);
    }
    if route == "redirect-cookie" {
        socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: /sse/cookies\r\nSet-Cookie: stream-session=stream-cookie-secret; Path=/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
        return Ok(true);
    }
    if route == "redirect" {
        socket.write_all(b"HTTP/1.1 302 Found\r\nLocation: /sse/events\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await?;
        return Ok(true);
    }
    if route == "html" || route == "status" || route == "compressed" {
        let (status, headers) = match route {
            "html" => ("200 OK", "Content-Type: text/html\r\n"),
            "status" => ("204 No Content", "Content-Type: text/event-stream\r\n"),
            _ => (
                "200 OK",
                "Content-Type: text/event-stream\r\nContent-Encoding: gzip\r\n",
            ),
        };
        socket
            .write_all(
                format!(
                    "HTTP/1.1 {status}\r\n{headers}Content-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await?;
        return Ok(true);
    }
    socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream; charset=utf-8\r\nConnection: close\r\n\r\n").await?;
    match route {
        "slow" => {
            for i in 0..10 {
                socket
                    .write_all(format!("id: {i}\ndata: {{\"n\":{i}}}\n\n").as_bytes())
                    .await?;
                socket.flush().await?;
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
        "multiline" => {
            socket
                .write_all(b"event: update\ndata: first\ndata: second\n\n")
                .await?
        }
        "ids" => {
            socket
                .write_all(b"id: 42\ndata: one\n\ndata: two\n\nid:\ndata: three\n\n")
                .await?
        }
        "retry" => {
            socket
                .write_all(b"retry: 123\ndata: one\n\nretry: invalid\ndata: two\n\n")
                .await?
        }
        "comments" => {
            socket
                .write_all(b": heartbeat\n\nunknown: ignored\ndata: ok\n\ndata: discarded")
                .await?
        }
        "utf8" => {
            for byte in "\u{feff}data: héllo\r\n\r\n".as_bytes() {
                socket.write_all(&[*byte]).await?;
                socket.flush().await?;
            }
        }
        "large" => {
            socket.write_all(b"data: ").await?;
            let data = vec![b'x'; 1024 * 1024 + 1];
            socket.write_all(&data).await?;
        }
        "cookies" => {
            socket
                .write_all(
                    format!(
                        "data: {}\n\n",
                        serde_json::json!({"cookie":request.header("cookie").unwrap_or_default()})
                    )
                    .as_bytes(),
                )
                .await?
        }
        _ => socket
            .write_all(
                b"id: 1\ndata: {\"n\":1}\n\nid: 2\ndata: {\"n\":2}\n\nid: 3\ndata: {\"n\":3}\n\n",
            )
            .await?,
    }
    socket.shutdown().await?;
    Ok(true)
}
