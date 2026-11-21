//! The request model. These types mirror the on-disk TOML schema (see FORMAT.md)
//! but carry no serialization logic: `sendito-store` owns the file format.

/// A saved request: either HTTP or gRPC.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Request {
    Http(HttpRequest),
    Grpc(GrpcRequest),
}

impl Request {
    /// Short label used for tree badges (`GET`, `POST`, `GRPC`).
    pub fn badge(&self) -> &str {
        match self {
            Request::Http(h) => &h.method,
            Request::Grpc(_) => "GRPC",
        }
    }

    pub fn scripts(&self) -> &Scripts {
        match self {
            Request::Http(h) => &h.scripts,
            Request::Grpc(g) => &g.scripts,
        }
    }

    pub fn scripts_mut(&mut self) -> &mut Scripts {
        match self {
            Request::Http(h) => &mut h.scripts,
            Request::Grpc(g) => &mut g.scripts,
        }
    }

    pub fn auth(&self) -> &Auth {
        match self {
            Request::Http(h) => &h.auth,
            Request::Grpc(g) => &g.auth,
        }
    }

    pub fn auth_mut(&mut self) -> &mut Auth {
        match self {
            Request::Http(h) => &mut h.auth,
            Request::Grpc(g) => &mut g.auth,
        }
    }

    pub fn new_http() -> Self {
        Request::Http(HttpRequest::default())
    }

    pub fn new_grpc() -> Self {
        Request::Grpc(GrpcRequest::default())
    }
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
    pub scripts: Scripts,
}

impl Default for HttpRequest {
    fn default() -> Self {
        Self {
            method: "GET".into(),
            url: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            body: Body::None,
            auth: Auth::None,
            options: HttpOptions::default(),
            scripts: Scripts::default(),
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
        }
    }

    /// Converts the body to another kind, carrying over text when it makes sense.
    pub fn convert(&self, kind: BodyKind) -> Body {
        let text = match self {
            Body::Raw { text, .. } | Body::Json { text } => text.clone(),
            _ => String::new(),
        };
        match kind {
            BodyKind::None => Body::None,
            BodyKind::Raw => Body::Raw {
                content_type: "text/plain".into(),
                text,
            },
            BodyKind::Json => Body::Json { text },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    None,
    Raw,
    Json,
    Form,
    Multipart,
    File,
}

impl BodyKind {
    pub const ALL: [BodyKind; 6] = [
        BodyKind::None,
        BodyKind::Json,
        BodyKind::Raw,
        BodyKind::Form,
        BodyKind::Multipart,
        BodyKind::File,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            BodyKind::None => "none",
            BodyKind::Raw => "raw",
            BodyKind::Json => "json",
            BodyKind::Form => "form",
            BodyKind::Multipart => "multipart",
            BodyKind::File => "file",
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
    #[default]
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
}

impl Auth {
    pub fn kind(&self) -> AuthKind {
        match self {
            Auth::None => AuthKind::None,
            Auth::Basic { .. } => AuthKind::Basic,
            Auth::Bearer { .. } => AuthKind::Bearer,
            Auth::ApiKey { .. } => AuthKind::ApiKey,
        }
    }

    pub fn with_kind(kind: AuthKind) -> Auth {
        match kind {
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
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthKind {
    None,
    Basic,
    Bearer,
    ApiKey,
}

impl AuthKind {
    pub const ALL: [AuthKind; 4] = [
        AuthKind::None,
        AuthKind::Basic,
        AuthKind::Bearer,
        AuthKind::ApiKey,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            AuthKind::None => "none",
            AuthKind::Basic => "basic",
            AuthKind::Bearer => "bearer",
            AuthKind::ApiKey => "apikey",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AuthKind::None => "No auth",
            AuthKind::Basic => "Basic",
            AuthKind::Bearer => "Bearer token",
            AuthKind::ApiKey => "API key",
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
}

impl Default for HttpOptions {
    fn default() -> Self {
        Self {
            timeout_ms: None,
            follow_redirects: true,
            verify_tls: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Scripts {
    pub pre: Vec<SetVar>,
    pub extract: Vec<Extract>,
    pub asserts: Vec<Assertion>,
}

impl Scripts {
    pub fn is_empty(&self) -> bool {
        self.pre.is_empty() && self.extract.is_empty() && self.asserts.is_empty()
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
    /// JSONPath for `Body`, header name for `Header`, ignored for `Status`.
    pub expr: String,
    /// Treat the extracted value as a secret (masked in UI and history).
    pub secret: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Source {
    #[default]
    Body,
    Header,
    Status,
}

impl Source {
    pub const ALL: [Source; 3] = [Source::Body, Source::Header, Source::Status];

    pub fn as_str(self) -> &'static str {
        match self {
            Source::Body => "jsonpath",
            Source::Header => "header",
            Source::Status => "status",
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
    pub scripts: Scripts,
}

impl Default for GrpcRequest {
    fn default() -> Self {
        Self {
            target: String::new(),
            service: String::new(),
            method: String::new(),
            message: "{}".into(),
            metadata: Vec::new(),
            auth: Auth::None,
            tls: false,
            verify_tls: true,
            deadline_ms: None,
            schema: SchemaSource::Reflection,
            scripts: Scripts::default(),
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
        r.auth_mut().clone_from(&Auth::Bearer { token: "t".into() });
        assert_eq!(r.auth().kind(), AuthKind::Bearer);
        r.scripts_mut().pre.push(SetVar::default());
        assert!(!r.scripts().is_empty());
        let g = Request::new_grpc();
        assert_eq!(g.badge(), "GRPC");
        assert!(g.scripts().is_empty());
        assert_eq!(g.auth(), &Auth::None);
        assert!(!KeyValue::new("a", "b").disabled().enabled);
        assert!(KeyValue::default().enabled);
    }
}
