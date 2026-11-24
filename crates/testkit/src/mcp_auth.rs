//! A fake OAuth 2.1 authorization server plus a protected MCP endpoint, for
//! testing the whole login flow without a browser.
//!
//! The MCP endpoint (`POST /mcp`) answers `401` with a `WWW-Authenticate`
//! pointing at `/.well-known/oauth-protected-resource` until a valid Bearer
//! token from `/token` arrives. `/authorize` validates the request (PKCE
//! included) and immediately redirects with a code, as if the user had
//! clicked allow. `/token` verifies the code against the PKCE challenge for
//! `authorization_code` and rotates tokens for `refresh_token`; access tokens
//! expire in `expires_in_secs` (1 by default).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use base64::Engine;
use sha2::Digest;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

#[derive(Default)]
struct State {
    /// code → (challenge, consumed)
    codes: HashMap<String, (String, bool)>,
    /// refresh tokens that are still valid
    refresh: HashSet<String>,
    /// access token → token id
    access: HashMap<String, u32>,
    next_id: u32,
    pub_registered: bool,
    logins: u32,
    refreshes: u32,
    http_requests: u32,
    /// Tokens issued for client_credentials, password and device_code.
    grants: u32,
    /// device code → token polls so far
    devices: HashMap<String, u32>,
}

impl State {
    fn new_id(&mut self) -> u32 {
        self.next_id += 1;
        self.next_id
    }
}

pub struct McpAuth {
    addr: SocketAddr,
    state: Arc<Mutex<State>>,
    task: JoinHandle<()>,
}

impl McpAuth {
    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// The MCP endpoint URL to configure in a client.
    pub fn mcp_url(&self) -> String {
        format!("{}/mcp", self.url())
    }

    pub fn refreshes(&self) -> u32 {
        self.state.lock().map(|s| s.refreshes).unwrap_or(0)
    }

    pub fn logins(&self) -> u32 {
        self.state.lock().map(|s| s.logins).unwrap_or(0)
    }

    pub fn registered(&self) -> bool {
        self.state.lock().map(|s| s.pub_registered).unwrap_or(false)
    }

    pub fn http_requests(&self) -> u32 {
        self.state.lock().map(|s| s.http_requests).unwrap_or(0)
    }

    /// Tokens issued without a browser: client_credentials, password and
    /// device_code.
    pub fn grants(&self) -> u32 {
        self.state.lock().map(|s| s.grants).unwrap_or(0)
    }
}

impl Drop for McpAuth {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub async fn start() -> std::io::Result<McpAuth> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let base = format!("http://{addr}");
    let state = Arc::new(Mutex::new(State::default()));
    let s2 = state.clone();
    let task = tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            let st = s2.clone();
            let b = base.clone();
            tokio::spawn(async move {
                let _ = serve(sock, st, b).await;
            });
        }
    });
    Ok(McpAuth { addr, state, task })
}

async fn serve(mut sock: TcpStream, state: Arc<Mutex<State>>, base: String) -> std::io::Result<()> {
    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = vec![0u8; 64 * 1024];
    loop {
        let head_end = loop {
            if let Some(i) = find(&buf, b"\r\n\r\n") {
                break i + 4;
            }
            let n = sock.read(&mut tmp).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
            if buf.len() > 64 * 1024 {
                return Ok(());
            }
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
        let mut lines = head.split("\r\n");
        let request_line = lines.next().unwrap_or_default().to_string();
        let headers: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(n, v)| (n.to_ascii_lowercase(), v.trim().to_string()))
            .collect();
        let len: usize = headers
            .iter()
            .find(|(n, _)| n == "content-length")
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        while buf.len() < head_end + len {
            let n = sock.read(&mut tmp).await?;
            if n == 0 {
                return Ok(());
            }
            buf.extend_from_slice(&tmp[..n]);
        }
        let body = String::from_utf8_lossy(&buf[head_end..head_end + len]).into_owned();
        buf.drain(..head_end + len);

        let mut parts = request_line.split(' ');
        let method = parts.next().unwrap_or_default().to_string();
        let target = parts.next().unwrap_or_default().to_string();
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (target.clone(), String::new()),
        };
        let query = parse_query(&query);
        match (method.as_str(), path.as_str()) {
            ("POST", "/mcp") => {
                let bearer = headers
                    .iter()
                    .find(|(n, _)| n == "authorization")
                    .and_then(|(_, v)| v.strip_prefix("Bearer ").map(str::to_string));
                let valid =
                    bearer.is_some_and(|t| state.lock().is_ok_and(|s| s.access.contains_key(&t)));
                if !valid {
                    respond(
                        &mut sock,
                        401,
                        "application/json",
                        "{\"error\":\"unauthorized\"}",
                        &format!(
                            "WWW-Authenticate: Bearer resource_metadata=\"{base}/.well-known/oauth-protected-resource\"\r\n"
                        ),
                    )
                    .await?;
                    continue;
                }
                respond(&mut sock, 200, "application/json", &mcp_answer(&body), "").await?;
            }
            ("GET", "/.well-known/oauth-protected-resource") => {
                let body = format!(
                    "{{\"resource\":\"{base}/mcp\",\"authorization_servers\":[\"{base}\"]}}"
                );
                respond(&mut sock, 200, "application/json", &body, "").await?;
            }
            ("GET", "/.well-known/oauth-authorization-server") => {
                let body = format!(
                    "{{\"issuer\":\"{base}\",\"authorization_endpoint\":\"{base}/authorize\",\
                     \"token_endpoint\":\"{base}/token\",\
                     \"registration_endpoint\":\"{base}/register\"}}"
                );
                respond(&mut sock, 200, "application/json", &body, "").await?;
            }
            ("POST", "/register") => {
                let registration: serde_json::Value =
                    serde_json::from_str(&body).unwrap_or_default();
                if registration
                    .pointer("/redirect_uris/0")
                    .and_then(|v| v.as_str())
                    != Some("http://127.0.0.1/callback")
                {
                    respond(&mut sock, 400, "application/json", "{}", "").await?;
                    continue;
                }
                if let Ok(mut s) = state.lock() {
                    s.pub_registered = true;
                }
                respond(
                    &mut sock,
                    201,
                    "application/json",
                    "{\"client_id\":\"cli-1\",\"client_secret\":\"test-client-secret\",\"client_name\":\"sendito\"}",
                    "",
                )
                .await?;
            }
            ("GET", "/authorize" | "/authorize-http") => {
                // Validate what the client built, then redirect as if allowed.
                let challenge = query.get("code_challenge").cloned();
                let redirect = query.get("redirect_uri").cloned();
                let st = query.get("state").cloned();
                let ok = query.get("response_type").map(String::as_str) == Some("code")
                    && if path == "/authorize-http" {
                        !query.contains_key("resource")
                    } else {
                        query.get("resource") == Some(&format!("{base}/mcp"))
                    }
                    && query.contains_key("client_id")
                    && challenge.is_some()
                    && query.get("code_challenge_method").map(String::as_str) == Some("S256")
                    && redirect
                        .as_deref()
                        .is_some_and(|r| r.starts_with("http://127.0.0.1"));
                if !ok {
                    respond(&mut sock, 400, "text/plain", "bad authorize request", "").await?;
                    continue;
                }
                let code = if let Ok(mut s) = state.lock() {
                    let id = s.new_id();
                    let code = format!("code-{id}");
                    s.codes
                        .insert(code.clone(), (challenge.unwrap_or_default(), false));
                    s.logins += 1;
                    code
                } else {
                    String::new()
                };
                let loc = match (redirect, st) {
                    (Some(r), Some(st)) => format!("{r}?code={code}&state={st}"),
                    (Some(r), None) => format!("{r}?code={code}"),
                    _ => String::new(),
                };
                respond(
                    &mut sock,
                    302,
                    "text/plain",
                    "redirecting",
                    &format!("Location: {loc}\r\n"),
                )
                .await?;
            }
            ("GET", "/http-api") => {
                let bearer = headers
                    .iter()
                    .find(|(n, _)| n == "authorization")
                    .and_then(|(_, v)| v.strip_prefix("Bearer "));
                // The retry after a 401 must also replay the cookie the 401 set.
                let cookie = headers
                    .iter()
                    .any(|(n, v)| n == "cookie" && v.contains("retry=ok-to-proceed"));
                let valid = state.lock().is_ok_and(|mut s| {
                    s.http_requests += 1;
                    s.http_requests > 1
                        && cookie
                        && bearer.is_some_and(|t| s.access.contains_key(t))
                });
                if valid {
                    let text = serde_json::json!({"ok": true, "token": bearer.unwrap_or_default()})
                        .to_string();
                    respond(&mut sock, 200, "application/json", &text, "").await?;
                } else {
                    respond(
                        &mut sock,
                        401,
                        "application/json",
                        "{}",
                        "Set-Cookie: retry=ok-to-proceed; Path=/\r\n",
                    )
                    .await?;
                }
            }
            // RFC 8628: the first poll is pending, the next one gets tokens.
            ("POST", "/device") => {
                let form = parse_query(&body);
                if form.get("client_id").map(String::as_str) != Some("test-client") {
                    respond(
                        &mut sock,
                        400,
                        "application/json",
                        "{\"error\":\"invalid_client\"}",
                        "",
                    )
                    .await?;
                    continue;
                }
                let code = format!("dev-{}", state.lock().map(|mut s| s.new_id()).unwrap_or(0));
                if let Ok(mut s) = state.lock() {
                    s.devices.insert(code.clone(), 0);
                }
                let text = format!(
                    "{{\"device_code\":\"{code}\",\"user_code\":\"ABCD-EFGH\",\
                     \"verification_uri\":\"{base}/device-page\",\"expires_in\":60,\"interval\":1}}"
                );
                respond(&mut sock, 200, "application/json", &text, "").await?;
            }
            ("POST", "/token" | "/token-http") => {
                let form = parse_query(&body);
                let resource = if path == "/token-http" {
                    None
                } else {
                    Some(format!("{base}/mcp"))
                };
                let out = token(&form, &state, resource.as_deref());
                let (status, text) = match out {
                    Ok(v) => (200, v),
                    Err(e) => (400, format!("{{\"error\":\"{e}\"}}")),
                };
                respond(&mut sock, status, "application/json", &text, "").await?;
            }
            _ => respond(&mut sock, 404, "text/plain", "not found", "").await?,
        }
    }
}

/// Issues or refreshes tokens, verifying PKCE for the code exchange.
fn token(
    form: &BTreeMap<String, String>,
    state: &Arc<Mutex<State>>,
    resource: Option<&str>,
) -> Result<String, String> {
    let mut s = state.lock().map_err(|_| "locked")?;
    if form.get("resource").map(String::as_str) != resource {
        return Err("invalid_target".into());
    }
    if form.get("client_secret").map(String::as_str) != Some("test-client-secret") {
        return Err("invalid_client".into());
    }
    match form.get("grant_type").map(String::as_str) {
        Some("authorization_code") => {
            let code = form.get("code").ok_or("no code")?;
            let verifier = form.get("code_verifier").ok_or("no verifier")?;
            let entry = s.codes.get_mut(code).ok_or("unknown code")?;
            if entry.1 {
                return Err("code already used".into());
            }
            let digest = sha2::Sha256::digest(verifier.as_bytes());
            let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
            if entry.0 != expect {
                return Err("pkce mismatch".into());
            }
            entry.1 = true;
        }
        Some("refresh_token") => {
            let old = form.get("refresh_token").ok_or("no refresh_token")?;
            if !s.refresh.remove(old) {
                return Err("unknown refresh_token".into());
            }
            s.refreshes += 1;
        }
        // No refresh token for these two: a new token is requested instead.
        Some("client_credentials") => return Ok(plain_token(&mut s)),
        Some("password") => {
            if form.get("username").map(String::as_str) != Some("ana")
                || form.get("password").map(String::as_str) != Some("test-password")
            {
                return Err("invalid_grant".into());
            }
            return Ok(plain_token(&mut s));
        }
        Some("urn:ietf:params:oauth:grant-type:device_code") => {
            let code = form.get("device_code").ok_or("no device_code")?;
            let polls = s.devices.get_mut(code).ok_or("expired_token")?;
            *polls += 1;
            if *polls == 1 {
                return Err("authorization_pending".into());
            }
            s.devices.remove(code);
            s.grants += 1;
        }
        g => return Err(format!("bad grant_type {g:?}")),
    }
    let id = s.new_id();
    let access = format!("tok-{id}");
    let refresh = format!("ref-{id}");
    s.access.insert(access.clone(), id);
    s.refresh.insert(refresh.clone());
    Ok(format!(
        "{{\"access_token\":\"{access}\",\"token_type\":\"Bearer\",\
          \"expires_in\":1,\"refresh_token\":\"{refresh}\"}}"
    ))
}

/// An access token without a refresh token (client_credentials, password).
fn plain_token(s: &mut State) -> String {
    s.grants += 1;
    let id = s.new_id();
    let access = format!("tok-{id}");
    s.access.insert(access.clone(), id);
    format!("{{\"access_token\":\"{access}\",\"token_type\":\"Bearer\",\"expires_in\":1}}")
}

/// A tiny JSON-RPC answer so `tools/call` works behind the auth wall.
fn mcp_answer(body: &str) -> String {
    let v: serde_json::Value = match serde_json::from_str(body) {
        Ok(v) => v,
        Err(_) => {
            return "{\"jsonrpc\":\"2.0\",\"id\":null,\"error\":{\"code\":-32700,\"message\":\"parse\"}}"
                .into()
        }
    };
    let id = v.get("id").cloned().unwrap_or(serde_json::Value::Null);
    let method = v.get("method").and_then(|m| m.as_str()).unwrap_or_default();
    let result = match method {
        "initialize" => serde_json::json!({
            "protocolVersion": "2025-06-18",
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "mcp-auth", "version": "1" }
        }),
        "tools/call" => serde_json::json!({
            "content": [ { "type": "text", "text": "ok behind auth" } ]
        }),
        _ => serde_json::json!({}),
    };
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": result }).to_string()
}

async fn respond(
    sock: &mut TcpStream,
    status: u16,
    ctype: &str,
    body: &str,
    extra_headers: &str,
) -> std::io::Result<()> {
    let reason = match status {
        200 => "OK",
        201 => "Created",
        302 => "Found",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\n{extra_headers}Content-Length: {}\r\nConnection: keep-alive\r\n\r\n",
        body.len()
    );
    sock.write_all(head.as_bytes()).await?;
    sock.write_all(body.as_bytes()).await
}

fn parse_query(q: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for pair in q.split('&').filter(|p| !p.is_empty()) {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        out.insert(urldecode(k), urldecode(v));
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn find(h: &[u8], pat: &[u8]) -> Option<usize> {
    h.windows(pat.len()).position(|w| w == pat)
}
