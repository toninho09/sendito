//! Turns a request template plus a variable scope into exactly what goes on the wire.
//!
//! Resolution is pure: file bodies are carried as paths and read by the sender.

use std::path::{Path, PathBuf};

use base64::Engine;
use percent_encoding::{AsciiSet, utf8_percent_encode};

use crate::model::*;
use crate::vars::{Rendered, Scope};

/// Characters percent-encoded in query parameter names and values: controls,
/// non-ASCII, space, the WHATWG query set (`"#<>'`), the characters that carry
/// meaning inside a query (`%&+=`) and the remaining unsafe ASCII (`[\]^`{|}`).
/// Other sub-delimiters such as `,;:/?@!$()*` are sent literally.
const QUERY: &AsciiSet = &percent_encoding::CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'\'')
    .add(b'<')
    .add(b'>')
    .add(b'%')
    .add(b'&')
    .add(b'+')
    .add(b'=')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// `application/x-www-form-urlencoded` byte serializer (WHATWG URL spec): `*-._`
/// and alphanumerics pass through, space becomes `+`, everything else is `%XX`.
pub fn form_urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'*' | b'-' | b'.' | b'_' => {
                out.push(b as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Percent-encodes a query component (RFC 3986 unreserved set kept).
pub fn query_encode(s: &str) -> String {
    utf8_percent_encode(s, QUERY).to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedBody {
    None,
    Bytes(Vec<u8>),
    File(PathBuf),
    Multipart(Vec<ResolvedPart>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedPart {
    pub name: String,
    pub data: PartData,
    pub content_type: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartData {
    Text(String),
    File { path: PathBuf, filename: String },
}

/// Where an outgoing header came from (shown in the UI next to auto headers).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeaderOrigin {
    User,
    Body,
    Auth,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedHttp {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub header_origins: Vec<HeaderOrigin>,
    pub body: ResolvedBody,
    pub timeout_ms: Option<u64>,
    pub follow_redirects: bool,
    pub verify_tls: bool,
    /// Unresolved variable names (left literally in the output).
    pub unresolved: Vec<String>,
    /// Notes about adjustments (e.g. a user Content-Type replaced for multipart).
    pub notes: Vec<String>,
}

impl ResolvedHttp {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
}

struct Ctx<'a> {
    scope: &'a Scope,
    unresolved: Vec<String>,
}

impl Ctx<'_> {
    fn r(&mut self, s: &str) -> String {
        let Rendered {
            text, unresolved, ..
        } = self.scope.render(s);
        for u in unresolved {
            if !self.unresolved.contains(&u) {
                self.unresolved.push(u);
            }
        }
        text
    }
}

/// Adds a scheme when the URL has none (`http://` like most API clients).
pub fn normalize_url(url: &str) -> String {
    let t = url.trim();
    if t.contains("://") || t.is_empty() {
        t.to_string()
    } else {
        format!("http://{t}")
    }
}

/// Appends encoded `name=value` pairs to `url`, before any `#fragment`.
pub fn append_query(url: &str, pairs: &[(String, String)]) -> String {
    if pairs.is_empty() {
        return url.to_string();
    }
    let (base, frag) = match url.find('#') {
        Some(i) => (&url[..i], &url[i..]),
        None => (url, ""),
    };
    let mut out = base.to_string();
    let q: Vec<String> = pairs
        .iter()
        .map(|(n, v)| format!("{}={}", query_encode(n), query_encode(v)))
        .collect();
    if out.contains('?') {
        if !out.ends_with('?') && !out.ends_with('&') {
            out.push('&');
        }
    } else {
        out.push('?');
    }
    out.push_str(&q.join("&"));
    out.push_str(frag);
    out
}

fn resolve_path(base_dir: &Path, p: &str) -> PathBuf {
    let expanded = if let Some(rest) = p.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(rest))
            .unwrap_or_else(|| PathBuf::from(p))
    } else {
        PathBuf::from(p)
    };
    if expanded.is_absolute() {
        expanded
    } else {
        base_dir.join(expanded)
    }
}

fn has_header(h: &[(String, String)], name: &str) -> bool {
    h.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
}

/// Header produced by the auth settings, if any (`None` for query API keys).
fn auth_header(ctx: &mut Ctx<'_>, auth: &Auth) -> Option<(String, String)> {
    match auth {
        Auth::None => None,
        Auth::Basic { username, password } => {
            let raw = format!("{}:{}", ctx.r(username), ctx.r(password));
            let enc = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
            Some(("Authorization".into(), format!("Basic {enc}")))
        }
        Auth::Bearer { token } => {
            Some(("Authorization".into(), format!("Bearer {}", ctx.r(token))))
        }
        Auth::ApiKey {
            key,
            value,
            location: ApiKeyLocation::Header,
        } => Some((ctx.r(key), ctx.r(value))),
        Auth::ApiKey { .. } | Auth::OAuth { .. } => None,
    }
}

/// Resolves an HTTP request. `base_dir` anchors relative file paths (the collection root).
pub fn resolve_http(req: &HttpRequest, scope: &Scope, base_dir: &Path) -> ResolvedHttp {
    let mut ctx = Ctx {
        scope,
        unresolved: Vec::new(),
    };
    let mut notes = Vec::new();
    let method = ctx.r(req.method.trim()).to_ascii_uppercase();
    let url = normalize_url(&ctx.r(&req.url));

    let mut query: Vec<(String, String)> = req
        .params
        .iter()
        .filter(|p| p.enabled && !p.name.is_empty())
        .map(|p| (ctx.r(&p.name), ctx.r(&p.value)))
        .collect();
    if let Auth::ApiKey {
        key,
        value,
        location: ApiKeyLocation::Query,
    } = &req.auth
    {
        query.push((ctx.r(key), ctx.r(value)));
    }
    let url = append_query(&url, &query);

    let mut headers: Vec<(String, String)> = Vec::new();
    let mut origins = Vec::new();
    for h in req
        .headers
        .iter()
        .filter(|h| h.enabled && !h.name.trim().is_empty())
    {
        headers.push((ctx.r(h.name.trim()), ctx.r(&h.value)));
        origins.push(HeaderOrigin::User);
    }

    let auto_ct =
        |headers: &mut Vec<(String, String)>, origins: &mut Vec<HeaderOrigin>, ct: &str| {
            if !ct.is_empty() && !has_header(headers, "content-type") {
                headers.push(("Content-Type".into(), ct.to_string()));
                origins.push(HeaderOrigin::Body);
            }
        };

    let body = match &req.body {
        Body::None => ResolvedBody::None,
        Body::Raw { content_type, text } => {
            let ct = ctx.r(content_type);
            auto_ct(&mut headers, &mut origins, &ct);
            ResolvedBody::Bytes(ctx.r(text).into_bytes())
        }
        Body::Json { text } => {
            auto_ct(&mut headers, &mut origins, "application/json");
            ResolvedBody::Bytes(ctx.r(text).into_bytes())
        }
        Body::Form { fields } => {
            auto_ct(
                &mut headers,
                &mut origins,
                "application/x-www-form-urlencoded",
            );
            let enc: Vec<String> = fields
                .iter()
                .filter(|f| f.enabled && !f.name.is_empty())
                .map(|f| {
                    format!(
                        "{}={}",
                        form_urlencode(&ctx.r(&f.name)),
                        form_urlencode(&ctx.r(&f.value))
                    )
                })
                .collect();
            ResolvedBody::Bytes(enc.join("&").into_bytes())
        }
        Body::File { path, content_type } => {
            let ct = ctx.r(content_type);
            let ct = if ct.is_empty() {
                "application/octet-stream".to_string()
            } else {
                ct
            };
            auto_ct(&mut headers, &mut origins, &ct);
            ResolvedBody::File(resolve_path(base_dir, &ctx.r(path)))
        }
        Body::Multipart { parts } => {
            if let Some(i) = headers
                .iter()
                .position(|(n, _)| n.eq_ignore_ascii_case("content-type"))
            {
                notes.push(format!(
                    "Content-Type header '{}' replaced by multipart/form-data with a generated boundary",
                    headers[i].1
                ));
                headers.remove(i);
                origins.remove(i);
            }
            let rp = parts
                .iter()
                .filter(|p| p.enabled && !p.name.is_empty())
                .map(|p| {
                    let ct = ctx.r(&p.content_type);
                    ResolvedPart {
                        name: ctx.r(&p.name),
                        data: match &p.kind {
                            PartKind::Text { value } => PartData::Text(ctx.r(value)),
                            PartKind::File { path, filename } => {
                                let path = resolve_path(base_dir, &ctx.r(path));
                                let filename = match filename {
                                    Some(f) if !f.is_empty() => ctx.r(f),
                                    _ => path
                                        .file_name()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_default(),
                                };
                                PartData::File { path, filename }
                            }
                        },
                        content_type: if ct.is_empty() { None } else { Some(ct) },
                    }
                })
                .collect();
            ResolvedBody::Multipart(rp)
        }
    };

    if let Some((name, value)) = auth_header(&mut ctx, &req.auth) {
        if has_header(&headers, &name) {
            notes.push(format!(
                "auth header '{name}' not added: the request already defines it"
            ));
        } else {
            headers.push((name, value));
            origins.push(HeaderOrigin::Auth);
        }
    }

    ResolvedHttp {
        method,
        url,
        headers,
        header_origins: origins,
        body,
        timeout_ms: req.options.timeout_ms,
        follow_redirects: req.options.follow_redirects,
        verify_tls: req.options.verify_tls,
        unresolved: ctx.unresolved,
        notes,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedGrpc {
    /// Endpoint URI with scheme (`http://` or `https://`).
    pub uri: String,
    pub tls: bool,
    pub verify_tls: bool,
    pub service: String,
    pub method: String,
    pub message: String,
    pub metadata: Vec<(String, String)>,
    pub deadline_ms: Option<u64>,
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMcp {
    pub server: String,
    pub tool: String,
    /// Argument JSON text as written, variables rendered.
    pub arguments: String,
    pub timeout_ms: Option<u64>,
    pub unresolved: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedWs {
    /// Handshake URL with scheme (`ws://` or `wss://`).
    pub url: String,
    pub tls: bool,
    pub verify_tls: bool,
    pub headers: Vec<(String, String)>,
    pub subprotocols: Vec<String>,
    /// Messages sent right after the handshake, as a JSON document as written.
    pub messages: String,
    pub timeout_ms: Option<u64>,
    pub deadline_ms: Option<u64>,
    pub cookies: Option<bool>,
    pub unresolved: Vec<String>,
}

/// Resolves an MCP request. The server name must match a `[mcp.servers]`
/// entry of the collection config (checked by the sender).
pub fn resolve_mcp(req: &McpRequest, scope: &Scope) -> ResolvedMcp {
    let mut ctx = Ctx {
        scope,
        unresolved: Vec::new(),
    };
    ResolvedMcp {
        server: ctx.r(req.server.trim()),
        tool: ctx.r(req.tool.trim()),
        arguments: ctx.r(&req.arguments),
        timeout_ms: req.timeout_ms,
        unresolved: ctx.unresolved,
    }
}

/// Resolves a gRPC request. A target scheme (`https://`/`http://`) overrides `tls`.
pub fn resolve_grpc(req: &GrpcRequest, scope: &Scope) -> ResolvedGrpc {
    let mut ctx = Ctx {
        scope,
        unresolved: Vec::new(),
    };
    let target = ctx.r(req.target.trim());
    let (tls, host) = if let Some(rest) = target.strip_prefix("https://") {
        (true, rest.to_string())
    } else if let Some(rest) = target.strip_prefix("http://") {
        (false, rest.to_string())
    } else if let Some(rest) = target.strip_prefix("grpcs://") {
        (true, rest.to_string())
    } else if let Some(rest) = target.strip_prefix("grpc://") {
        (false, rest.to_string())
    } else {
        (req.tls, target.clone())
    };
    let host = host.trim_end_matches('/').to_string();
    let uri = format!("{}://{}", if tls { "https" } else { "http" }, host);
    let mut metadata: Vec<(String, String)> = req
        .metadata
        .iter()
        .filter(|m| m.enabled && !m.name.trim().is_empty())
        .map(|m| (ctx.r(m.name.trim()).to_ascii_lowercase(), ctx.r(&m.value)))
        .collect();
    if let Some((n, v)) = auth_header(&mut ctx, &req.auth) {
        let n = n.to_ascii_lowercase();
        if !metadata.iter().any(|(k, _)| *k == n) {
            metadata.push((n, v));
        }
    }
    if let Auth::ApiKey {
        key,
        value,
        location: ApiKeyLocation::Query,
    } = &req.auth
    {
        let n = ctx.r(key).to_ascii_lowercase();
        if !metadata.iter().any(|(k, _)| *k == n) {
            let v = ctx.r(value);
            metadata.push((n, v));
        }
    }
    ResolvedGrpc {
        uri,
        tls,
        verify_tls: req.verify_tls,
        service: ctx.r(req.service.trim()),
        method: ctx.r(req.method.trim()),
        message: ctx.r(&req.message),
        metadata,
        deadline_ms: req.deadline_ms,
        unresolved: ctx.unresolved,
    }
}

/// Resolves a WebSocket request. A URL scheme (`wss://`/`https://`/`ws://`/
/// `http://`) overrides the `tls` option.
pub fn resolve_ws(req: &WebSocketRequest, scope: &Scope) -> ResolvedWs {
    let mut ctx = Ctx {
        scope,
        unresolved: Vec::new(),
    };
    let raw = ctx.r(req.url.trim());
    let (tls, rest) = if let Some(rest) = raw
        .strip_prefix("wss://")
        .or_else(|| raw.strip_prefix("https://"))
    {
        (true, rest)
    } else if let Some(rest) = raw
        .strip_prefix("ws://")
        .or_else(|| raw.strip_prefix("http://"))
    {
        (false, rest)
    } else {
        (req.options.tls, raw.as_str())
    };
    let mut url = format!(
        "{}://{}",
        if tls { "wss" } else { "ws" },
        rest.trim_start_matches('/')
    );
    if let Auth::ApiKey {
        key,
        value,
        location: ApiKeyLocation::Query,
    } = &req.auth
    {
        url = append_query(&url, &[(ctx.r(key), ctx.r(value))]);
    }
    let mut headers: Vec<(String, String)> = req
        .headers
        .iter()
        .filter(|h| h.enabled && !h.name.trim().is_empty())
        .map(|h| (ctx.r(h.name.trim()), ctx.r(&h.value)))
        .collect();
    if let Some((n, v)) = auth_header(&mut ctx, &req.auth)
        && !headers.iter().any(|(k, _)| k.eq_ignore_ascii_case(&n))
    {
        headers.push((n, v));
    }
    ResolvedWs {
        url,
        tls,
        verify_tls: req.options.verify_tls,
        headers,
        subprotocols: req
            .subprotocols
            .iter()
            .map(|p| ctx.r(p.trim()))
            .filter(|p| !p.is_empty())
            .collect(),
        messages: ctx.r(&req.messages),
        timeout_ms: req.options.timeout_ms,
        deadline_ms: req.options.deadline_ms,
        cookies: req.options.cookies,
        unresolved: ctx.unresolved,
    }
}

/// Unresolved variables of any request (for highlighting before send).
pub fn unresolved_vars(req: &Request, scope: &Scope) -> Vec<String> {
    match req {
        Request::Http(h) => resolve_http(h, scope, Path::new(".")).unresolved,
        Request::Grpc(g) => resolve_grpc(g, scope).unresolved,
        Request::Ws(w) => resolve_ws(w, scope).unresolved,
        Request::Mcp(m) => resolve_mcp(m, scope).unresolved,
    }
}

fn mask_kv(v: &[KeyValue], s: &Scope) -> Vec<KeyValue> {
    v.iter()
        .map(|kv| KeyValue {
            name: s.render_masked(&kv.name),
            value: s.render_masked(&kv.value),
            enabled: kv.enabled,
        })
        .collect()
}

fn mask_auth(a: &Auth, s: &Scope) -> Auth {
    match a {
        Auth::None => Auth::None,
        Auth::Basic { username, password } => Auth::Basic {
            username: s.render_masked(username),
            password: s.render_masked(password),
        },
        Auth::Bearer { token } => Auth::Bearer {
            token: s.render_masked(token),
        },
        Auth::ApiKey {
            key,
            value,
            location,
        } => Auth::ApiKey {
            key: s.render_masked(key),
            value: s.render_masked(value),
            location: *location,
        },
        Auth::OAuth { name } => Auth::OAuth {
            name: s.render_masked(name),
        },
    }
}

/// A copy of `req` with every non-secret variable substituted and every
/// secret one left as a `{{placeholder}}`. Safe to persist in history.
pub fn mask_request(req: &Request, s: &Scope) -> Request {
    match req {
        Request::Http(h) => {
            let body = match &h.body {
                Body::None => Body::None,
                Body::Raw { content_type, text } => Body::Raw {
                    content_type: s.render_masked(content_type),
                    text: s.render_masked(text),
                },
                Body::Json { text } => Body::Json {
                    text: s.render_masked(text),
                },
                Body::Form { fields } => Body::Form {
                    fields: mask_kv(fields, s),
                },
                Body::Multipart { parts } => Body::Multipart {
                    parts: parts
                        .iter()
                        .map(|p| MultipartPart {
                            name: s.render_masked(&p.name),
                            kind: match &p.kind {
                                PartKind::Text { value } => PartKind::Text {
                                    value: s.render_masked(value),
                                },
                                PartKind::File { path, filename } => PartKind::File {
                                    path: s.render_masked(path),
                                    filename: filename.as_ref().map(|f| s.render_masked(f)),
                                },
                            },
                            content_type: s.render_masked(&p.content_type),
                            enabled: p.enabled,
                        })
                        .collect(),
                },
                Body::File { path, content_type } => Body::File {
                    path: s.render_masked(path),
                    content_type: s.render_masked(content_type),
                },
            };
            Request::Http(HttpRequest {
                method: s.render_masked(&h.method),
                url: s.render_masked(&h.url),
                params: mask_kv(&h.params, s),
                headers: mask_kv(&h.headers, s),
                body,
                auth: mask_auth(&h.auth, s),
                options: h.options.clone(),
                scripts: h.scripts.clone(),
            })
        }
        Request::Grpc(g) => Request::Grpc(GrpcRequest {
            target: s.render_masked(&g.target),
            service: g.service.clone(),
            method: g.method.clone(),
            message: s.render_masked(&g.message),
            metadata: mask_kv(&g.metadata, s),
            auth: mask_auth(&g.auth, s),
            tls: g.tls,
            verify_tls: g.verify_tls,
            deadline_ms: g.deadline_ms,
            schema: g.schema.clone(),
            scripts: g.scripts.clone(),
        }),
        Request::Ws(w) => Request::Ws(WebSocketRequest {
            url: s.render_masked(&w.url),
            headers: mask_kv(&w.headers, s),
            subprotocols: w.subprotocols.iter().map(|p| s.render_masked(p)).collect(),
            messages: s.render_masked(&w.messages),
            auth: mask_auth(&w.auth, s),
            options: w.options.clone(),
            scripts: w.scripts.clone(),
        }),
        Request::Mcp(m) => Request::Mcp(McpRequest {
            server: s.render_masked(&m.server),
            tool: m.tool.clone(),
            arguments: s.render_masked(&m.arguments),
            timeout_ms: m.timeout_ms,
            scripts: m.scripts.clone(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vars::VarValue;
    use std::collections::BTreeMap;

    fn scope() -> Scope {
        let mut m = BTreeMap::new();
        m.insert("base".into(), VarValue::plain("http://localhost:8080"));
        m.insert("tok".into(), VarValue::secret("abc"));
        m.insert("user".into(), VarValue::plain("ana"));
        Scope::new().with_layer("env", m)
    }

    #[test]
    fn encodes_query() {
        assert_eq!(
            query_encode("a b&c=d/é~,:+%"),
            "a%20b%26c%3Dd/%C3%A9~,:%2B%25"
        );
        assert_eq!(form_urlencode("a b&c=d*é"), "a+b%26c%3Dd*%C3%A9");
        assert_eq!(
            append_query("http://x/p?a=1#f", &[("b".into(), "2 3".into())]),
            "http://x/p?a=1&b=2%203#f"
        );
        assert_eq!(
            append_query("http://x/?", &[("b".into(), "".into())]),
            "http://x/?b="
        );
        assert_eq!(append_query("http://x", &[]), "http://x");
        assert_eq!(normalize_url(" localhost:1/x "), "http://localhost:1/x");
        assert_eq!(normalize_url("https://a"), "https://a");
    }

    #[test]
    fn http_full() {
        let req = HttpRequest {
            method: "post".into(),
            url: "{{base}}/users".into(),
            params: vec![
                KeyValue::new("q", "café"),
                KeyValue::new("off", "x").disabled(),
            ],
            headers: vec![
                KeyValue::new("X-Dup", "1"),
                KeyValue::new("X-Dup", "2"),
                KeyValue::new("X-Miss", "{{missing}}"),
            ],
            body: Body::Json {
                text: "{\"u\":\"{{user}}\"}".into(),
            },
            auth: Auth::Bearer {
                token: "{{tok}}".into(),
            },
            ..Default::default()
        };
        let r = resolve_http(&req, &scope(), Path::new("/c"));
        assert_eq!(r.method, "POST");
        assert_eq!(r.url, "http://localhost:8080/users?q=caf%C3%A9");
        assert_eq!(
            r.headers,
            vec![
                ("X-Dup".to_string(), "1".to_string()),
                ("X-Dup".to_string(), "2".to_string()),
                ("X-Miss".to_string(), "{{missing}}".to_string()),
                ("Content-Type".to_string(), "application/json".to_string()),
                ("Authorization".to_string(), "Bearer abc".to_string()),
            ]
        );
        assert_eq!(r.header_origins.len(), r.headers.len());
        assert_eq!(r.body, ResolvedBody::Bytes(b"{\"u\":\"ana\"}".to_vec()));
        assert_eq!(r.unresolved, vec!["missing".to_string()]);
        assert_eq!(r.header("content-type"), Some("application/json"));
    }

    #[test]
    fn auth_variants() {
        let s = scope();
        let mut req = HttpRequest {
            url: "x".into(),
            auth: Auth::Basic {
                username: "{{user}}".into(),
                password: "{{tok}}".into(),
            },
            ..Default::default()
        };
        let r = resolve_http(&req, &s, Path::new("."));
        assert_eq!(r.header("authorization"), Some("Basic YW5hOmFiYw=="));
        req.auth = Auth::ApiKey {
            key: "api_key".into(),
            value: "{{tok}}".into(),
            location: ApiKeyLocation::Query,
        };
        let r = resolve_http(&req, &s, Path::new("."));
        assert_eq!(r.url, "http://x?api_key=abc");
        req.auth = Auth::ApiKey {
            key: "X-Key".into(),
            value: "v".into(),
            location: ApiKeyLocation::Header,
        };
        req.headers.push(KeyValue::new("x-key", "mine"));
        let r = resolve_http(&req, &s, Path::new("."));
        assert_eq!(r.header("x-key"), Some("mine"));
        assert_eq!(r.notes.len(), 1);
    }

    #[test]
    fn bodies() {
        let s = scope();
        let mut req = HttpRequest {
            url: "x".into(),
            body: Body::Form {
                fields: vec![
                    KeyValue::new("a b", "c&d"),
                    KeyValue::new("z", "1").disabled(),
                ],
            },
            ..Default::default()
        };
        let r = resolve_http(&req, &s, Path::new("."));
        assert_eq!(r.body, ResolvedBody::Bytes(b"a+b=c%26d".to_vec()));
        assert_eq!(
            r.header("content-type"),
            Some("application/x-www-form-urlencoded")
        );

        req.body = Body::Raw {
            content_type: "text/xml".into(),
            text: "<a/>".into(),
        };
        req.headers = vec![KeyValue::new("content-type", "application/custom")];
        let r = resolve_http(&req, &s, Path::new("."));
        assert_eq!(r.header("content-type"), Some("application/custom"));
        assert_eq!(r.headers.len(), 1);

        req.headers.clear();
        req.body = Body::File {
            path: "data/x.bin".into(),
            content_type: String::new(),
        };
        let r = resolve_http(&req, &s, Path::new("/col"));
        assert_eq!(r.body, ResolvedBody::File(PathBuf::from("/col/data/x.bin")));
        assert_eq!(r.header("content-type"), Some("application/octet-stream"));

        req.headers = vec![KeyValue::new("Content-Type", "multipart/form-data")];
        req.body = Body::Multipart {
            parts: vec![
                MultipartPart {
                    name: "t".into(),
                    kind: PartKind::Text {
                        value: "{{user}}".into(),
                    },
                    content_type: String::new(),
                    enabled: true,
                },
                MultipartPart {
                    name: "f".into(),
                    kind: PartKind::File {
                        path: "/abs/a.png".into(),
                        filename: None,
                    },
                    content_type: "image/png".into(),
                    enabled: true,
                },
                MultipartPart {
                    name: "g".into(),
                    kind: PartKind::File {
                        path: "b.txt".into(),
                        filename: Some("renamed.txt".into()),
                    },
                    content_type: String::new(),
                    enabled: true,
                },
            ],
        };
        let r = resolve_http(&req, &s, Path::new("/col"));
        assert!(r.headers.is_empty());
        assert_eq!(r.notes.len(), 1);
        let ResolvedBody::Multipart(parts) = r.body else {
            panic!("expected multipart")
        };
        assert_eq!(parts[0].data, PartData::Text("ana".into()));
        assert_eq!(
            parts[1].data,
            PartData::File {
                path: "/abs/a.png".into(),
                filename: "a.png".into()
            }
        );
        assert_eq!(parts[1].content_type.as_deref(), Some("image/png"));
        assert_eq!(
            parts[2].data,
            PartData::File {
                path: "/col/b.txt".into(),
                filename: "renamed.txt".into()
            }
        );
    }

    #[test]
    fn grpc_resolution() {
        let s = scope();
        let g = GrpcRequest {
            target: "https://api:443/".into(),
            service: "a.B".into(),
            method: "C".into(),
            message: "{\"u\":\"{{user}}\"}".into(),
            metadata: vec![
                KeyValue::new("X-Trace", "1"),
                KeyValue::new("off", "1").disabled(),
            ],
            auth: Auth::Bearer {
                token: "{{tok}}".into(),
            },
            ..Default::default()
        };
        let r = resolve_grpc(&g, &s);
        assert!(r.tls);
        assert_eq!(r.uri, "https://api:443");
        assert_eq!(r.message, "{\"u\":\"ana\"}");
        assert_eq!(
            r.metadata,
            vec![
                ("x-trace".to_string(), "1".to_string()),
                ("authorization".to_string(), "Bearer abc".to_string())
            ]
        );
        let mut g2 = g.clone();
        g2.target = "localhost:50051".into();
        g2.auth = Auth::ApiKey {
            key: "X-Key".into(),
            value: "k".into(),
            location: ApiKeyLocation::Query,
        };
        let r = resolve_grpc(&g2, &s);
        assert!(!r.tls);
        assert_eq!(r.uri, "http://localhost:50051");
        assert!(r.metadata.contains(&("x-key".to_string(), "k".to_string())));
        for (t, tls) in [
            ("grpcs://h:1", true),
            ("grpc://h:1", false),
            ("http://h:1", false),
        ] {
            g2.target = t.into();
            assert_eq!(resolve_grpc(&g2, &s).tls, tls);
        }
        assert!(unresolved_vars(&Request::Grpc(g2), &s).is_empty());
    }

    #[test]
    fn ws_resolution() {
        let s = scope();
        let w = WebSocketRequest {
            url: "wss://{{user}}.example/chat".into(),
            headers: vec![
                KeyValue::new("origin", "https://app"),
                KeyValue::new("off", "1").disabled(),
            ],
            subprotocols: vec!["chat.v2".into(), " chat.v1 ".into(), "  ".into()],
            messages: r#"["{{user}}", {"binary": "AAEC"}]"#.into(),
            auth: Auth::Bearer {
                token: "{{tok}}".into(),
            },
            options: WsOptions {
                tls: false,
                deadline_ms: Some(50),
                ..Default::default()
            },
            ..Default::default()
        };
        let r = resolve_ws(&w, &s);
        assert_eq!(r.url, "wss://ana.example/chat");
        assert!(r.tls);
        assert_eq!(
            r.headers,
            vec![
                ("origin".to_string(), "https://app".to_string()),
                ("Authorization".to_string(), "Bearer abc".to_string()),
            ]
        );
        assert_eq!(r.subprotocols, vec!["chat.v2", "chat.v1"]);
        // The opening messages render their variables, like the gRPC message.
        assert_eq!(r.messages, r#"["ana", {"binary": "AAEC"}]"#);
        assert!(r.unresolved.is_empty());
        // An undefined variable in a message is reported before sending.
        let mut w2 = w.clone();
        w2.messages = r#"["{{missing}}"]"#.into();
        assert_eq!(
            unresolved_vars(&Request::Ws(w2.clone()), &s),
            vec!["missing"]
        );
        // A URL without a scheme follows `tls`; http/https mean ws/wss.
        let mut w3 = w2.clone();
        w3.url = "host/x".into();
        w3.options.tls = true;
        assert_eq!(resolve_ws(&w3, &s).url, "wss://host/x");
        for (raw, tls) in [
            ("https://h/p", true),
            ("http://h/p", false),
            ("wss://h/p", true),
            ("ws://h/p", false),
        ] {
            w3.url = raw.into();
            assert_eq!(resolve_ws(&w3, &s).tls, tls, "{raw}");
        }
        // An API key in the query is appended to the resolved URL.
        let mut w4 = w.clone();
        w4.auth = Auth::ApiKey {
            key: "k".into(),
            value: "{{tok}}".into(),
            location: ApiKeyLocation::Query,
        };
        assert!(resolve_ws(&w4, &s).url.ends_with("/chat?k=abc"));
    }

    #[test]
    fn masking() {
        let s = scope();
        let req = Request::Http(HttpRequest {
            url: "{{base}}/x?t={{tok}}".into(),
            headers: vec![KeyValue::new("A", "{{tok}}")],
            auth: Auth::Basic {
                username: "{{user}}".into(),
                password: "{{tok}}".into(),
            },
            body: Body::Multipart {
                parts: vec![MultipartPart {
                    name: "a".into(),
                    kind: PartKind::Text {
                        value: "{{tok}}".into(),
                    },
                    content_type: String::new(),
                    enabled: true,
                }],
            },
            ..Default::default()
        });
        let m = mask_request(&req, &s);
        let Request::Http(h) = &m else { panic!() };
        assert_eq!(h.url, "http://localhost:8080/x?t={{tok}}");
        assert_eq!(h.headers[0].value, "{{tok}}");
        assert_eq!(
            h.auth,
            Auth::Basic {
                username: "ana".into(),
                password: "{{tok}}".into()
            }
        );
        let g = Request::Grpc(GrpcRequest {
            message: "{{tok}}{{user}}".into(),
            auth: Auth::ApiKey {
                key: "k".into(),
                value: "{{tok}}".into(),
                location: ApiKeyLocation::Header,
            },
            ..Default::default()
        });
        let Request::Grpc(gm) = mask_request(&g, &s) else {
            panic!()
        };
        assert_eq!(gm.message, "{{tok}}ana");
        for body in [
            Body::None,
            Body::Raw {
                content_type: "t".into(),
                text: "{{tok}}".into(),
            },
            Body::Json {
                text: "{{user}}".into(),
            },
            Body::Form {
                fields: vec![KeyValue::new("a", "{{tok}}")],
            },
            Body::File {
                path: "{{user}}".into(),
                content_type: String::new(),
            },
        ] {
            let r = Request::Http(HttpRequest {
                body,
                ..Default::default()
            });
            let m = mask_request(&r, &s);
            assert!(!format!("{m:?}").contains("abc"));
        }
        let b = Request::Http(HttpRequest {
            auth: Auth::Bearer {
                token: "{{tok}}".into(),
            },
            ..Default::default()
        });
        assert!(!format!("{:?}", mask_request(&b, &s)).contains("abc"));
    }
}
