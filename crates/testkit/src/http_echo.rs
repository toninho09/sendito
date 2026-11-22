//! A minimal HTTP/1.1 server that records the exact bytes of every request.
//!
//! Routes:
//! * `/status/<code>` — reply with that status
//! * `/redirect/<n>` — redirect chain ending at `/echo`
//! * `/delay/<ms>` — wait before replying
//! * `/bytes/<n>` — reply with `n` bytes
//! * `/json/<n>` — reply with a JSON document of about `n` bytes
//! * anything else — reply with a JSON summary of the request

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One request exactly as received.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Captured {
    pub raw: Vec<u8>,
    pub head_len: usize,
}

impl Captured {
    pub fn head(&self) -> String {
        String::from_utf8_lossy(&self.raw[..self.head_len]).into_owned()
    }

    pub fn request_line(&self) -> String {
        self.head()
            .split("\r\n")
            .next()
            .unwrap_or_default()
            .to_string()
    }

    pub fn method(&self) -> String {
        self.request_line()
            .split(' ')
            .next()
            .unwrap_or_default()
            .to_string()
    }

    pub fn target(&self) -> String {
        self.request_line()
            .split(' ')
            .nth(1)
            .unwrap_or_default()
            .to_string()
    }

    /// Header lines in wire order (name as sent, value trimmed).
    pub fn headers(&self) -> Vec<(String, String)> {
        let head = &self.raw[..self.head_len];
        let text = String::from_utf8_lossy(head);
        text.split("\r\n")
            .skip(1)
            .filter(|l| !l.is_empty())
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.to_string(), v.trim().to_string()))
            .collect()
    }

    /// Header values as raw bytes (for non-ASCII values).
    pub fn header_bytes(&self, name: &str) -> Vec<Vec<u8>> {
        let head = &self.raw[..self.head_len];
        head.split(|b| *b == b'\n')
            .skip(1)
            .filter_map(|l| {
                let l = l.strip_suffix(b"\r").unwrap_or(l);
                let i = l.iter().position(|b| *b == b':')?;
                let (n, v) = l.split_at(i);
                if String::from_utf8_lossy(n).eq_ignore_ascii_case(name) {
                    let v = &v[1..];
                    let start = v.iter().position(|b| *b != b' ').unwrap_or(v.len());
                    Some(v[start..].to_vec())
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn header(&self, name: &str) -> Option<String> {
        self.headers()
            .into_iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
    }

    pub fn header_all(&self, name: &str) -> Vec<String> {
        self.headers()
            .into_iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v)
            .collect()
    }

    /// Body bytes after de-chunking.
    pub fn body(&self) -> Vec<u8> {
        let raw = &self.raw[self.head_len..];
        if self
            .header("transfer-encoding")
            .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"))
        {
            dechunk(raw)
        } else {
            raw.to_vec()
        }
    }

    /// Human-readable dump: request head verbatim, then the body (text, or
    /// base64 when it is not UTF-8).
    pub fn dump(&self) -> String {
        let mut s = self.head();
        let body = self.body();
        match std::str::from_utf8(&body) {
            Ok(t) => s.push_str(t),
            Err(_) => {
                s.push_str("<binary body, base64>\n");
                s.push_str(&base64::engine::general_purpose::STANDARD.encode(&body));
            }
        }
        s
    }
}

fn dechunk(mut raw: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    while let Some(i) = raw.windows(2).position(|w| w == b"\r\n") {
        let size_line = String::from_utf8_lossy(&raw[..i]);
        let size = usize::from_str_radix(size_line.split(';').next().unwrap_or("0").trim(), 16)
            .unwrap_or(0);
        raw = &raw[i + 2..];
        if size == 0 || raw.len() < size {
            break;
        }
        out.extend_from_slice(&raw[..size]);
        raw = raw.get(size + 2..).unwrap_or_default();
    }
    out
}

pub struct HttpEcho {
    pub addr: SocketAddr,
    pub captures: Arc<Mutex<Vec<Captured>>>,
    task: tokio::task::JoinHandle<()>,
}

impl HttpEcho {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}", self.addr.port())
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

impl Drop for HttpEcho {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Starts the echo server on a random local port.
pub async fn start() -> std::io::Result<HttpEcho> {
    start_on("127.0.0.1:0").await
}

pub async fn start_on(addr: &str) -> std::io::Result<HttpEcho> {
    let listener = TcpListener::bind(addr).await?;
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
    Ok(HttpEcho {
        addr,
        captures,
        task,
    })
}

fn find(h: &[u8], pat: &[u8]) -> Option<usize> {
    h.windows(pat.len()).position(|w| w == pat)
}

async fn serve(mut sock: TcpStream, captures: Arc<Mutex<Vec<Captured>>>) -> std::io::Result<()> {
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
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
        let content_length = head
            .split("\r\n")
            .find_map(|l| l.strip_prefix("content-length:"))
            .and_then(|v| v.trim().parse::<usize>().ok());
        let chunked = head
            .split("\r\n")
            .any(|l| l.starts_with("transfer-encoding:") && l.contains("chunked"));
        let total = if let Some(cl) = content_length {
            while buf.len() < head_end + cl {
                let n = sock.read(&mut tmp).await?;
                if n == 0 {
                    break;
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            head_end + cl
        } else if chunked {
            loop {
                if find(&buf[head_end..], b"\r\n0\r\n\r\n").is_some()
                    || buf[head_end..].starts_with(b"0\r\n\r\n")
                {
                    let end = if buf[head_end..].starts_with(b"0\r\n\r\n") {
                        head_end + 5
                    } else {
                        head_end + find(&buf[head_end..], b"\r\n0\r\n\r\n").unwrap_or(0) + 7
                    };
                    break end;
                }
                let n = sock.read(&mut tmp).await?;
                if n == 0 {
                    break buf.len();
                }
                buf.extend_from_slice(&tmp[..n]);
            }
        } else {
            head_end
        };
        let total = total.min(buf.len());
        let cap = Captured {
            raw: buf[..total].to_vec(),
            head_len: head_end,
        };
        buf.drain(..total);
        if let Ok(mut c) = captures.lock() {
            c.push(cap.clone());
        }
        let (status, extra, body, delay) = respond(&cap);
        if let Some(d) = delay {
            tokio::time::sleep(d).await;
        }
        let reason = match status {
            200 => "OK",
            201 => "Created",
            204 => "No Content",
            302 => "Found",
            404 => "Not Found",
            500 => "Internal Server Error",
            _ => "Status",
        };
        let mut resp = format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\n",
            body.len()
        );
        for (k, v) in extra {
            resp.push_str(&format!("{k}: {v}\r\n"));
        }
        resp.push_str("\r\n");
        sock.write_all(resp.as_bytes()).await?;
        if cap.method() != "HEAD" {
            sock.write_all(&body).await?;
        }
        sock.flush().await?;
    }
}

type Reply = (u16, Vec<(String, String)>, Vec<u8>, Option<Duration>);

fn respond(c: &Captured) -> Reply {
    let target = c.target();
    let path = target.split('?').next().unwrap_or("/").to_string();
    let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
    let json =
        |v: serde_json::Value| -> Vec<u8> { serde_json::to_vec_pretty(&v).unwrap_or_default() };
    let ct = || vec![("Content-Type".to_string(), "application/json".to_string())];
    match parts.as_slice() {
        ["status", code] => {
            let code: u16 = code.parse().unwrap_or(500);
            (code, ct(), json(serde_json::json!({"status": code})), None)
        }
        ["redirect", n] => {
            let n: u32 = n.parse().unwrap_or(1);
            let to = if n <= 1 {
                "/echo".to_string()
            } else {
                format!("/redirect/{}", n - 1)
            };
            (302, vec![("Location".into(), to)], Vec::new(), None)
        }
        ["set-cookie"] => (
            200,
            vec![
                ("Content-Type".into(), "application/json".into()),
                ("Set-Cookie".into(), "session=corpus-1; Path=/".into()),
                ("Set-Cookie".into(), "theme=dark; Max-Age=3600".into()),
            ],
            json(serde_json::json!({"cookies_set": 2})),
            None,
        ),
        ["delay", ms] => {
            let ms: u64 = ms.parse().unwrap_or(0);
            (
                200,
                ct(),
                json(serde_json::json!({"delayed_ms": ms})),
                Some(Duration::from_millis(ms)),
            )
        }
        ["bytes", n] => {
            let n: usize = n.parse().unwrap_or(0);
            let body: Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
            (
                200,
                vec![("Content-Type".into(), "application/octet-stream".into())],
                body,
                None,
            )
        }
        ["json", n] => {
            let n: usize = n.parse().unwrap_or(1000);
            (200, ct(), big_json(n), None)
        }
        _ => {
            let headers: Vec<serde_json::Value> = c
                .headers()
                .into_iter()
                .map(|(n, v)| serde_json::json!([n, v]))
                .collect();
            let body = c.body();
            let v = serde_json::json!({
                "method": c.method(),
                "target": target,
                "headers": headers,
                "body": String::from_utf8_lossy(&body),
                "body_len": body.len(),
                "session": "server-session-1",
            });
            (
                200,
                {
                    let mut h = ct();
                    h.push(("X-Echo".into(), "1".into()));
                    h
                },
                json(v),
                None,
            )
        }
    }
}

/// A JSON document of roughly `n` bytes with nested objects and arrays.
pub fn big_json(n: usize) -> Vec<u8> {
    let mut out = String::with_capacity(n + 256);
    out.push_str("{\"items\":[");
    let mut i = 0usize;
    while out.len() < n {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"id\":{i},\"name\":\"item-{i}\",\"price\":{}.{:02},\"tags\":[\"a\",\"b\",\"c\"],\"active\":{},\"meta\":{{\"created\":\"2024-01-01T00:00:00Z\",\"score\":null}}}}",
            i % 1000,
            i % 100,
            i.is_multiple_of(2)
        ));
        i += 1;
    }
    out.push_str(&format!("],\"count\":{i}}}"));
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captured_parsing() {
        let c = Captured {
            raw: b"POST /a?b=1 HTTP/1.1\r\nhost: x\r\ntransfer-encoding: chunked\r\nX-U: \xc3\xa9\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n".to_vec(),
            head_len: 0,
        };
        let head_len = find(&c.raw, b"\r\n\r\n").map(|i| i + 4).unwrap_or(0);
        let c = Captured { head_len, ..c };
        assert_eq!(c.method(), "POST");
        assert_eq!(c.target(), "/a?b=1");
        assert_eq!(c.body(), b"abcde");
        assert_eq!(c.header_bytes("x-u"), vec!["é".as_bytes().to_vec()]);
        assert_eq!(c.header_all("host"), vec!["x".to_string()]);
        assert!(c.dump().contains("abcde"));
        assert!(serde_json::from_slice::<serde_json::Value>(&big_json(5000)).is_ok());
    }
}
