//! An open request tab: editor widgets for every field, conversion to and
//! from the request model, and response state.

use sendito_core::model::*;
use sendito_grpc::ServiceInfo;

use crate::editor::{Highlight, TextEditor};
use crate::form::{Cell, ColKind, Column, Table};
use crate::viewer::{Doc, Viewer};

pub const HTTP_TABS: &[&str] = &["Params", "Headers", "Body", "Auth", "Scripts", "Options"];
pub const GRPC_TABS: &[&str] = &["Message", "Metadata", "Auth", "Scripts", "Options"];
pub const MCP_TABS: &[&str] = &["Tool", "Args", "Scripts", "Options"];
pub const RESP_TABS: &[&str] = &["Body", "Headers", "Timing", "Results"];

/// Response tab names: the fourth holds assertions, extracted variables and
/// notes ("Results"); for gRPC and MCP it also holds the message log ("Log").
pub fn resp_tab_names(kind: Kind) -> [&'static str; 4] {
    match kind {
        Kind::Http => ["Body", "Headers", "Timing", "Results"],
        Kind::Grpc => ["Body", "Metadata", "Timing", "Log"],
        Kind::Mcp => ["Body", "Request", "Timing", "Log"],
    }
}

pub const SOURCES: &[&str] = &["jsonpath", "header", "status"];
/// Assertion operators as shown in the table (same order as `AssertOp::ALL`).
pub const OPS: &[&str] = &[
    "==",
    "!=",
    "contains",
    "!contains",
    "exists",
    "!exists",
    "<",
    "<=",
    ">",
    ">=",
    "matches",
];
pub const PART_KINDS: &[&str] = &["text", "file"];

/// A focusable field in the request editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Field {
    Params,
    Headers,
    BodyKind,
    BodyText,
    RawContentType,
    FormFields,
    Parts,
    FilePath,
    FileContentType,
    AuthKind,
    AuthUser,
    AuthPass,
    AuthToken,
    AuthOAuthName,
    AuthKey,
    AuthValue,
    AuthIn,
    ScriptSection,
    ScriptPre,
    ScriptExtract,
    ScriptAssert,
    Timeout,
    FollowRedirects,
    VerifyTls,
    Cookies,
    Message,
    Metadata,
    SchemaSource,
    ProtoFiles,
    ImportPaths,
    Tls,
    GrpcVerify,
    Deadline,
    McpServer,
    McpTool,
}

impl Field {
    pub fn label(self) -> &'static str {
        match self {
            Field::Params => "Query parameters",
            Field::Headers => "Headers",
            Field::BodyKind => "Body type",
            Field::BodyText => "Body",
            Field::RawContentType => "Content-Type",
            Field::FormFields => "Fields",
            Field::Parts => "Parts",
            Field::FilePath => "File path",
            Field::FileContentType => "Content-Type",
            Field::AuthKind => "Auth type",
            Field::AuthUser => "Username",
            Field::AuthPass => "Password",
            Field::AuthToken => "Token",
            Field::AuthOAuthName => "OAuth identity",
            Field::AuthKey => "Key name",
            Field::AuthValue => "Key value",
            Field::AuthIn => "Send in",
            Field::ScriptSection => "Section",
            Field::ScriptPre => "Pre-request: set variables",
            Field::ScriptExtract => "After response: extract into variables",
            Field::ScriptAssert => "After response: assertions",
            Field::Timeout => "Timeout (ms, empty = default)",
            Field::FollowRedirects => "Follow redirects",
            Field::VerifyTls => "Verify TLS certificates",
            Field::Cookies => "Cookie jar",
            Field::Message => {
                "Request message (JSON; an array sends several on client/bidi streams)"
            }
            Field::Metadata => "Metadata",
            Field::SchemaSource => "Schema from",
            Field::ProtoFiles => ".proto files (comma-separated, relative to the collection)",
            Field::ImportPaths => "Import paths (comma-separated)",
            Field::Tls => "Use TLS",
            Field::GrpcVerify => "Verify TLS certificates",
            Field::Deadline => "Deadline (ms, empty = none)",
            Field::McpServer => "Server (a [mcp.servers] entry of the config)",
            Field::McpTool => "Tool",
        }
    }

    pub fn is_table(self) -> bool {
        matches!(
            self,
            Field::Params
                | Field::Headers
                | Field::FormFields
                | Field::Parts
                | Field::ScriptPre
                | Field::ScriptExtract
                | Field::ScriptAssert
                | Field::Metadata
        )
    }

    pub fn is_multiline(self) -> bool {
        matches!(self, Field::BodyText | Field::Message)
    }

    pub fn is_text(self) -> bool {
        matches!(
            self,
            Field::BodyText
                | Field::RawContentType
                | Field::FilePath
                | Field::FileContentType
                | Field::AuthUser
                | Field::AuthPass
                | Field::AuthToken
                | Field::AuthOAuthName
                | Field::AuthKey
                | Field::AuthValue
                | Field::Timeout
                | Field::Message
                | Field::ProtoFiles
                | Field::ImportPaths
                | Field::Deadline
                | Field::McpServer
                | Field::McpTool
        )
    }

    /// Choice fields cycle with Space/Enter/Left/Right; toggles flip.
    pub fn is_choice(self) -> bool {
        matches!(
            self,
            Field::BodyKind
                | Field::AuthKind
                | Field::AuthIn
                | Field::SchemaSource
                | Field::ScriptSection
                | Field::Cookies
        )
    }

    pub fn is_toggle(self) -> bool {
        matches!(
            self,
            Field::FollowRedirects | Field::VerifyTls | Field::Tls | Field::GrpcVerify
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Http,
    Grpc,
    Mcp,
}

/// Editor widgets for every field of a request.
#[derive(Debug, Clone)]
pub struct Editors {
    pub kind: Kind,
    pub method: String,
    pub url: TextEditor,
    pub params: Table,
    pub headers: Table,
    pub body_kind: BodyKind,
    pub body_text: TextEditor,
    pub raw_ct: TextEditor,
    pub form: Table,
    pub parts: Table,
    pub file_path: TextEditor,
    pub file_ct: TextEditor,
    pub auth_kind: AuthKind,
    pub auth_user: TextEditor,
    pub auth_pass: TextEditor,
    pub auth_token: TextEditor,
    pub auth_oauth_name: TextEditor,
    pub auth_key: TextEditor,
    pub auth_value: TextEditor,
    pub auth_in: ApiKeyLocation,
    pub pre: Table,
    pub extract: Table,
    pub asserts: Table,
    pub timeout: TextEditor,
    pub follow: bool,
    pub verify: bool,
    /// `[options] cookies`: `None` follows the `[http] cookie_jar` config.
    pub cookies: Option<bool>,
    pub target: TextEditor,
    /// `Service/Method`, typed or picked from the schema.
    pub grpc_method_text: TextEditor,
    pub message: TextEditor,
    pub metadata: Table,
    pub tls: bool,
    pub grpc_verify: bool,
    pub deadline: TextEditor,
    pub schema_files: bool,
    pub proto_files: TextEditor,
    pub import_paths: TextEditor,
    pub mcp_server: TextEditor,
    pub mcp_tool: TextEditor,
    /// Scripts tab section: 0 pre-request, 1 extract, 2 assertions.
    pub script_section: usize,
}

fn ms_text(v: Option<u64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

fn parse_ms(s: &str) -> Option<u64> {
    s.trim().parse().ok()
}

fn list_text(v: &[String]) -> String {
    v.join(", ")
}

fn parse_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// `Service/Method` as shown in the method field.
fn grpc_method_label(service: &str, method: &str) -> String {
    if service.is_empty() && method.is_empty() {
        String::new()
    } else if method.is_empty() {
        service.to_string()
    } else {
        format!("{service}/{method}")
    }
}

/// Splits the method field into (service, method). A value without a slash is
/// the service only: the last dot-separated segment may also be a method, but
/// guessing there would turn a half-typed service into a wrong call.
fn split_method(text: &str) -> (String, String) {
    let text = text.trim();
    match text.rsplit_once('/') {
        Some((s, m)) => (s.trim().to_string(), m.trim().to_string()),
        None => (text.to_string(), String::new()),
    }
}

fn kv_table(name: &'static str, value: &'static str, rows: &[KeyValue]) -> Table {
    let mut t = Table::kv(name, value);
    for kv in rows {
        t.push_texts(kv.enabled, &[&kv.name, &kv.value]);
    }
    t
}

fn kv_rows(t: &Table) -> Vec<KeyValue> {
    t.values()
        .into_iter()
        .map(|r| KeyValue {
            name: r.cells[0].text(),
            value: r.cells[1].text(),
            enabled: r.enabled,
        })
        .collect()
}

fn t(s: &str) -> Cell {
    Cell::Text(TextEditor::single(s))
}

impl Editors {
    pub fn from_request(req: &Request) -> Self {
        let mut e = Editors {
            kind: Kind::Http,
            method: "GET".into(),
            url: TextEditor::single("")
                .with_placeholder("https://…  or paste a curl command")
                .with_highlight(Highlight::Url),
            params: Table::kv("Name", "Value"),
            headers: Table::kv("Header", "Value"),
            body_kind: BodyKind::None,
            body_text: TextEditor::multi("").with_highlight(Highlight::Json),
            raw_ct: TextEditor::single("text/plain"),
            form: Table::kv("Field", "Value"),
            parts: Table::new(
                vec![
                    Column {
                        title: "Name",
                        kind: ColKind::Text,
                        weight: 2,
                    },
                    Column {
                        title: "Kind",
                        kind: ColKind::Choice(PART_KINDS),
                        weight: 1,
                    },
                    Column {
                        title: "Value / path",
                        kind: ColKind::Text,
                        weight: 2,
                    },
                    Column {
                        title: "Filename",
                        kind: ColKind::Text,
                        weight: 3,
                    },
                    Column {
                        title: "Content-Type",
                        kind: ColKind::Text,
                        weight: 3,
                    },
                ],
                true,
            ),
            file_path: TextEditor::single("")
                .with_placeholder("path/to/file (relative to the collection)"),
            file_ct: TextEditor::single("").with_placeholder("application/octet-stream"),
            auth_kind: AuthKind::None,
            auth_user: TextEditor::single(""),
            auth_pass: TextEditor::single(""),
            auth_token: TextEditor::single("").with_placeholder("{{token}}"),
            auth_oauth_name: TextEditor::single("").with_placeholder("name from [oauth]"),
            auth_key: TextEditor::single("X-API-Key"),
            auth_value: TextEditor::single(""),
            auth_in: ApiKeyLocation::Header,
            pre: Table::new(
                vec![
                    Column {
                        title: "Variable",
                        kind: ColKind::Text,
                        weight: 2,
                    },
                    Column {
                        title: "Value (template)",
                        kind: ColKind::Text,
                        weight: 4,
                    },
                ],
                false,
            ),
            extract: Table::new(
                vec![
                    Column {
                        title: "Variable",
                        kind: ColKind::Text,
                        weight: 2,
                    },
                    Column {
                        title: "From",
                        kind: ColKind::Choice(SOURCES),
                        weight: 1,
                    },
                    Column {
                        title: "JSONPath / header name",
                        kind: ColKind::Text,
                        weight: 4,
                    },
                    Column {
                        title: "Secret",
                        kind: ColKind::Toggle,
                        weight: 1,
                    },
                ],
                false,
            ),
            asserts: Table::new(
                vec![
                    Column {
                        title: "From",
                        kind: ColKind::Choice(SOURCES),
                        weight: 1,
                    },
                    Column {
                        title: "JSONPath / header",
                        kind: ColKind::Text,
                        weight: 3,
                    },
                    Column {
                        title: "Op",
                        kind: ColKind::Choice(OPS),
                        weight: 1,
                    },
                    Column {
                        title: "Expected",
                        kind: ColKind::Text,
                        weight: 3,
                    },
                ],
                false,
            ),
            timeout: TextEditor::single("").with_placeholder("default"),
            follow: true,
            verify: true,
            cookies: None,
            target: TextEditor::single("").with_placeholder("localhost:50051  (https:// for TLS)"),
            grpc_method_text: TextEditor::single("")
                .with_placeholder("Service/Method  (Enter lists the schema)"),
            message: TextEditor::multi("{}").with_highlight(Highlight::Json),
            metadata: Table::kv("Key", "Value"),
            tls: false,
            grpc_verify: true,
            deadline: TextEditor::single("").with_placeholder("none"),
            schema_files: false,
            proto_files: TextEditor::single("").with_placeholder("protos/service.proto"),
            import_paths: TextEditor::single("").with_placeholder("protos"),
            mcp_server: TextEditor::single("").with_placeholder("server name from [mcp.servers]"),
            mcp_tool: TextEditor::single("").with_placeholder("tool name"),
            script_section: 0,
        };
        let scripts = req.scripts();
        for s in &scripts.pre {
            e.pre.push(true, vec![t(&s.var), t(&s.value)]);
        }
        for x in &scripts.extract {
            let src = Source::ALL.iter().position(|s| *s == x.source).unwrap_or(0);
            e.extract.push(
                true,
                vec![
                    t(&x.var),
                    Cell::Choice(src),
                    t(&x.expr),
                    Cell::Toggle(x.secret),
                ],
            );
        }
        for a in &scripts.asserts {
            let src = Source::ALL.iter().position(|s| *s == a.source).unwrap_or(0);
            let op = AssertOp::ALL.iter().position(|o| *o == a.op).unwrap_or(0);
            e.asserts.push(
                true,
                vec![Cell::Choice(src), t(&a.expr), Cell::Choice(op), t(&a.value)],
            );
        }
        e.script_section = if !scripts.pre.is_empty() {
            0
        } else if !scripts.extract.is_empty() {
            1
        } else if !scripts.asserts.is_empty() {
            2
        } else {
            0
        };
        e.set_auth(req.auth());
        match req {
            Request::Http(h) => {
                e.kind = Kind::Http;
                e.method = h.method.clone();
                e.url.set_text(&h.url);
                e.params = kv_table("Name", "Value", &h.params);
                e.headers = kv_table("Header", "Value", &h.headers);
                e.body_kind = h.body.kind();
                match &h.body {
                    Body::None => {}
                    Body::Raw { content_type, text } => {
                        e.raw_ct.set_text(content_type);
                        e.body_text.set_text(text);
                        e.body_text.highlight = Highlight::None;
                    }
                    Body::Json { text } => e.body_text.set_text(text),
                    Body::Form { fields } => e.form = kv_table("Field", "Value", fields),
                    Body::Multipart { parts } => {
                        for p in parts {
                            let (kind, value, filename) = match &p.kind {
                                PartKind::Text { value } => (0, value.clone(), String::new()),
                                PartKind::File { path, filename } => {
                                    (1, path.clone(), filename.clone().unwrap_or_default())
                                }
                            };
                            e.parts.push(
                                p.enabled,
                                vec![
                                    t(&p.name),
                                    Cell::Choice(kind),
                                    t(&value),
                                    t(&filename),
                                    t(&p.content_type),
                                ],
                            );
                        }
                    }
                    Body::File { path, content_type } => {
                        e.file_path.set_text(path);
                        e.file_ct.set_text(content_type);
                    }
                }
                e.timeout.set_text(&ms_text(h.options.timeout_ms));
                e.follow = h.options.follow_redirects;
                e.verify = h.options.verify_tls;
                e.cookies = h.options.cookies;
            }
            Request::Grpc(g) => {
                e.kind = Kind::Grpc;
                e.target.set_text(&g.target);
                e.grpc_method_text
                    .set_text(&grpc_method_label(&g.service, &g.method));
                e.message.set_text(&g.message);
                e.metadata = kv_table("Key", "Value", &g.metadata);
                e.tls = g.tls;
                e.grpc_verify = g.verify_tls;
                e.deadline.set_text(&ms_text(g.deadline_ms));
                if let SchemaSource::Files {
                    files,
                    import_paths,
                } = &g.schema
                {
                    e.schema_files = true;
                    e.proto_files.set_text(&list_text(files));
                    e.import_paths.set_text(&list_text(import_paths));
                }
            }
            Request::Mcp(m) => {
                e.kind = Kind::Mcp;
                e.mcp_server.set_text(&m.server);
                e.mcp_tool.set_text(&m.tool);
                e.message.set_text(&m.arguments);
                e.timeout.set_text(&ms_text(m.timeout_ms));
            }
        }
        e
    }

    fn set_auth(&mut self, a: &Auth) {
        self.auth_kind = a.kind();
        match a {
            Auth::None => {}
            Auth::Basic { username, password } => {
                self.auth_user.set_text(username);
                self.auth_pass.set_text(password);
            }
            Auth::Bearer { token } => self.auth_token.set_text(token),
            Auth::OAuth { name } => self.auth_oauth_name.set_text(name),
            Auth::ApiKey {
                key,
                value,
                location,
            } => {
                self.auth_key.set_text(key);
                self.auth_value.set_text(value);
                self.auth_in = *location;
            }
        }
    }

    fn auth(&self) -> Auth {
        match self.auth_kind {
            AuthKind::None => Auth::None,
            AuthKind::Basic => Auth::Basic {
                username: self.auth_user.text(),
                password: self.auth_pass.text(),
            },
            AuthKind::Bearer => Auth::Bearer {
                token: self.auth_token.text(),
            },
            AuthKind::OAuth => Auth::OAuth {
                name: self.auth_oauth_name.text(),
            },
            AuthKind::ApiKey => Auth::ApiKey {
                key: self.auth_key.text(),
                value: self.auth_value.text(),
                location: self.auth_in,
            },
        }
    }

    fn scripts(&self) -> Scripts {
        Scripts {
            pre: self
                .pre
                .values()
                .into_iter()
                .map(|r| SetVar {
                    var: r.cells[0].text(),
                    value: r.cells[1].text(),
                })
                .collect(),
            extract: self
                .extract
                .values()
                .into_iter()
                .map(|r| Extract {
                    var: r.cells[0].text(),
                    source: Source::ALL[r.cells[1].choice().min(2)],
                    expr: r.cells[2].text(),
                    secret: r.cells[3].toggle(),
                })
                .collect(),
            asserts: self
                .asserts
                .values()
                .into_iter()
                .map(|r| Assertion {
                    source: Source::ALL[r.cells[0].choice().min(2)],
                    expr: r.cells[1].text(),
                    op: AssertOp::ALL[r.cells[2].choice().min(AssertOp::ALL.len() - 1)],
                    value: r.cells[3].text(),
                })
                .collect(),
        }
    }

    /// The gRPC service part of `Service/Method`.
    pub fn service(&self) -> String {
        let text = self.grpc_method_text.text();
        split_method(&text).0
    }

    /// The gRPC method part of `Service/Method`.
    pub fn method(&self) -> String {
        let text = self.grpc_method_text.text();
        split_method(&text).1
    }

    pub fn to_request(&self) -> Request {
        match self.kind {
            Kind::Http => {
                let body = match self.body_kind {
                    BodyKind::None => Body::None,
                    BodyKind::Json => Body::Json {
                        text: self.body_text.text(),
                    },
                    BodyKind::Raw => Body::Raw {
                        content_type: self.raw_ct.text(),
                        text: self.body_text.text(),
                    },
                    BodyKind::Form => Body::Form {
                        fields: kv_rows(&self.form),
                    },
                    BodyKind::Multipart => Body::Multipart {
                        parts: self
                            .parts
                            .values()
                            .into_iter()
                            .map(|r| {
                                let value = r.cells[2].text();
                                let filename = r.cells[3].text();
                                MultipartPart {
                                    name: r.cells[0].text(),
                                    kind: if r.cells[1].choice() == 1 {
                                        PartKind::File {
                                            path: value,
                                            filename: if filename.is_empty() {
                                                None
                                            } else {
                                                Some(filename)
                                            },
                                        }
                                    } else {
                                        PartKind::Text { value }
                                    },
                                    content_type: r.cells[4].text(),
                                    enabled: r.enabled,
                                }
                            })
                            .collect(),
                    },
                    BodyKind::File => Body::File {
                        path: self.file_path.text(),
                        content_type: self.file_ct.text(),
                    },
                };
                Request::Http(HttpRequest {
                    method: self.method.clone(),
                    url: self.url.text(),
                    params: kv_rows(&self.params),
                    headers: kv_rows(&self.headers),
                    body,
                    auth: self.auth(),
                    options: HttpOptions {
                        timeout_ms: parse_ms(&self.timeout.text()),
                        follow_redirects: self.follow,
                        verify_tls: self.verify,
                        cookies: self.cookies,
                    },
                    scripts: self.scripts(),
                })
            }
            Kind::Grpc => Request::Grpc(GrpcRequest {
                target: self.target.text(),
                service: self.service(),
                method: self.method(),
                message: self.message.text(),
                metadata: kv_rows(&self.metadata),
                auth: self.auth(),
                tls: self.tls,
                verify_tls: self.grpc_verify,
                deadline_ms: parse_ms(&self.deadline.text()),
                schema: if self.schema_files {
                    SchemaSource::Files {
                        files: parse_list(&self.proto_files.text()),
                        import_paths: parse_list(&self.import_paths.text()),
                    }
                } else {
                    SchemaSource::Reflection
                },
                scripts: self.scripts(),
            }),
            Kind::Mcp => Request::Mcp(McpRequest {
                server: self.mcp_server.text(),
                tool: self.mcp_tool.text(),
                arguments: self.message.text(),
                timeout_ms: parse_ms(&self.timeout.text()),
                scripts: self.scripts(),
            }),
        }
    }

    /// Fields of a request sub-tab, top to bottom.
    pub fn fields(&self, tab: usize) -> Vec<Field> {
        let auth_fields = |out: &mut Vec<Field>| {
            out.push(Field::AuthKind);
            match self.auth_kind {
                AuthKind::None => {}
                AuthKind::Basic => out.extend([Field::AuthUser, Field::AuthPass]),
                AuthKind::Bearer => out.push(Field::AuthToken),
                AuthKind::OAuth => out.push(Field::AuthOAuthName),
                AuthKind::ApiKey => out.extend([Field::AuthKey, Field::AuthValue, Field::AuthIn]),
            }
        };
        let mut v = Vec::new();
        match self.kind {
            Kind::Http => match tab {
                0 => v.push(Field::Params),
                1 => v.push(Field::Headers),
                2 => {
                    v.push(Field::BodyKind);
                    match self.body_kind {
                        BodyKind::None => {}
                        BodyKind::Json => v.push(Field::BodyText),
                        BodyKind::Raw => v.extend([Field::RawContentType, Field::BodyText]),
                        BodyKind::Form => v.push(Field::FormFields),
                        BodyKind::Multipart => v.push(Field::Parts),
                        BodyKind::File => v.extend([Field::FilePath, Field::FileContentType]),
                    }
                }
                3 => auth_fields(&mut v),
                4 => v.extend([Field::ScriptSection, self.script_table()]),
                _ => v.extend([
                    Field::Timeout,
                    Field::FollowRedirects,
                    Field::VerifyTls,
                    Field::Cookies,
                ]),
            },
            Kind::Grpc => match tab {
                0 => v.push(Field::Message),
                1 => v.push(Field::Metadata),
                2 => auth_fields(&mut v),
                3 => v.extend([Field::ScriptSection, self.script_table()]),
                _ => {
                    v.push(Field::SchemaSource);
                    if self.schema_files {
                        v.extend([Field::ProtoFiles, Field::ImportPaths]);
                    }
                    v.extend([Field::Tls, Field::GrpcVerify, Field::Deadline]);
                }
            },
            Kind::Mcp => match tab {
                0 => v.extend([Field::McpServer, Field::McpTool]),
                1 => v.push(Field::Message),
                2 => v.extend([Field::ScriptSection, self.script_table()]),
                _ => v.push(Field::Timeout),
            },
        }
        v
    }

    fn script_table(&self) -> Field {
        match self.script_section {
            1 => Field::ScriptExtract,
            2 => Field::ScriptAssert,
            _ => Field::ScriptPre,
        }
    }

    fn script_options(&self) -> Vec<String> {
        vec![
            format!("Pre-request: set variables ({})", self.pre.values().len()),
            format!("After response: extract ({})", self.extract.values().len()),
            format!(
                "After response: assertions ({})",
                self.asserts.values().len()
            ),
        ]
    }

    pub fn tab_names(&self) -> &'static [&'static str] {
        match self.kind {
            Kind::Http => HTTP_TABS,
            Kind::Grpc => GRPC_TABS,
            Kind::Mcp => MCP_TABS,
        }
    }

    pub fn text_mut(&mut self, f: Field) -> Option<&mut TextEditor> {
        Some(match f {
            Field::BodyText => &mut self.body_text,
            Field::RawContentType => &mut self.raw_ct,
            Field::FilePath => &mut self.file_path,
            Field::FileContentType => &mut self.file_ct,
            Field::AuthUser => &mut self.auth_user,
            Field::AuthPass => &mut self.auth_pass,
            Field::AuthToken => &mut self.auth_token,
            Field::AuthOAuthName => &mut self.auth_oauth_name,
            Field::AuthKey => &mut self.auth_key,
            Field::AuthValue => &mut self.auth_value,
            Field::Timeout => &mut self.timeout,
            Field::Message => &mut self.message,
            Field::ProtoFiles => &mut self.proto_files,
            Field::ImportPaths => &mut self.import_paths,
            Field::Deadline => &mut self.deadline,
            Field::McpServer => &mut self.mcp_server,
            Field::McpTool => &mut self.mcp_tool,
            _ => return None,
        })
    }

    pub fn table_mut(&mut self, f: Field) -> Option<&mut Table> {
        Some(match f {
            Field::Params => &mut self.params,
            Field::Headers => &mut self.headers,
            Field::FormFields => &mut self.form,
            Field::Parts => &mut self.parts,
            Field::ScriptPre => &mut self.pre,
            Field::ScriptExtract => &mut self.extract,
            Field::ScriptAssert => &mut self.asserts,
            Field::Metadata => &mut self.metadata,
            _ => return None,
        })
    }

    /// Current value label of a choice or toggle field.
    pub fn choice_label(&self, f: Field) -> String {
        match f {
            Field::BodyKind => self.body_kind.label().into(),
            Field::AuthKind => self.auth_kind.label().into(),
            Field::AuthIn => match self.auth_in {
                ApiKeyLocation::Header => "Header".into(),
                ApiKeyLocation::Query => "Query parameter".into(),
            },
            Field::SchemaSource => {
                if self.schema_files {
                    "Proto files".into()
                } else {
                    "Server reflection".into()
                }
            }
            Field::ScriptSection => self
                .script_options()
                .get(self.script_section)
                .cloned()
                .unwrap_or_default(),
            Field::FollowRedirects => onoff(self.follow),
            Field::VerifyTls => onoff(self.verify),
            Field::Cookies => match self.cookies {
                None => "Config default".into(),
                Some(true) => "On".into(),
                Some(false) => "Off".into(),
            },
            Field::Tls => onoff(self.tls),
            Field::GrpcVerify => onoff(self.grpc_verify),
            _ => String::new(),
        }
    }

    pub fn choice_options(&self, f: Field) -> Vec<String> {
        match f {
            Field::BodyKind => BodyKind::ALL
                .iter()
                .map(|k| k.label().to_string())
                .collect(),
            Field::AuthKind => AuthKind::ALL
                .iter()
                .filter(|k| self.kind == Kind::Http || **k != AuthKind::OAuth)
                .map(|k| k.label().to_string())
                .collect(),
            Field::AuthIn => vec!["Header".into(), "Query parameter".into()],
            Field::SchemaSource => vec!["Server reflection".into(), "Proto files".into()],
            Field::ScriptSection => self.script_options(),
            Field::Cookies => vec!["Config default".into(), "On".into(), "Off".into()],
            _ => vec![],
        }
    }

    /// Cycles a choice (dir = ±1) or flips a toggle. Returns true if changed.
    pub fn cycle(&mut self, f: Field, dir: isize) -> bool {
        let step = |i: usize, n: usize| ((i as isize + dir).rem_euclid(n as isize)) as usize;
        match f {
            Field::BodyKind => {
                let i = BodyKind::ALL
                    .iter()
                    .position(|k| *k == self.body_kind)
                    .unwrap_or(0);
                self.set_body_kind(BodyKind::ALL[step(i, BodyKind::ALL.len())]);
            }
            Field::AuthKind => {
                let i = AuthKind::ALL
                    .iter()
                    .position(|k| *k == self.auth_kind)
                    .unwrap_or(0);
                let len = if self.kind == Kind::Http {
                    AuthKind::ALL.len()
                } else {
                    AuthKind::ALL.len() - 1
                };
                self.auth_kind = AuthKind::ALL[step(i, len)];
            }
            Field::AuthIn => {
                self.auth_in = match self.auth_in {
                    ApiKeyLocation::Header => ApiKeyLocation::Query,
                    ApiKeyLocation::Query => ApiKeyLocation::Header,
                }
            }
            Field::SchemaSource => self.schema_files = !self.schema_files,
            Field::ScriptSection => self.script_section = step(self.script_section, 3),
            Field::FollowRedirects => self.follow = !self.follow,
            Field::VerifyTls => self.verify = !self.verify,
            Field::Cookies => {
                let index = match self.cookies {
                    None => 0,
                    Some(true) => 1,
                    Some(false) => 2,
                };
                self.cookies = match step(index, 3) {
                    0 => None,
                    1 => Some(true),
                    _ => Some(false),
                };
            }
            Field::Tls => self.tls = !self.tls,
            Field::GrpcVerify => self.grpc_verify = !self.grpc_verify,
            _ => return false,
        }
        true
    }

    pub fn set_choice(&mut self, f: Field, index: usize) {
        match f {
            Field::BodyKind => {
                if let Some(k) = BodyKind::ALL.get(index) {
                    self.set_body_kind(*k);
                }
            }
            Field::AuthKind => {
                if let Some(k) = AuthKind::ALL.get(index) {
                    self.auth_kind = *k;
                }
            }
            Field::AuthIn => {
                self.auth_in = if index == 1 {
                    ApiKeyLocation::Query
                } else {
                    ApiKeyLocation::Header
                }
            }
            Field::SchemaSource => self.schema_files = index == 1,
            Field::ScriptSection => self.script_section = index.min(2),
            Field::Cookies => {
                self.cookies = match index {
                    1 => Some(true),
                    2 => Some(false),
                    _ => None,
                }
            }
            _ => {}
        }
    }

    pub fn set_body_kind(&mut self, k: BodyKind) {
        self.body_kind = k;
        self.body_text.highlight = if k == BodyKind::Json {
            Highlight::Json
        } else {
            Highlight::None
        };
    }

    /// Headers Sendito will add from the body and auth settings (shown under Headers).
    pub fn auto_headers(&self) -> Vec<(String, String)> {
        let mut v = Vec::new();
        let has = |n: &str| {
            kv_rows(&self.headers)
                .iter()
                .any(|h| h.enabled && h.name.eq_ignore_ascii_case(n))
        };
        let ct = match self.body_kind {
            BodyKind::Json => Some("application/json".to_string()),
            BodyKind::Raw => Some(self.raw_ct.text()).filter(|s| !s.is_empty()),
            BodyKind::Form => Some("application/x-www-form-urlencoded".to_string()),
            BodyKind::Multipart => Some("multipart/form-data; boundary=…".to_string()),
            BodyKind::File => Some(if self.file_ct.text().is_empty() {
                "application/octet-stream".to_string()
            } else {
                self.file_ct.text()
            }),
            BodyKind::None => None,
        };
        if let Some(ct) = ct
            && (!has("content-type") || self.body_kind == BodyKind::Multipart)
        {
            v.push(("Content-Type".into(), ct));
        }
        match self.auth_kind {
            AuthKind::Basic if !has("authorization") => v.push((
                "Authorization".into(),
                "Basic base64(username:password)".into(),
            )),
            AuthKind::Bearer if !has("authorization") => v.push((
                "Authorization".into(),
                format!("Bearer {}", self.auth_token.text()),
            )),
            AuthKind::OAuth if !has("authorization") => v.push((
                "Authorization".into(),
                "Bearer •••••• (OAuth keychain)".into(),
            )),
            AuthKind::ApiKey
                if self.auth_in == ApiKeyLocation::Header && !has(&self.auth_key.text()) =>
            {
                v.push((self.auth_key.text(), self.auth_value.text()))
            }
            _ => {}
        }
        v
    }

    /// Applies vim mode to every editor (normal mode when enabled).
    pub fn set_vim(&mut self, on: bool) {
        let mode = if on {
            crate::editor::VimMode::Normal
        } else {
            crate::editor::VimMode::Insert
        };
        for e in [
            &mut self.url,
            &mut self.body_text,
            &mut self.raw_ct,
            &mut self.file_path,
            &mut self.file_ct,
            &mut self.auth_user,
            &mut self.auth_pass,
            &mut self.auth_token,
            &mut self.auth_oauth_name,
            &mut self.auth_key,
            &mut self.auth_value,
            &mut self.timeout,
            &mut self.target,
            &mut self.message,
            &mut self.deadline,
            &mut self.proto_files,
            &mut self.import_paths,
        ] {
            e.vim = mode;
        }
        for t in [
            &mut self.params,
            &mut self.headers,
            &mut self.form,
            &mut self.parts,
            &mut self.pre,
            &mut self.extract,
            &mut self.asserts,
            &mut self.metadata,
        ] {
            for r in &mut t.rows {
                for c in &mut r.cells {
                    if let Cell::Text(e) = c {
                        e.vim = mode;
                    }
                }
            }
        }
    }
}

fn onoff(b: bool) -> String {
    if b { "On".into() } else { "Off".into() }
}

/// Summary of the last response for the status line and response header.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    /// e.g. `200 OK` or `OK (0)` for gRPC; empty while waiting.
    pub status: String,
    pub code: Option<u16>,
    pub ok: bool,
    pub time_ms: Option<f64>,
    pub size: Option<u64>,
    /// e.g. `gzip 1.2 KB`: the body arrived with this Content-Encoding and was
    /// decoded; `size` is the decoded size.
    pub decoded: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Response {
    pub summary: Summary,
    pub body: Viewer,
    pub headers: Viewer,
    pub timing: Viewer,
    pub messages: Viewer,
    pub grpc_events: Vec<sendito_grpc::Event>,
    pub grpc_kind: Option<sendito_grpc::CallKind>,
    /// Short outcome of the assertions, shown on the Results tab: "2/3 ✗", "3/3 ✓".
    pub results_badge: Option<String>,
    /// The jq expression filtering the body for display, if any.
    pub jq: Option<String>,
    /// The unfiltered body bytes, captured when the first filter is applied.
    pub jq_src: Option<Vec<u8>>,
}

impl Response {
    pub fn new() -> Self {
        Self {
            summary: Summary::default(),
            body: Viewer::empty(),
            headers: wrapped(Doc::text("")),
            timing: wrapped(Doc::text("")),
            messages: wrapped(Doc::text("")),
            grpc_events: Vec::new(),
            grpc_kind: None,
            results_badge: None,
            jq: None,
            jq_src: None,
        }
    }

    /// Replaces the body document (a response, message or call result); any
    /// jq filter no longer applies to the new document.
    pub fn set_body(&mut self, doc: Doc) {
        self.jq = None;
        self.jq_src = None;
        self.body = Viewer::new(doc);
    }

    /// Replaces the body with wrapped text (a failure report); any jq filter
    /// no longer applies to it.
    pub fn set_body_wrapped(&mut self, text: String) {
        self.set_body(Doc::text(&text));
        self.body.wrap = true;
    }

    /// Applies (or, with an empty expression, removes) the jq filter over the
    /// body and rebuilds the viewer. The original bytes are captured when the
    /// first filter succeeds, so a new expression re-filters the original
    /// body and clearing restores it.
    ///
    /// Returns whether a filter is active afterwards.
    pub fn apply_jq(&mut self, expr: &str) -> Result<bool, String> {
        let expr = expr.trim();
        if expr.is_empty() {
            self.jq = None;
            if let Some(src) = self.jq_src.take() {
                self.swap_body(Doc::from_bytes(src, Some("application/json")));
            }
            return Ok(false);
        }
        let outs = match self.jq_src.as_deref() {
            Some(src) => crate::filter::apply(expr, src),
            None => {
                // Capture the original bytes only when the filter succeeds,
                // so a failed first filter leaves no half-state behind.
                let src = self.body.doc.bytes().to_vec();
                let outs = crate::filter::apply(expr, &src);
                if outs.is_ok() {
                    self.jq_src = Some(src);
                }
                outs
            }
        };
        let outs = outs?;
        self.jq = Some(expr.to_string());
        let doc = match outs.as_slice() {
            [] => Doc::text("(no output)\n"),
            [one] => Doc::from_bytes(one.clone(), Some("application/json")),
            many => {
                // One JSON value per line, like jq's own output stream.
                let mut text = Vec::new();
                for (i, o) in many.iter().enumerate() {
                    if i > 0 {
                        text.push(b'\n');
                    }
                    text.extend_from_slice(o);
                }
                Doc::from_bytes(text, None)
            }
        };
        self.swap_body(doc);
        Ok(true)
    }

    /// Swaps the body viewer's document, keeping the view mode, wrap and line
    /// numbers.
    fn swap_body(&mut self, doc: Doc) {
        let mut v = Viewer::new(doc);
        v.mode = self.body.mode;
        v.wrap = self.body.wrap;
        v.numbers = self.body.numbers;
        self.body = v;
    }

    pub fn viewer_mut(&mut self, tab: usize) -> &mut Viewer {
        match tab {
            0 => &mut self.body,
            1 => &mut self.headers,
            2 => &mut self.timing,
            _ => &mut self.messages,
        }
    }
}

/// A viewer that wraps long lines (used for text panes).
pub fn wrapped(doc: Doc) -> Viewer {
    let mut v = Viewer::new(doc);
    v.wrap = true;
    v
}

/// The path of a URL (without scheme, host or a leading `{{base}}` variable, and
/// without query); the whole URL when it has no path.
pub fn url_path(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = if rest.starts_with("{{") {
        rest.split_once("}}").map_or(rest, |(_, r)| r)
    } else {
        rest.find('/').map_or("", |i| &rest[i..])
    };
    let path = rest.split(['?', '#']).next().unwrap_or("");
    if path.is_empty() || path == "/" {
        url.to_string()
    } else {
        path.to_string()
    }
}

/// Formats milliseconds: one decimal below 10 ms.
pub fn fmt_ms(ms: f64) -> String {
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{} ms", ms.round() as u64)
    }
}

impl Default for Response {
    fn default() -> Self {
        Self::new()
    }
}

/// An in-flight request.
pub struct Running {
    pub task: tokio::task::JoinHandle<()>,
    pub control: Option<tokio::sync::mpsc::UnboundedSender<crate::runner::GrpcControl>>,
    pub started: std::time::Instant,
    /// The call accepts more request messages (client/bidi streaming).
    pub streaming_input: bool,
}

/// Watch (repeat) state of a tab: the request is sent again on this interval
/// while the tab is active and idle. Stopped by the watch key again, cancelling,
/// closing the tab or changing the environment.
#[derive(Debug, Clone)]
pub struct Watch {
    pub interval: std::time::Duration,
    /// When the next repeat is due.
    pub next: std::time::Instant,
    /// One `(ok, ms)` per send while watching, oldest first.
    pub samples: Vec<(bool, f64)>,
}

/// How many watch samples are kept for the sparkline.
pub const WATCH_SAMPLES: usize = 120;

impl Watch {
    pub fn new(interval: std::time::Duration) -> Self {
        Self {
            interval,
            next: std::time::Instant::now() + interval,
            samples: Vec::new(),
        }
    }

    pub fn record(&mut self, ok: bool, ms: f64) {
        self.samples.push((ok, ms));
        let n = self.samples.len();
        if n > WATCH_SAMPLES {
            self.samples.drain(..n - WATCH_SAMPLES);
        }
    }
}

pub struct Tab {
    pub id: u64,
    pub path: Option<String>,
    pub name: String,
    pub saved: Option<Request>,
    pub ed: Editors,
    pub req_tab: usize,
    pub resp_tab: usize,
    pub field: usize,
    pub response: Option<Response>,
    pub running: Option<Running>,
    pub watch: Option<Watch>,
    pub services: Option<Vec<ServiceInfo>>,
    pub mcp_tools: Vec<sendito_mcp::Tool>,
    /// The server `mcp_tools` was loaded from; the list is stale for any other.
    pub mcp_tools_server: String,
    /// Why listing `mcp_tools_server`'s tools failed; completion does not
    /// retry a dead server by itself.
    pub mcp_tools_error: Option<String>,
    pub schema_error: Option<String>,
    /// Cached model built from the editors (refreshed after each change).
    pub model: Request,
    /// Where an unsaved tab came from, shown in the request header
    /// (e.g. "history entry of 12:46:51").
    pub origin: Option<String>,
    /// The saved request a history copy came from; its history entries and
    /// "save to tree" default to that folder.
    pub origin_path: Option<String>,
}

impl Tab {
    pub fn new(
        id: u64,
        path: Option<String>,
        name: String,
        req: &Request,
        saved: Option<Request>,
    ) -> Self {
        let ed = Editors::from_request(req);
        Self {
            id,
            path,
            name,
            saved,
            ed,
            req_tab: 0,
            resp_tab: 0,
            field: 0,
            response: None,
            running: None,
            watch: None,
            services: None,
            mcp_tools: Vec::new(),
            mcp_tools_server: String::new(),
            mcp_tools_error: None,
            schema_error: None,
            model: req.clone(),
            origin: None,
            origin_path: None,
        }
    }

    pub fn refresh_model(&mut self) {
        self.model = self.ed.to_request();
    }

    pub fn dirty(&self) -> bool {
        match &self.saved {
            Some(s) => *s != self.model,
            None => true,
        }
    }

    /// The name shown for the tab (on the tab, in the header and in dialogs):
    /// stable while the request is edited; a new request is "untitled N".
    pub fn title(&self) -> String {
        self.name.clone()
    }

    pub fn badge(&self) -> String {
        self.model.badge().to_string()
    }

    pub fn fields(&self) -> Vec<Field> {
        self.ed.fields(self.req_tab)
    }

    pub fn current_field(&self) -> Option<Field> {
        self.fields().get(self.field).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewer::ViewMode;

    #[test]
    fn jq_filter_state() {
        let body = br#"{"items":[{"id":1},{"id":2}],"count":2}"#;
        let mut r = Response::new();
        r.set_body(Doc::from_bytes(body.to_vec(), Some("application/json")));
        // A filter replaces the shown document and remembers the expression.
        assert!(r.apply_jq(".count").unwrap());
        assert_eq!(r.jq.as_deref(), Some(".count"));
        assert_eq!(r.body.doc.line(ViewMode::Pretty, 0), "2");
        // A new expression re-filters the original body, not the output.
        assert!(r.apply_jq(".items[0].id").unwrap());
        assert_eq!(r.body.doc.line(ViewMode::Pretty, 0), "1");
        // Clearing restores the original bytes and drops the state.
        assert!(!r.apply_jq("").unwrap());
        assert!(r.jq.is_none() && r.jq_src.is_none());
        assert_eq!(r.body.doc.bytes(), &body[..]);
        // An empty filter with none applied is a no-op.
        assert!(!r.apply_jq("").unwrap());
        // A replaced body drops a pending filter.
        assert!(r.apply_jq(".count").unwrap());
        r.set_body(Doc::text("x"));
        assert!(r.jq.is_none() && r.jq_src.is_none());
        // A bad expression changes nothing.
        assert!(r.apply_jq("nope").is_err());
        assert!(r.jq.is_none());
    }

    fn full() -> Request {
        Request::Http(HttpRequest {
            method: "POST".into(),
            url: "{{base}}/x".into(),
            params: vec![KeyValue::new("a", "1"), KeyValue::new("b", "2").disabled()],
            headers: vec![KeyValue::new("X", "y")],
            body: Body::Multipart {
                parts: vec![
                    MultipartPart {
                        name: "t".into(),
                        kind: PartKind::Text { value: "v".into() },
                        content_type: String::new(),
                        enabled: true,
                    },
                    MultipartPart {
                        name: "f".into(),
                        kind: PartKind::File {
                            path: "a.png".into(),
                            filename: Some("b.png".into()),
                        },
                        content_type: "image/png".into(),
                        enabled: false,
                    },
                ],
            },
            auth: Auth::ApiKey {
                key: "k".into(),
                value: "v".into(),
                location: ApiKeyLocation::Query,
            },
            options: HttpOptions {
                timeout_ms: Some(10),
                follow_redirects: false,
                verify_tls: false,
                cookies: None,
            },
            scripts: Scripts {
                pre: vec![SetVar {
                    var: "a".into(),
                    value: "b".into(),
                }],
                extract: vec![Extract {
                    var: "t".into(),
                    source: Source::Header,
                    expr: "x".into(),
                    secret: true,
                }],
                asserts: vec![Assertion {
                    source: Source::Status,
                    expr: String::new(),
                    op: AssertOp::Lt,
                    value: "300".into(),
                }],
            },
        })
    }

    #[test]
    fn roundtrip_through_editors() {
        let bodies = [
            Body::None,
            Body::Json {
                text: "{\n  \"a\": 1\n}".into(),
            },
            Body::Raw {
                content_type: "text/xml".into(),
                text: "<a/>".into(),
            },
            Body::Form {
                fields: vec![KeyValue::new("a", "b")],
            },
            Body::File {
                path: "f.bin".into(),
                content_type: String::new(),
            },
        ];
        let mut reqs = vec![full()];
        for b in bodies {
            for auth in [
                Auth::None,
                Auth::Basic {
                    username: "u".into(),
                    password: "p".into(),
                },
                Auth::Bearer { token: "t".into() },
                Auth::OAuth {
                    name: "identity".into(),
                },
            ] {
                reqs.push(Request::Http(HttpRequest {
                    body: b.clone(),
                    auth,
                    ..Default::default()
                }));
            }
        }
        reqs.push(Request::Grpc(GrpcRequest {
            target: "h:1".into(),
            service: "s.S".into(),
            method: "M".into(),
            message: "{}".into(),
            metadata: vec![KeyValue::new("k", "v")],
            tls: true,
            verify_tls: false,
            deadline_ms: Some(5),
            schema: SchemaSource::Files {
                files: vec!["a.proto".into(), "b.proto".into()],
                import_paths: vec!["p".into()],
            },
            ..Default::default()
        }));
        reqs.push(Request::new_grpc());
        for r in reqs {
            let e = Editors::from_request(&r);
            assert_eq!(e.to_request(), r);
        }
    }

    #[test]
    fn fields_and_choices() {
        let mut e = Editors::from_request(&full());
        assert_eq!(e.fields(2), vec![Field::BodyKind, Field::Parts]);
        assert_eq!(
            e.fields(3),
            vec![
                Field::AuthKind,
                Field::AuthKey,
                Field::AuthValue,
                Field::AuthIn
            ]
        );
        assert!(e.cycle(Field::BodyKind, 1));
        assert_eq!(e.body_kind, BodyKind::File);
        e.set_choice(Field::BodyKind, 1);
        assert_eq!(e.body_kind, BodyKind::Json);
        assert_eq!(e.choice_label(Field::BodyKind), "JSON");
        assert_eq!(e.choice_options(Field::AuthKind).len(), 5);
        assert!(e.cycle(Field::FollowRedirects, 1));
        assert_eq!(e.choice_label(Field::FollowRedirects), "On");
        assert!(!e.cycle(Field::Params, 1));
        e.set_choice(Field::AuthIn, 0);
        let auto = e.auto_headers();
        assert!(auto.iter().any(|(n, _)| n == "Content-Type"));
        assert!(auto.iter().any(|(n, _)| n == "k"));
        e.set_vim(true);
        assert!(e.text_mut(Field::BodyText).is_some());
        assert!(e.table_mut(Field::Params).is_some());
        assert!(e.text_mut(Field::Params).is_none());
        for f in [
            Field::Params,
            Field::BodyText,
            Field::BodyKind,
            Field::Tls,
            Field::Cookies,
        ] {
            assert!(!f.label().is_empty());
        }
        // The cookie jar option cycles: default → on → off → default.
        assert_eq!(e.choice_label(Field::Cookies), "Config default");
        assert!(e.cycle(Field::Cookies, 1));
        assert_eq!(e.choice_label(Field::Cookies), "On");
        assert!(e.cycle(Field::Cookies, 1));
        assert_eq!(e.choice_label(Field::Cookies), "Off");
        assert!(e.cycle(Field::Cookies, 1));
        assert_eq!(e.choice_label(Field::Cookies), "Config default");
        e.cookies = Some(true);
        let Request::Http(h) = e.to_request() else {
            panic!("not http")
        };
        assert_eq!(h.options.cookies, Some(true));
        let mut tab = Tab::new(1, None, "x".into(), &full(), Some(full()));
        tab.refresh_model();
        assert!(!tab.dirty());
        tab.ed.url.set_text("changed");
        tab.refresh_model();
        assert!(tab.dirty());
        assert_eq!(tab.badge(), "POST");
        assert_eq!(tab.current_field(), Some(Field::Params));
        let mut r = Response::new();
        r.viewer_mut(3);
    }
}
