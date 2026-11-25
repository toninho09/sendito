//! The request model. These types mirror the on-disk TOML schema (see FORMAT.md)
//! but carry no serialization logic: `sendito-store` owns the file format.

/// A saved request: HTTP, gRPC, WebSocket, SSE, MQTT or MCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Http(HttpRequest),
    Grpc(GrpcRequest),
    Ws(WebSocketRequest),
    Sse(SseRequest),
    Mqtt(MqttRequest),
    Mcp(McpRequest),
}

/// The auth MCP requests answer with: authentication belongs to the server
/// definition in the config, not to the request.
static NO_AUTH: Auth = Auth::None;

impl Request {
    /// Short label used for tree badges (`GET`, `POST`, `GRPC`, `WS`, `MCP`).
    pub fn badge(&self) -> &str {
        match self {
            Request::Http(h) => &h.method,
            Request::Grpc(_) => "GRPC",
            Request::Ws(_) => "WS",
            Request::Sse(_) => "SSE",
            Request::Mqtt(_) => "MQTT",
            Request::Mcp(_) => "MCP",
        }
    }

    /// The fields every request kind has.
    pub fn common(&self) -> &RequestCommon {
        match self {
            Request::Http(h) => &h.common,
            Request::Grpc(g) => &g.common,
            Request::Ws(w) => &w.common,
            Request::Sse(s) => &s.common,
            Request::Mqtt(m) => &m.common,
            Request::Mcp(m) => &m.common,
        }
    }

    pub fn common_mut(&mut self) -> &mut RequestCommon {
        match self {
            Request::Http(h) => &mut h.common,
            Request::Grpc(g) => &mut g.common,
            Request::Ws(w) => &mut w.common,
            Request::Sse(s) => &mut s.common,
            Request::Mqtt(m) => &mut m.common,
            Request::Mcp(m) => &mut m.common,
        }
    }

    pub fn scripts(&self) -> &Scripts {
        &self.common().scripts
    }

    pub fn scripts_mut(&mut self) -> &mut Scripts {
        &mut self.common_mut().scripts
    }

    /// The Markdown description of the request.
    pub fn description(&self) -> &str {
        &self.common().description
    }

    pub fn auth(&self) -> &Auth {
        match self {
            Request::Http(h) => &h.auth,
            Request::Grpc(g) => &g.auth,
            Request::Ws(w) => &w.auth,
            Request::Sse(s) => &s.auth,
            Request::Mqtt(m) => &m.auth,
            Request::Mcp(_) => &NO_AUTH,
        }
    }

    /// The auth of an HTTP, gRPC, SSE, MQTT or WebSocket request; `None` for MCP, whose
    /// auth lives on the server definition in the config.
    pub fn auth_mut(&mut self) -> Option<&mut Auth> {
        match self {
            Request::Http(h) => Some(&mut h.auth),
            Request::Grpc(g) => Some(&mut g.auth),
            Request::Ws(w) => Some(&mut w.auth),
            Request::Sse(s) => Some(&mut s.auth),
            Request::Mqtt(m) => Some(&mut m.auth),
            Request::Mcp(_) => None,
        }
    }

    pub fn new_http() -> Self {
        Request::Http(HttpRequest::default())
    }

    pub fn new_grpc() -> Self {
        Request::Grpc(GrpcRequest::default())
    }

    pub fn new_ws() -> Self {
        Request::Ws(WebSocketRequest::default())
    }

    pub fn new_sse() -> Self {
        Request::Sse(SseRequest::default())
    }

    pub fn new_mqtt() -> Self {
        Request::Mqtt(MqttRequest::default())
    }

    pub fn new_mcp() -> Self {
        Request::Mcp(McpRequest::default())
    }
}

/// The fields every request kind has. A field that all kinds share goes
/// here, so the model, the file format and the editors read it in one place.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequestCommon {
    pub scripts: Scripts,
    /// Markdown notes for people; never sent on the wire.
    pub description: String,
}

/// A name/value row with an enable toggle (query params, headers, form fields, metadata).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyValue {
    pub name: String,
    pub value: String,
    pub enabled: bool,
}

impl KeyValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }

    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

impl Default for KeyValue {
    fn default() -> Self {
        Self::new("", "")
    }
}

/// Every HTTP method Sendito offers in the method picker. Any other token is
/// also accepted (custom methods are sent verbatim).
pub const HTTP_METHODS: &[&str] = &[
    "GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS", "TRACE", "CONNECT",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    pub params: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub body: Body,
    pub auth: Auth,
    pub options: HttpOptions,
    /// The fields every request kind has.
    pub common: RequestCommon,
}

impl Default for HttpRequest {
    fn default() -> Self {
        Self {
            method: "GET".into(),
            url: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            body: Body::None,
            auth: Auth::Inherit,
            options: HttpOptions::default(),
            common: RequestCommon::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Body {
    #[default]
    None,
    /// Raw text sent as-is. `content_type` is added as a `Content-Type` header
    /// unless the request already defines one.
    Raw { content_type: String, text: String },
    /// JSON text sent as-is with `Content-Type: application/json` (unless overridden).
    Json { text: String },
    /// `application/x-www-form-urlencoded` fields.
    Form { fields: Vec<KeyValue> },
    /// `multipart/form-data` parts.
    Multipart { parts: Vec<MultipartPart> },
    /// The bytes of a file, sent verbatim.
    File { path: String, content_type: String },
    /// A GraphQL operation. Sent as one JSON object: `query`, `variables`
    /// (omitted when empty) and `operationName` (omitted when empty).
    GraphQL {
        query: String,
        /// A JSON object, or empty.
        variables: String,
        operation_name: String,
    },
}

impl Body {
    pub fn kind(&self) -> BodyKind {
        match self {
            Body::None => BodyKind::None,
            Body::Raw { .. } => BodyKind::Raw,
            Body::Json { .. } => BodyKind::Json,
            Body::Form { .. } => BodyKind::Form,
            Body::Multipart { .. } => BodyKind::Multipart,
            Body::File { .. } => BodyKind::File,
            Body::GraphQL { .. } => BodyKind::GraphQL,
        }
    }

    /// Converts the body to another kind, carrying over text when it makes sense.
    pub fn convert(&self, kind: BodyKind) -> Body {
        let text = match self {
            Body::Raw { text, .. } | Body::Json { text } => text.clone(),
            Body::GraphQL { .. } => graphql_document(self),
            _ => String::new(),
        };
        match kind {
            BodyKind::None => Body::None,
            BodyKind::Raw => Body::Raw {
                content_type: "text/plain".into(),
                text,
            },
            BodyKind::Json => Body::Json { text },
            BodyKind::GraphQL => match self {
                Body::GraphQL { .. } => self.clone(),
                Body::Raw { .. } | Body::Json { .. } => graphql_body(&text),
                _ => graphql_body(""),
            },
            BodyKind::Form => match self {
                Body::Form { fields } => Body::Form {
                    fields: fields.clone(),
                },
                _ => Body::Form { fields: Vec::new() },
            },
            BodyKind::Multipart => match self {
                Body::Multipart { parts } => Body::Multipart {
                    parts: parts.clone(),
                },
                _ => Body::Multipart { parts: Vec::new() },
            },
            BodyKind::File => match self {
                Body::File { .. } => self.clone(),
                _ => Body::File {
                    path: String::new(),
                    content_type: String::new(),
                },
            },
        }
    }
}

/// A GraphQL body from JSON or raw text: a JSON object with a string `query`
/// is split into its parts (`variables` compact, `operationName`); any other
/// text becomes the query as a whole.
pub fn graphql_body(text: &str) -> Body {
    let object = serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| match v {
            serde_json::Value::Object(o) => Some(o),
            _ => None,
        })
        .filter(|o| o.get("query").is_some_and(|q| q.is_string()));
    match object {
        Some(o) => Body::GraphQL {
            query: o["query"].as_str().unwrap_or_default().to_string(),
            variables: match o.get("variables") {
                Some(v) => v.to_string(),
                None => String::new(),
            },
            operation_name: o
                .get("operationName")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        None => Body::GraphQL {
            query: text.to_string(),
            variables: String::new(),
            operation_name: String::new(),
        },
    }
}

/// The JSON document a GraphQL body is sent as: `query`, then `variables`
/// when not empty (it must be a JSON object), then `operationName` when not
/// empty. Variables and the operation name are already rendered.
pub fn graphql_document_json(
    query: &str,
    variables: &str,
    operation_name: &str,
) -> Result<String, String> {
    let mut o = serde_json::Map::new();
    o.insert("query".into(), serde_json::Value::String(query.to_string()));
    if !variables.trim().is_empty() {
        let v: serde_json::Value = serde_json::from_str(variables)
            .map_err(|e| format!("variables must be a JSON object: {e}"))?;
        if !v.is_object() {
            return Err("variables must be a JSON object".into());
        }
        o.insert("variables".into(), v);
    }
    if !operation_name.is_empty() {
        o.insert(
            "operationName".into(),
            serde_json::Value::String(operation_name.to_string()),
        );
    }
    serde_json::to_string(&serde_json::Value::Object(o)).map_err(|e| e.to_string())
}

/// The JSON document of a GraphQL body, for converting it to a JSON or raw
/// body. Variables that do not parse are kept as a string.
fn graphql_document(body: &Body) -> String {
    let Body::GraphQL {
        query,
        variables,
        operation_name,
    } = body
    else {
        return String::new();
    };
    graphql_document_json(query, variables, operation_name).unwrap_or_else(|_| {
        let mut o = serde_json::Map::new();
        o.insert("query".into(), serde_json::Value::String(query.clone()));
        if !variables.trim().is_empty() {
            o.insert(
                "variables".into(),
                serde_json::Value::String(variables.clone()),
            );
        }
        if !operation_name.trim().is_empty() {
            o.insert(
                "operationName".into(),
                serde_json::Value::String(operation_name.clone()),
            );
        }
        serde_json::Value::Object(o).to_string()
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    None,
    Raw,
    Json,
    Form,
    Multipart,
    File,
    GraphQL,
}

impl BodyKind {
    pub const ALL: [BodyKind; 7] = [
        BodyKind::None,
        BodyKind::Json,
        BodyKind::Raw,
        BodyKind::Form,
        BodyKind::Multipart,
        BodyKind::File,
        BodyKind::GraphQL,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BodyKind::None => "none",
            BodyKind::Raw => "raw",
            BodyKind::Json => "json",
            BodyKind::Form => "form",
            BodyKind::Multipart => "multipart",
            BodyKind::File => "file",
            BodyKind::GraphQL => "graphql",
        }
    }

    /// The Content-Type this kind adds when no header sets one. Raw and File
    /// take theirs from their own field; multipart (with its boundary) always
    /// replaces a header.
    pub fn content_type(self) -> Option<&'static str> {
        match self {
            BodyKind::Json | BodyKind::GraphQL => Some("application/json"),
            BodyKind::Form => Some("application/x-www-form-urlencoded"),
            BodyKind::Multipart => Some("multipart/form-data"),
            _ => None,
        }
    }

    /// A short name for the body type chips.
    pub fn short_label(self) -> &'static str {
        match self {
            BodyKind::Form => "Form",
            BodyKind::GraphQL => "GraphQL",
            k => k.label(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BodyKind::None => "None",
            BodyKind::Raw => "Raw",
            BodyKind::Json => "JSON",
            BodyKind::Form => "Form URL-encoded",
            BodyKind::Multipart => "Multipart",
            BodyKind::File => "File",
            BodyKind::GraphQL => "GraphQL",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipartPart {
    pub name: String,
    pub kind: PartKind,
    /// Explicit part content type. Empty means: none for text parts, guessed from
    /// the file extension for file parts.
    pub content_type: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartKind {
    Text {
        value: String,
    },
    File {
        path: String,
        filename: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Auth {
    /// Use the auth of the nearest folder that defines one (`folder.toml`);
    /// no auth when no folder does. A request file without `[auth]` inherits.
    #[default]
    Inherit,
    None,
    Basic {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
    },
    ApiKey {
        key: String,
        value: String,
        location: ApiKeyLocation,
    },
    /// A named HTTP OAuth identity from the collection configuration.
    OAuth {
        name: String,
    },
    /// HTTP Digest (RFC 7616): the first send has no `Authorization`; a `401`
    /// with a Digest challenge is answered once. HTTP only.
    Digest {
        username: String,
        password: String,
    },
    /// AWS Signature Version 4, computed over the request as it is sent.
    /// HTTP only.
    AwsSigV4 {
        access_key: String,
        secret_key: String,
        /// Temporary credentials only; empty sends no `X-Amz-Security-Token`.
        session_token: String,
        region: String,
        service: String,
    },
    /// A JWT signed at send time and sent as `Authorization: Bearer`.
    Jwt {
        algorithm: JwtAlgorithm,
        /// The HMAC secret (HS*).
        secret: String,
        /// A PEM private key file for RS256 / ES256, relative to the collection.
        private_key: String,
        /// The claims, JSON text.
        claims: String,
        /// Adds `iat` (now) and `exp` (now + this) to the claims.
        expires_in_s: Option<u64>,
    },
}

/// The signing algorithms of [`Auth::Jwt`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum JwtAlgorithm {
    #[default]
    Hs256,
    Hs384,
    Hs512,
    Rs256,
    Es256,
}

impl JwtAlgorithm {
    pub const ALL: [JwtAlgorithm; 5] = [
        JwtAlgorithm::Hs256,
        JwtAlgorithm::Hs384,
        JwtAlgorithm::Hs512,
        JwtAlgorithm::Rs256,
        JwtAlgorithm::Es256,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            JwtAlgorithm::Hs256 => "HS256",
            JwtAlgorithm::Hs384 => "HS384",
            JwtAlgorithm::Hs512 => "HS512",
            JwtAlgorithm::Rs256 => "RS256",
            JwtAlgorithm::Es256 => "ES256",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|a| a.as_str().eq_ignore_ascii_case(s))
    }

    /// Signed with a shared secret (HS*), not a private key file.
    pub fn uses_secret(self) -> bool {
        matches!(
            self,
            JwtAlgorithm::Hs256 | JwtAlgorithm::Hs384 | JwtAlgorithm::Hs512
        )
    }
}

impl Auth {
    pub fn kind(&self) -> AuthKind {
        match self {
            Auth::Inherit => AuthKind::Inherit,
            Auth::None => AuthKind::None,
            Auth::Basic { .. } => AuthKind::Basic,
            Auth::Bearer { .. } => AuthKind::Bearer,
            Auth::ApiKey { .. } => AuthKind::ApiKey,
            Auth::OAuth { .. } => AuthKind::OAuth,
            Auth::Digest { .. } => AuthKind::Digest,
            Auth::AwsSigV4 { .. } => AuthKind::AwsSigV4,
            Auth::Jwt { .. } => AuthKind::Jwt,
        }
    }

    pub fn with_kind(kind: AuthKind) -> Auth {
        match kind {
            AuthKind::Inherit => Auth::Inherit,
            AuthKind::None => Auth::None,
            AuthKind::Basic => Auth::Basic {
                username: String::new(),
                password: String::new(),
            },
            AuthKind::Bearer => Auth::Bearer {
                token: String::new(),
            },
            AuthKind::ApiKey => Auth::ApiKey {
                key: "X-API-Key".into(),
                value: String::new(),
                location: ApiKeyLocation::Header,
            },
            AuthKind::OAuth => Auth::OAuth {
                name: String::new(),
            },
            AuthKind::Digest => Auth::Digest {
                username: String::new(),
                password: String::new(),
            },
            AuthKind::AwsSigV4 => Auth::AwsSigV4 {
                access_key: String::new(),
                secret_key: String::new(),
                session_token: String::new(),
                region: "us-east-1".into(),
                service: String::new(),
            },
            AuthKind::Jwt => Auth::Jwt {
                algorithm: JwtAlgorithm::Hs256,
                secret: String::new(),
                private_key: String::new(),
                claims: "{}".into(),
                expires_in_s: None,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    Inherit,
    None,
    Basic,
    Bearer,
    ApiKey,
    Jwt,
    OAuth,
    Digest,
    AwsSigV4,
}

impl AuthKind {
    /// Every kind; the HTTP-only ones ([`AuthKind::http_only`]) come last.
    pub const ALL: [AuthKind; 9] = [
        AuthKind::Inherit,
        AuthKind::None,
        AuthKind::Basic,
        AuthKind::Bearer,
        AuthKind::ApiKey,
        AuthKind::Jwt,
        AuthKind::OAuth,
        AuthKind::Digest,
        AuthKind::AwsSigV4,
    ];

    /// Kinds that only HTTP requests can send: a gRPC call or a WebSocket
    /// session with one of them fails with a message.
    pub fn http_only(self) -> bool {
        matches!(
            self,
            AuthKind::OAuth | AuthKind::Digest | AuthKind::AwsSigV4
        )
    }

    pub fn as_str(self) -> &'static str {
        match self {
            AuthKind::Inherit => "inherit",
            AuthKind::None => "none",
            AuthKind::Basic => "basic",
            AuthKind::Bearer => "bearer",
            AuthKind::ApiKey => "apikey",
            AuthKind::OAuth => "oauth",
            AuthKind::Digest => "digest",
            AuthKind::AwsSigV4 => "awsv4",
            AuthKind::Jwt => "jwt",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AuthKind::Inherit => "Inherit",
            AuthKind::None => "No auth",
            AuthKind::Basic => "Basic",
            AuthKind::Bearer => "Bearer token",
            AuthKind::ApiKey => "API key",
            AuthKind::OAuth => "OAuth 2",
            AuthKind::Digest => "Digest",
            AuthKind::AwsSigV4 => "AWS Signature v4",
            AuthKind::Jwt => "JWT (signed here)",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ApiKeyLocation {
    #[default]
    Header,
    Query,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpOptions {
    /// Total request timeout. `None` uses the configured default.
    pub timeout_ms: Option<u64>,
    pub follow_redirects: bool,
    pub verify_tls: bool,
    /// Replay cookies from the collection's cookie jar and store the ones the
    /// response sets. `None` follows the `[http] cookie_jar` config.
    pub cookies: Option<bool>,
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            timeout_ms: None,
            follow_redirects: true,
            verify_tls: true,
            cookies: None,
        }
    }
}

/// What a folder gives the requests below it (`folder.toml`): headers, auth,
/// variables and scripts. Closer folders win; see `crate::inherit`. The
/// description documents the folder only; requests do not inherit it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FolderSettings {
    pub headers: Vec<KeyValue>,
    pub auth: Auth,
    /// Plain values; a value may reference other variables (`{{secret}}`).
    pub vars: Vec<KeyValue>,
    pub scripts: Scripts,
    /// Markdown notes for people; never sent on the wire.
    pub description: String,
}

impl FolderSettings {
    pub fn is_empty(&self) -> bool {
        self.headers.is_empty()
            && self.auth == Auth::Inherit
            && self.vars.is_empty()
            && self.scripts.is_empty()
            && self.description.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scripts {
    pub pre: Vec<SetVar>,
    pub extract: Vec<Extract>,
    pub asserts: Vec<Assertion>,
    /// Lua scripts that run after the `pre` rows (`[scripts.lua] pre`). A
    /// file holds at most one; a request with its folders applied holds one
    /// per level, the collection root first.
    pub lua_pre: Vec<LuaScript>,
    /// Lua scripts that run after the assertions (`[scripts.lua] post`).
    pub lua_post: Vec<LuaScript>,
}

impl Scripts {
    pub fn is_empty(&self) -> bool {
        self.pre.is_empty()
            && self.extract.is_empty()
            && self.asserts.is_empty()
            && self.lua_pre.is_empty()
            && self.lua_post.is_empty()
    }
}

/// The code of one Lua script and where it comes from.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct LuaScript {
    /// Empty for the request's own script; `folder <path>` for a folder's
    /// script after `inherit::apply`. Error messages name it.
    pub origin: String,
    pub code: String,
}

impl LuaScript {
    /// The request's own script; `None` when `code` is blank.
    pub fn own(code: impl Into<String>) -> Option<Self> {
        let code = code.into();
        (!code.trim().is_empty()).then(|| Self {
            origin: String::new(),
            code,
        })
    }
}

/// Pre-request step: set `var` to `value` (a template that may reference variables).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SetVar {
    pub var: String,
    pub value: String,
}

/// Post-response step: extract a value from the response into `var`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extract {
    pub var: String,
    pub source: Source,
    /// What to read from the source; see [`Source`].
    pub expr: String,
    /// Treat the extracted value as a secret (masked in UI and history).
    pub secret: bool,
}

/// The part of a response a script rule reads. The rule's `expr` selects
/// inside it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    /// The JSON body; `expr` is a JSONPath.
    #[default]
    Body,
    /// A header value; `expr` is the header name.
    Header,
    /// The status code; `expr` is not used.
    Status,
    /// A time phase in milliseconds; `expr` is `total` (empty), `ttfb`,
    /// `dns`, `connect`, `tls` or `download`.
    Time,
    /// A size in bytes; `expr` is `total` (empty), `body` or `headers`.
    Size,
    /// The lowercase hex digest of the body; `expr` is `md5`, `sha1`,
    /// `sha256` or `sha512`.
    Hash,
    /// The XML body; `expr` is an XPath 3.1 expression.
    XPath,
    /// The raw body text; `expr` is a regular expression. The value is
    /// capture group 1, or the whole match.
    Regex,
    /// A cookie of this response's `Set-Cookie` headers; `expr` is its name.
    Cookie,
}

impl Source {
    pub const ALL: [Source; 9] = [
        Source::Body,
        Source::Header,
        Source::Status,
        Source::Time,
        Source::Size,
        Source::Hash,
        Source::XPath,
        Source::Regex,
        Source::Cookie,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Source::Body => "jsonpath",
            Source::Header => "header",
            Source::Status => "status",
            Source::Time => "time",
            Source::Size => "size",
            Source::Hash => "hash",
            Source::XPath => "xpath",
            Source::Regex => "regex",
            Source::Cookie => "cookie",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assertion {
    pub source: Source,
    pub expr: String,
    pub op: AssertOp,
    pub value: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AssertOp {
    #[default]
    Eq,
    Ne,
    Contains,
    NotContains,
    Exists,
    NotExists,
    Lt,
    Le,
    Gt,
    Ge,
    Matches,
}

impl AssertOp {
    pub const ALL: [AssertOp; 11] = [
        AssertOp::Eq,
        AssertOp::Ne,
        AssertOp::Contains,
        AssertOp::NotContains,
        AssertOp::Exists,
        AssertOp::NotExists,
        AssertOp::Lt,
        AssertOp::Le,
        AssertOp::Gt,
        AssertOp::Ge,
        AssertOp::Matches,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AssertOp::Eq => "eq",
            AssertOp::Ne => "ne",
            AssertOp::Contains => "contains",
            AssertOp::NotContains => "not_contains",
            AssertOp::Exists => "exists",
            AssertOp::NotExists => "not_exists",
            AssertOp::Lt => "lt",
            AssertOp::Le => "le",
            AssertOp::Gt => "gt",
            AssertOp::Ge => "ge",
            AssertOp::Matches => "matches",
        }
    }

    pub fn symbol(self) -> &'static str {
        match self {
            AssertOp::Eq => "==",
            AssertOp::Ne => "!=",
            AssertOp::Contains => "contains",
            AssertOp::NotContains => "!contains",
            AssertOp::Exists => "exists",
            AssertOp::NotExists => "!exists",
            AssertOp::Lt => "<",
            AssertOp::Le => "<=",
            AssertOp::Gt => ">",
            AssertOp::Ge => ">=",
            AssertOp::Matches => "matches",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// Whether the operator compares against `value`.
    pub fn takes_value(self) -> bool {
        !matches!(self, AssertOp::Exists | AssertOp::NotExists)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrpcRequest {
    /// `host:port`, optionally prefixed with `http://` or `https://`.
    pub target: String,
    /// Fully-qualified service name, e.g. `acme.v1.Users`.
    pub service: String,
    pub method: String,
    /// Request message as JSON. For client-streaming and bidi calls a JSON array
    /// sends one message per element.
    pub message: String,
    pub metadata: Vec<KeyValue>,
    pub auth: Auth,
    pub tls: bool,
    pub verify_tls: bool,
    pub deadline_ms: Option<u64>,
    pub schema: SchemaSource,
    /// The fields every request kind has.
    pub common: RequestCommon,
}

impl Default for GrpcRequest {
    fn default() -> Self {
        Self {
            target: String::new(),
            service: String::new(),
            method: String::new(),
            message: "{}".into(),
            metadata: Vec::new(),
            auth: Auth::Inherit,
            tls: false,
            verify_tls: true,
            deadline_ms: None,
            schema: SchemaSource::Reflection,
            common: RequestCommon::default(),
        }
    }
}

/// A WebSocket session: one URL, optional headers, subprotocols and an
/// opening message document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSocketRequest {
    /// `ws://`, `wss://` (or `http://`/`https://`); a URL without a scheme
    /// uses the `tls` option.
    pub url: String,
    pub headers: Vec<KeyValue>,
    /// Values for `Sec-WebSocket-Protocol`, in order.
    pub subprotocols: Vec<String>,
    /// Messages sent right after the handshake, as a JSON document: an array
    /// of strings (text frames) and `{ "binary": "base64" }` objects (binary
    /// frames), or one such value alone. Stored and read as written (a
    /// half-typed document is a valid file); rejected only at send time.
    pub messages: String,
    pub auth: Auth,
    pub options: WsOptions,
    /// The fields every request kind has.
    pub common: RequestCommon,
}

impl Default for WebSocketRequest {
    fn default() -> Self {
        Self {
            url: String::new(),
            headers: Vec::new(),
            subprotocols: Vec::new(),
            messages: "[]".into(),
            auth: Auth::Inherit,
            options: WsOptions::default(),
            common: RequestCommon::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WsOptions {
    /// Handshake timeout. `None` uses the configured default.
    pub timeout_ms: Option<u64>,
    /// Optional whole-session deadline (like the gRPC deadline).
    pub deadline_ms: Option<u64>,
    pub verify_tls: bool,
    /// TLS when the URL has no scheme; default false.
    pub tls: bool,
    /// Replay cookies from the collection's jar in the handshake. `None`
    /// follows the `[http] cookie_jar` config.
    pub cookies: Option<bool>,
}

impl Default for WsOptions {
    fn default() -> Self {
        Self {
            timeout_ms: None,
            deadline_ms: None,
            verify_tls: true,
            tls: false,
            cookies: None,
        }
    }
}

/// A live server-sent events session. SSE has no request body.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SseRequest {
    pub url: String,
    pub params: Vec<KeyValue>,
    pub headers: Vec<KeyValue>,
    pub auth: Auth,
    pub options: SseOptions,
    /// The fields every request kind has.
    pub common: RequestCommon,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseOptions {
    pub timeout_ms: Option<u64>,
    pub deadline_ms: Option<u64>,
    pub max_events: Option<u64>,
    pub follow_redirects: bool,
    pub verify_tls: bool,
    pub cookies: Option<bool>,
}
impl Default for SseOptions {
    fn default() -> Self {
        Self {
            timeout_ms: None,
            deadline_ms: None,
            max_events: None,
            follow_redirects: true,
            verify_tls: true,
            cookies: None,
        }
    }
}
impl SseRequest {
    /// HTTP opening fields, for shared resolution and authentication.
    pub fn http_request(&self) -> HttpRequest {
        let mut headers = self.headers.clone();
        if !headers
            .iter()
            .any(|h| h.enabled && h.name.trim().eq_ignore_ascii_case("accept"))
        {
            headers.push(KeyValue::new("Accept", "text/event-stream"));
        }
        HttpRequest {
            method: "GET".into(),
            url: self.url.clone(),
            params: self.params.clone(),
            headers,
            body: Body::None,
            auth: self.auth.clone(),
            options: HttpOptions {
                timeout_ms: self.options.timeout_ms,
                follow_redirects: self.options.follow_redirects,
                verify_tls: self.options.verify_tls,
                cookies: self.options.cookies,
            },
            common: RequestCommon {
                scripts: self.common.scripts.clone(),
                description: String::new(),
            },
        }
    }
}

/// The MQTT protocol version of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MqttProtocol {
    #[default]
    V3_1_1,
    V5,
}

impl MqttProtocol {
    pub const ALL: [MqttProtocol; 2] = [MqttProtocol::V3_1_1, MqttProtocol::V5];

    /// The text the request file uses: `3.1.1` or `5`.
    pub const fn as_str(self) -> &'static str {
        match self {
            MqttProtocol::V3_1_1 => "3.1.1",
            MqttProtocol::V5 => "5",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text.trim() {
            "3.1.1" => Some(MqttProtocol::V3_1_1),
            "5" => Some(MqttProtocol::V5),
            _ => None,
        }
    }
}

/// The default MQTT keep-alive interval, in seconds.
pub const MQTT_KEEP_ALIVE_S: u64 = 60;

/// A live MQTT session: connect, subscribe, publish, then receive messages
/// until the deadline, a disconnect or a cancel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MqttRequest {
    /// `mqtt://host[:port]` or `mqtts://host[:port]`.
    pub url: String,
    /// Empty generates `sendito-<8 hex>` at send time.
    pub client_id: String,
    /// The MQTT user name and password. When they are empty, Basic auth
    /// gives them.
    pub username: String,
    pub password: String,
    pub protocol: MqttProtocol,
    pub clean_session: bool,
    pub keep_alive_s: u64,
    pub options: MqttOptions,
    pub subscriptions: Vec<MqttSubscription>,
    /// Sent in this order after every subscription is acknowledged.
    pub publish: Vec<MqttPublish>,
    pub last_will: Option<MqttWill>,
    /// `Inherit`, `None` or `Basic`; other kinds fail before the connection.
    pub auth: Auth,
    /// The fields every request kind has.
    pub common: RequestCommon,
}

impl Default for MqttRequest {
    fn default() -> Self {
        Self {
            url: String::new(),
            client_id: String::new(),
            username: String::new(),
            password: String::new(),
            protocol: MqttProtocol::default(),
            clean_session: true,
            keep_alive_s: MQTT_KEEP_ALIVE_S,
            options: MqttOptions::default(),
            subscriptions: Vec::new(),
            publish: Vec::new(),
            last_will: None,
            auth: Auth::default(),
            common: RequestCommon::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MqttOptions {
    /// The time to wait for CONNACK. `None` uses `[mqtt] timeout_ms`.
    pub timeout_ms: Option<u64>,
    /// The session ends after this time. `None` runs until a disconnect.
    pub deadline_ms: Option<u64>,
    pub verify_tls: bool,
}

impl Default for MqttOptions {
    fn default() -> Self {
        Self {
            timeout_ms: None,
            deadline_ms: None,
            verify_tls: true,
        }
    }
}

/// One subscription row: a topic filter and the maximum QoS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MqttSubscription {
    pub topic: String,
    pub qos: u8,
    pub enabled: bool,
}

impl Default for MqttSubscription {
    fn default() -> Self {
        Self {
            topic: String::new(),
            qos: 0,
            enabled: true,
        }
    }
}

/// The payload of a publish or of the Last Will: UTF-8 text, or base64
/// text that decodes to the bytes on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MqttPayload {
    pub text: String,
    pub base64: bool,
}

impl MqttPayload {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            base64: false,
        }
    }
}

/// One message: a publish sent when the session starts, or the Last Will
/// the broker publishes when the connection drops ungracefully.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MqttMessage {
    pub topic: String,
    pub payload: MqttPayload,
    pub qos: u8,
    pub retain: bool,
}

pub type MqttPublish = MqttMessage;
pub type MqttWill = MqttMessage;

/// An MCP call: one tool on one server from the collection config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpRequest {
    /// Server name, a key of `[mcp.servers]` in the collection config.
    pub server: String,
    pub tool: String,
    /// Tool arguments as JSON text. An empty string omits the arguments property.
    pub arguments: String,
    pub timeout_ms: Option<u64>,
    /// The fields every request kind has.
    pub common: RequestCommon,
}

impl Default for McpRequest {
    fn default() -> Self {
        Self {
            server: String::new(),
            tool: String::new(),
            arguments: "{}".into(),
            timeout_ms: None,
            common: RequestCommon::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SchemaSource {
    #[default]
    Reflection,
    Files {
        files: Vec<String>,
        import_paths: Vec<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_conversion_keeps_text() {
        let b = Body::Json { text: "{}".into() };
        assert_eq!(
            b.convert(BodyKind::Raw),
            Body::Raw {
                content_type: "text/plain".into(),
                text: "{}".into()
            }
        );
        assert_eq!(b.convert(BodyKind::None), Body::None);
        assert_eq!(b.convert(BodyKind::Form), Body::Form { fields: vec![] });
        assert_eq!(b.kind(), BodyKind::Json);
        let f = Body::Form {
            fields: vec![KeyValue::new("a", "b")],
        };
        assert_eq!(f.convert(BodyKind::Form), f);
        assert!(matches!(
            f.convert(BodyKind::Multipart),
            Body::Multipart { .. }
        ));
        assert!(matches!(f.convert(BodyKind::File), Body::File { .. }));
    }

    #[test]
    fn graphql_conversion() {
        // A JSON object with a query is split into its parts.
        let b = graphql_body(
            r#"{"query": "{ me { id } }", "variables": {"x": 1}, "operationName": "Me"}"#,
        );
        assert_eq!(
            b,
            Body::GraphQL {
                query: "{ me { id } }".into(),
                variables: "{\"x\":1}".into(),
                operation_name: "Me".into(),
            }
        );
        assert_eq!(b.kind(), BodyKind::GraphQL);
        // Any other text is the query as a whole.
        let plain = graphql_body("query Q { a }");
        assert_eq!(
            plain,
            Body::GraphQL {
                query: "query Q { a }".into(),
                variables: String::new(),
                operation_name: String::new(),
            }
        );
        // A JSON object without a string query, and a JSON array, are queries too.
        assert!(matches!(
            graphql_body("{\"a\": 1}"),
            Body::GraphQL { query, .. } if query == "{\"a\": 1}"
        ));
        assert!(matches!(
            graphql_body("[1]"),
            Body::GraphQL { query, .. } if query == "[1]"
        ));
        // Converting a GraphQL body to JSON gives the document, and back.
        let doc = b.convert(BodyKind::Json);
        assert_eq!(
            doc,
            Body::Json {
                text:
                    "{\"query\":\"{ me { id } }\",\"variables\":{\"x\":1},\"operationName\":\"Me\"}"
                        .into()
            }
        );
        assert_eq!(doc.convert(BodyKind::GraphQL), b);
        assert_eq!(
            plain.convert(BodyKind::Raw),
            Body::Raw {
                content_type: "text/plain".into(),
                text: "{\"query\":\"query Q { a }\"}".into(),
            }
        );
        assert_eq!(b.convert(BodyKind::GraphQL), b);
        assert_eq!(
            graphql_document_json("{ a }", "", ""),
            Ok("{\"query\":\"{ a }\"}".into())
        );
        assert_eq!(
            graphql_document_json("{ a }", "{\"n\": 2}", "A"),
            Ok("{\"query\":\"{ a }\",\"variables\":{\"n\":2},\"operationName\":\"A\"}".into())
        );
        assert!(graphql_document_json("{ a }", "[1]", "").is_err());
        assert!(graphql_document_json("{ a }", "{", "").is_err());
    }

    #[test]
    fn kinds_roundtrip() {
        for k in BodyKind::ALL {
            assert_eq!(BodyKind::parse(k.as_str()), Some(k));
            assert!(!k.label().is_empty());
        }
        for k in AuthKind::ALL {
            assert_eq!(Auth::with_kind(k).kind(), k);
            assert!(!k.label().is_empty());
            assert!(!k.as_str().is_empty());
        }
        for s in Source::ALL {
            assert_eq!(Source::parse(s.as_str()), Some(s));
        }
        for o in AssertOp::ALL {
            assert_eq!(AssertOp::parse(o.as_str()), Some(o));
            assert!(!o.symbol().is_empty());
        }
        assert!(!AssertOp::Exists.takes_value());
        assert!(AssertOp::Eq.takes_value());
    }

    #[test]
    fn request_accessors() {
        let mut r = Request::new_http();
        assert_eq!(r.badge(), "GET");
        r.auth_mut()
            .expect("a new HTTP request has auth")
            .clone_from(&Auth::Bearer { token: "t".into() });
        assert_eq!(r.auth().kind(), AuthKind::Bearer);
        r.scripts_mut().pre.push(SetVar::default());
        assert!(!r.scripts().is_empty());
        let g = Request::new_grpc();
        assert_eq!(g.badge(), "GRPC");
        assert!(g.scripts().is_empty());
        assert_eq!(g.auth(), &Auth::Inherit);
        assert!(!KeyValue::new("a", "b").disabled().enabled);
        assert!(KeyValue::default().enabled);
    }
}
