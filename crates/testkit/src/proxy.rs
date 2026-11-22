//! A minimal HTTP forward proxy for tests: plain-HTTP requests in absolute
//! form are forwarded to their origin, and `CONNECT` (a TLS tunnel) is
//! refused with `502`. Every forwarded target is recorded.
//!
//! The corpus and the wire guarantees are unaffected: this server exists so
//! the `[http] proxy` option can be exercised end to end.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

pub struct ProxyEcho {
    pub addr: SocketAddr,
    targets: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

impl ProxyEcho {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
    }

    /// The absolute-form targets forwarded so far, in order.
    pub fn take(&self) -> Vec<String> {
        self.targets
            .lock()
            .map(|mut t| std::mem::take(&mut *t))
            .unwrap_or_default()
    }
}

impl Drop for ProxyEcho {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts the proxy on a random local port.
pub async fn start() -> std::io::Result<ProxyEcho> {
    start_on("127.0.0.1:0").await
}

pub async fn start_on(addr: &str) -> std::io::Result<ProxyEcho> {
    let listener = TcpListener::bind(addr).await?;
    let addr = listener.local_addr()?;
    let targets: Arc<Mutex<Vec<String>>> = Arc::default();
    let t2 = targets.clone();
    let task = tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let t = t2.clone();
            tokio::spawn(async move {
                let _ = serve(sock, t).await;
            });
        }
    });
    Ok(ProxyEcho {
        addr,
        targets,
        task,
    })
}

fn find(hay: &[u8], pat: &[u8]) -> Option<usize> {
    hay.windows(pat.len()).position(|w| w == pat)
}

/// Forwards one request per connection (the sender may pool connections and
/// send the next request on a fresh one).
async fn serve(mut client: TcpStream, targets: Arc<Mutex<Vec<String>>>) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = vec![0u8; 16 * 1024];
    let head_end = loop {
        if let Some(i) = find(&buf, b"\r\n\r\n") {
            break i + 4;
        }
        let n = client.read(&mut tmp).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut parts = head.split("\r\n").next().unwrap_or_default().split(' ');
    let method = parts.next().unwrap_or_default().to_string();
    let target = parts.next().unwrap_or_default().to_string();
    if method.eq_ignore_ascii_case("CONNECT") {
        client
            .write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\n\r\n")
            .await?;
        return Ok(());
    }
    // Only absolute-form (proxy) requests name their origin.
    let Some(rest) = target.strip_prefix("http://") else {
        client
            .write_all(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
            .await?;
        return Ok(());
    };
    let hostport = rest.split('/').next().unwrap_or_default().to_string();
    let mut path = rest[hostport.len()..].to_string();
    if path.is_empty() {
        path.push('/');
    }
    let content_length = head
        .split("\r\n")
        .find_map(|l| {
            let (n, v) = l.split_once(':')?;
            n.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    if head.to_ascii_lowercase().contains("transfer-encoding:") {
        // Chunked requests would need re-framing; nothing in the tests sends
        // one through the proxy.
        client
            .write_all(b"HTTP/1.1 501 Not Implemented\r\nContent-Length: 0\r\n\r\n")
            .await?;
        return Ok(());
    }
    while buf.len() < head_end + content_length {
        let n = client.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let mut upstream = TcpStream::connect(&hostport).await?;
    let mut forwarded = format!("{method} {path} HTTP/1.1\r\n");
    for line in head.split("\r\n").skip(1) {
        if !line.is_empty() {
            forwarded.push_str(line);
            forwarded.push_str("\r\n");
        }
    }
    forwarded.push_str("\r\n");
    upstream.write_all(forwarded.as_bytes()).await?;
    upstream.write_all(&buf[head_end..]).await?;
    // The origin always frames its response with Content-Length; copy that
    // much back and close.
    let mut resp: Vec<u8> = Vec::new();
    loop {
        if let Some(i) = find(&resp, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&resp[..i + 4]).into_owned();
            let len = head
                .split("\r\n")
                .find_map(|l| {
                    let (n, v) = l.split_once(':')?;
                    n.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| v.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            let total = i + 4 + len;
            while resp.len() < total {
                let n = upstream.read(&mut tmp).await?;
                if n == 0 {
                    break;
                }
                resp.extend_from_slice(&tmp[..n]);
            }
            break;
        }
        let n = upstream.read(&mut tmp).await?;
        if n == 0 {
            break;
        }
        resp.extend_from_slice(&tmp[..n]);
    }
    client.write_all(&resp).await?;
    if let Ok(mut t) = targets.lock() {
        t.push(target);
    }
    Ok(())
}
