//! Proxy settings and the HTTP CONNECT tunnel of a WebSocket.

use super::*;

/// Proxy honoured for WebSocket sessions: an explicit HTTP(S) CONNECT proxy.
/// An `https://` proxy is itself reached over TLS (verified against the
/// platform roots, or skipped when the session sets `verify_tls = false`).
/// SOCKS proxies and `proxy = "env"` discovery are not supported (DEBT.md).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WsProxy {
    /// `"http://host:port"` or `"https://host:port"`; credentials in the URL
    /// become a `Proxy-Authorization` header.
    Connect { url: String },
}

impl WsProxy {
    /// Parses the `[http] proxy` value for WebSocket use. SOCKS and `env`
    /// discovery are refused; `None` means no proxy.
    pub fn of(spec: Option<&sendito_http::ProxySpec>) -> Result<Option<Self>, String> {
        let Some(spec) = spec else { return Ok(None) };
        match spec {
            sendito_http::ProxySpec::Env => Err(
                "WebSocket does not support proxy = \"env\"; set an http:// or https:// proxy URL"
                    .into(),
            ),
            sendito_http::ProxySpec::Url(u) => {
                let parsed = url::Url::parse(u).map_err(|e| format!("invalid proxy {u:?}: {e}"))?;
                match parsed.scheme() {
                    "http" | "https" => Ok(Some(WsProxy::Connect { url: u.clone() })),
                    s => Err(format!(
                        "WebSocket through a {s} proxy is not supported; use an http:// or https:// proxy"
                    )),
                }
            }
        }
    }

    /// `(host, port, secure)` of the proxy itself.
    pub(super) fn endpoint(&self) -> Result<(String, u16, bool), String> {
        let Self::Connect { url } = self;
        let parsed = url::Url::parse(url).map_err(|e| format!("invalid proxy {url:?}: {e}"))?;
        let host = parsed
            .host_str()
            .filter(|h| !h.is_empty())
            .ok_or_else(|| format!("proxy {url:?} has no host"))?
            .to_string();
        let port = parsed
            .port_or_known_default()
            .ok_or_else(|| format!("proxy {url:?} has no port"))?;
        Ok((host, port, parsed.scheme() == "https"))
    }

    pub(super) fn credentials(&self) -> Option<String> {
        let Self::Connect { url } = self;
        let parsed = url::Url::parse(url).ok()?;
        let user = parsed.username();
        if user.is_empty() {
            return None;
        }
        Some(format!(
            "{}:{}",
            percent_decode(user),
            percent_decode(parsed.password().unwrap_or_default())
        ))
    }
}

pub(super) fn percent_decode(s: &str) -> String {
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

/// Establishes the tunnel to `host:port` through the proxy. An `https://`
/// proxy adds its own TLS layer below the session's; its certificate is
/// verified against `[http.tls] ca` when set, and the platform roots
/// otherwise (only the CA travels to the proxy, never the client pair).
pub(super) async fn through_proxy(
    proxy: &WsProxy,
    host: &str,
    port: u16,
    timeout: Option<Duration>,
    tls: &sendito_http::TlsMaterial,
) -> Result<Transport, String> {
    let (phost, pport, psecure) = proxy.endpoint()?;
    let tcp = connect_with_timeout((phost.as_str(), pport), timeout)
        .await
        .map_err(|e| e.to_string())?;
    let mut stream: Transport = if psecure {
        let material = sendito_http::TlsMaterial {
            ca: tls.ca.clone(),
            client_cert: None,
            client_key: None,
        };
        let cfg = sendito_http::tls_config(true, &[], &material)
            .map_err(|e| format!("proxy TLS: {e}"))?;
        let name = ServerName::try_from(phost.clone())
            .map_err(|e| format!("proxy name: {e}"))?
            .to_owned();
        let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
        let tls = limited(timeout, connector.connect(name, Box::new(tcp) as Transport))
            .await
            .ok_or_else(|| "proxy TLS handshake timed out".to_string())?
            .map_err(|e| format!("proxy TLS: {e}"))?;
        Box::new(tls) as Transport
    } else {
        Box::new(tcp) as Transport
    };
    let mut req = format!("CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n");
    if let Some(c) = proxy.credentials() {
        let encoded = base64::engine::general_purpose::STANDARD.encode(c.as_bytes());
        req.push_str(&format!("Proxy-Authorization: Basic {encoded}\r\n"));
    }
    req.push_str("\r\n");
    let answer = limited(timeout, async {
        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            stream
                .read_exact(&mut byte)
                .await
                .map_err(|e| e.to_string())?;
            buf.push(byte[0]);
            if buf.ends_with(b"\r\n\r\n") {
                break;
            }
            // A proxy error page can be long; stop after a sane bound.
            if buf.len() > 64 * 1024 {
                return Err("proxy answer exceeds 64 KiB".to_string());
            }
        }
        Ok(buf)
    })
    .await
    .ok_or_else(|| match timeout {
        Some(d) => format!("CONNECT timed out after {d:?}"),
        None => "CONNECT timed out".into(),
    })??;
    let head = String::from_utf8_lossy(&answer);
    let status = head
        .lines()
        .next()
        .and_then(|l| l.split(' ').nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .unwrap_or(0);
    if status != 200 {
        return Err(format!(
            "the proxy refused CONNECT ({status}): {}",
            head.lines().next().unwrap_or("").trim()
        ));
    }
    Ok(stream)
}
