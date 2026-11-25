//! The editor widgets of a request and their conversion to and from the model.

use super::*;

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
    pub graphql_variables: TextEditor,
    pub graphql_operation: TextEditor,
    pub graphql_section: usize,
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
    pub max_events: TextEditor,
    pub schema_files: bool,
    pub proto_files: TextEditor,
    pub import_paths: TextEditor,
    pub mcp_server: TextEditor,
    pub mcp_tool: TextEditor,
    pub ws_subprotocols: TextEditor,
    /// A folder's variables (Kind::Folder only).
    pub vars: Table,
    pub aws_access: TextEditor,
    pub aws_secret: TextEditor,
    pub aws_token: TextEditor,
    pub aws_region: TextEditor,
    pub aws_service: TextEditor,
    pub jwt_alg: JwtAlgorithm,
    pub jwt_secret: TextEditor,
    pub jwt_key: TextEditor,
    pub jwt_claims: TextEditor,
    pub jwt_expires: TextEditor,
    /// Scripts tab section: 0 pre-request, 1 extract, 2 assertions.
    pub script_section: usize,
    /// MQTT subscriptions: topic filter and QoS.
    pub mqtt_subs: Table,
    /// MQTT publishes: topic, payload, QoS, retain and base64.
    pub mqtt_publish: Table,
    pub will_topic: TextEditor,
    pub will_payload: TextEditor,
    pub will_qos: u8,
    pub will_retain: bool,
    pub will_base64: bool,
    pub mqtt_user: TextEditor,
    pub mqtt_pass: TextEditor,
    pub mqtt_client_id: TextEditor,
    pub mqtt_protocol: MqttProtocol,
    pub clean_session: bool,
    pub keep_alive: TextEditor,
    /// The Markdown description (Docs tab).
    pub description: TextEditor,
    /// The Docs tab shows the rendered description, not the text.
    pub docs_preview: bool,
    /// The first line of the rendered description on screen.
    pub docs_scroll: usize,
}

/// A folder tab keeps its settings in an HTTP request, so the tab's model,
/// saved state and editors work as for a request: the headers, auth and
/// scripts and description as they are, the variables in `params`.
pub fn folder_request(f: &FolderSettings) -> Request {
    Request::Http(HttpRequest {
        params: f.vars.clone(),
        headers: f.headers.clone(),
        auth: f.auth.clone(),
        scripts: f.scripts.clone(),
        description: f.description.clone(),
        ..Default::default()
    })
}

/// The folder settings a [`folder_request`] holds.
pub fn request_folder(req: &Request) -> FolderSettings {
    match req {
        Request::Http(h) => FolderSettings {
            headers: h.headers.clone(),
            auth: h.auth.clone(),
            vars: h.params.clone(),
            scripts: h.scripts.clone(),
            description: h.description.clone(),
        },
        _ => FolderSettings::default(),
    }
}

/// The views of the Docs tab, in `docs_preview` order.
const DOCS_VIEWS: [&str; 2] = ["Edit", "Preview"];

pub(super) fn ms_text(v: Option<u64>) -> String {
    v.map(|v| v.to_string()).unwrap_or_default()
}

pub(super) fn parse_ms(s: &str) -> Option<u64> {
    s.trim().parse().ok()
}

pub(super) fn list_text(v: &[String]) -> String {
    v.join(", ")
}

pub(super) fn parse_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// `Service/Method` as shown in the method field.
pub(super) fn grpc_method_label(service: &str, method: &str) -> String {
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
pub(super) fn split_method(text: &str) -> (String, String) {
    let text = text.trim();
    match text.rsplit_once('/') {
        Some((s, m)) => (s.trim().to_string(), m.trim().to_string()),
        None => (text.to_string(), String::new()),
    }
}

pub(super) fn kv_table(name: &'static str, value: &'static str, rows: &[KeyValue]) -> Table {
    let mut t = Table::kv(name, value);
    for kv in rows {
        t.push_texts(kv.enabled, &[&kv.name, &kv.value]);
    }
    t
}

pub(super) fn kv_rows(t: &Table) -> Vec<KeyValue> {
    t.values()
        .into_iter()
        .map(|r| KeyValue {
            name: r.cells[0].text(),
            value: r.cells[1].text(),
            enabled: r.enabled,
        })
        .collect()
}

pub(super) fn t(s: &str) -> Cell {
    Cell::Text(TextEditor::single(s))
}

fn qos_column() -> Column {
    Column {
        title: "QoS",
        kind: ColKind::Choice(QOS),
        weight: 1,
    }
}

fn mqtt_subs_table() -> Table {
    Table::new(
        vec![
            Column {
                title: "Topic filter",
                kind: ColKind::Text,
                weight: 5,
            },
            qos_column(),
        ],
        true,
    )
}

fn mqtt_publish_table() -> Table {
    Table::new(
        vec![
            Column {
                title: "Topic",
                kind: ColKind::Text,
                weight: 3,
            },
            Column {
                title: "Payload",
                kind: ColKind::Text,
                weight: 5,
            },
            qos_column(),
            Column {
                title: "Retain",
                kind: ColKind::Toggle,
                weight: 1,
            },
            Column {
                title: "Base64",
                kind: ColKind::Toggle,
                weight: 1,
            },
        ],
        false,
    )
}

/// The message of one Publish table row.
pub fn mqtt_publish_row(r: &Row) -> MqttMessage {
    MqttMessage {
        topic: r.cells[0].text(),
        payload: MqttPayload {
            text: r.cells[1].text(),
            base64: r.cells[4].toggle(),
        },
        qos: r.cells[2].choice().min(2) as u8,
        retain: r.cells[3].toggle(),
    }
}

impl Editors {
    /// The request bar's text editor. Every kind draws the same strip
    /// `[picker chip][endpoint input]` (gRPC and MCP draw `[input][chip]`):
    /// part 1 is the endpoint input (URL, target or server), part 0 the
    /// inline-editable chip (gRPC method, MCP tool).
    pub fn bar_text(&mut self, part: usize) -> Option<&mut TextEditor> {
        match (self.kind, part) {
            (Kind::Http, 1) | (Kind::Ws, 1) | (Kind::Sse, 1) | (Kind::Mqtt, 1) => {
                Some(&mut self.url)
            }
            (Kind::Grpc, 1) => Some(&mut self.target),
            (Kind::Mcp, 1) => Some(&mut self.mcp_server),
            (Kind::Grpc, 0) => Some(&mut self.grpc_method_text),
            (Kind::Mcp, 0) => Some(&mut self.mcp_tool),
            _ => None,
        }
    }
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
            graphql_variables: TextEditor::multi("").with_highlight(Highlight::Json),
            graphql_operation: TextEditor::single(""),
            graphql_section: 0,
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
            auth_kind: AuthKind::Inherit,
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
            max_events: TextEditor::single("").with_placeholder("none"),
            schema_files: false,
            proto_files: TextEditor::single("").with_placeholder("protos/service.proto"),
            import_paths: TextEditor::single("").with_placeholder("protos"),
            mcp_server: TextEditor::single("")
                .with_placeholder("server name from [mcp.servers] or http(s):// URL"),
            mcp_tool: TextEditor::single("").with_placeholder("tool name"),
            ws_subprotocols: TextEditor::single("").with_placeholder("chat.v2, chat"),
            vars: Table::kv("Variable", "Value"),
            aws_access: TextEditor::single("").with_placeholder("{{aws_access_key_id}}"),
            aws_secret: TextEditor::single("").with_placeholder("{{aws_secret_access_key}}"),
            aws_token: TextEditor::single("").with_placeholder("empty for long-term keys"),
            aws_region: TextEditor::single("us-east-1"),
            aws_service: TextEditor::single("").with_placeholder("execute-api"),
            jwt_alg: JwtAlgorithm::Hs256,
            jwt_secret: TextEditor::single("").with_placeholder("{{jwt_secret}}"),
            jwt_key: TextEditor::single("").with_placeholder("keys/private.pem"),
            jwt_claims: TextEditor::multi("{}").with_highlight(Highlight::Json),
            jwt_expires: TextEditor::single("").with_placeholder("none"),
            script_section: 0,
            mqtt_subs: mqtt_subs_table(),
            mqtt_publish: mqtt_publish_table(),
            will_topic: TextEditor::single("").with_placeholder("none"),
            will_payload: TextEditor::multi(""),
            will_qos: 0,
            will_retain: false,
            will_base64: false,
            mqtt_user: TextEditor::single("").with_placeholder("from Basic auth"),
            mqtt_pass: TextEditor::single(""),
            mqtt_client_id: TextEditor::single("").with_placeholder("sendito-<8 hex>"),
            mqtt_protocol: MqttProtocol::default(),
            clean_session: true,
            keep_alive: TextEditor::single(&MQTT_KEEP_ALIVE_S.to_string()),
            description: TextEditor::multi("")
                .with_placeholder("Markdown: # Title, **bold**, - list, `code`, [link](url)"),
            docs_preview: false,
            docs_scroll: 0,
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
                    Body::GraphQL {
                        query,
                        variables,
                        operation_name,
                    } => {
                        e.body_text.set_text(query);
                        e.body_text.highlight = Highlight::None;
                        e.graphql_variables.set_text(variables);
                        e.graphql_operation.set_text(operation_name);
                    }
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
            Request::Sse(s) => {
                e = Self::from_request(&Request::Http(s.http_request()));
                e.kind = Kind::Sse;
                e.deadline.set_text(&ms_text(s.options.deadline_ms));
                e.max_events.set_text(&ms_text(s.options.max_events));
            }
            Request::Ws(w) => {
                e.kind = Kind::Ws;
                e.url.placeholder = "ws://host:port/path  (wss:// for TLS)".into();
                e.url.set_text(&w.url);
                e.headers = kv_table("Header", "Value", &w.headers);
                e.ws_subprotocols.set_text(&list_text(&w.subprotocols));
                e.message.set_text(&w.messages);
                e.timeout.set_text(&ms_text(w.options.timeout_ms));
                e.deadline.set_text(&ms_text(w.options.deadline_ms));
                e.tls = w.options.tls;
                e.grpc_verify = w.options.verify_tls;
                e.cookies = w.options.cookies;
            }
            Request::Mqtt(m) => {
                e.kind = Kind::Mqtt;
                e.url.placeholder = "mqtt://host:1883  (mqtts:// for TLS)".into();
                e.url.set_text(&m.url);
                for sub in &m.subscriptions {
                    e.mqtt_subs.push(
                        sub.enabled,
                        vec![t(&sub.topic), Cell::Choice(usize::from(sub.qos.min(2)))],
                    );
                }
                for p in &m.publish {
                    e.mqtt_publish.push(
                        true,
                        vec![
                            t(&p.topic),
                            t(&p.payload.text),
                            Cell::Choice(usize::from(p.qos.min(2))),
                            Cell::Toggle(p.retain),
                            Cell::Toggle(p.payload.base64),
                        ],
                    );
                }
                if let Some(w) = &m.last_will {
                    e.will_topic.set_text(&w.topic);
                    e.will_payload.set_text(&w.payload.text);
                    e.will_qos = w.qos.min(2);
                    e.will_retain = w.retain;
                    e.will_base64 = w.payload.base64;
                }
                e.mqtt_user.set_text(&m.username);
                e.mqtt_pass.set_text(&m.password);
                e.mqtt_client_id.set_text(&m.client_id);
                e.mqtt_protocol = m.protocol;
                e.clean_session = m.clean_session;
                e.keep_alive.set_text(&m.keep_alive_s.to_string());
                e.timeout.set_text(&ms_text(m.options.timeout_ms));
                e.deadline.set_text(&ms_text(m.options.deadline_ms));
                e.verify = m.options.verify_tls;
            }
            Request::Mcp(m) => {
                e.kind = Kind::Mcp;
                e.mcp_server.set_text(&m.server);
                e.mcp_tool.set_text(&m.tool);
                e.message.set_text(&m.arguments);
                e.timeout.set_text(&ms_text(m.timeout_ms));
            }
        }
        e.description.set_text(req.description());
        // A written description opens rendered, as a page to read.
        e.docs_preview = !req.description().is_empty();
        e
    }

    /// The editors of a folder's settings.
    pub fn from_folder(f: &FolderSettings) -> Self {
        let mut e = Self::from_request(&folder_request(f));
        e.kind = Kind::Folder;
        e.vars = kv_table("Variable", "Value", &f.vars);
        e
    }

    fn set_auth(&mut self, a: &Auth) {
        self.auth_kind = a.kind();
        match a {
            Auth::Inherit | Auth::None => {}
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
            Auth::Digest { username, password } => {
                self.auth_user.set_text(username);
                self.auth_pass.set_text(password);
            }
            Auth::AwsSigV4 {
                access_key,
                secret_key,
                session_token,
                region,
                service,
            } => {
                self.aws_access.set_text(access_key);
                self.aws_secret.set_text(secret_key);
                self.aws_token.set_text(session_token);
                self.aws_region.set_text(region);
                self.aws_service.set_text(service);
            }
            Auth::Jwt {
                algorithm,
                secret,
                private_key,
                claims,
                expires_in_s,
            } => {
                self.jwt_alg = *algorithm;
                self.jwt_secret.set_text(secret);
                self.jwt_key.set_text(private_key);
                self.jwt_claims.set_text(claims);
                self.jwt_expires.set_text(&ms_text(*expires_in_s));
            }
        }
    }

    fn auth(&self) -> Auth {
        match self.auth_kind {
            AuthKind::Inherit => Auth::Inherit,
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
            AuthKind::Digest => Auth::Digest {
                username: self.auth_user.text(),
                password: self.auth_pass.text(),
            },
            AuthKind::AwsSigV4 => Auth::AwsSigV4 {
                access_key: self.aws_access.text(),
                secret_key: self.aws_secret.text(),
                session_token: self.aws_token.text(),
                region: self.aws_region.text(),
                service: self.aws_service.text(),
            },
            AuthKind::Jwt => Auth::Jwt {
                algorithm: self.jwt_alg,
                secret: self.jwt_secret.text(),
                private_key: self.jwt_key.text(),
                claims: self.jwt_claims.text(),
                expires_in_s: parse_ms(&self.jwt_expires.text()),
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
                    source: Source::ALL[r.cells[1].choice().min(Source::ALL.len() - 1)],
                    expr: r.cells[2].text(),
                    secret: r.cells[3].toggle(),
                })
                .collect(),
            asserts: self
                .asserts
                .values()
                .into_iter()
                .map(|r| Assertion {
                    source: Source::ALL[r.cells[0].choice().min(Source::ALL.len() - 1)],
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
                    BodyKind::GraphQL => Body::GraphQL {
                        query: self.body_text.text(),
                        variables: self.graphql_variables.text(),
                        operation_name: self.graphql_operation.text(),
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
                    description: self.description.text(),
                })
            }
            Kind::Sse => Request::Sse(SseRequest {
                url: self.url.text(),
                params: kv_rows(&self.params),
                headers: kv_rows(&self.headers),
                auth: self.auth(),
                scripts: self.scripts(),
                description: self.description.text(),
                options: SseOptions {
                    timeout_ms: parse_ms(&self.timeout.text()),
                    deadline_ms: parse_ms(&self.deadline.text()),
                    max_events: parse_ms(&self.max_events.text()),
                    follow_redirects: self.follow,
                    verify_tls: self.verify,
                    cookies: self.cookies,
                },
            }),
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
                description: self.description.text(),
            }),
            Kind::Ws => Request::Ws(WebSocketRequest {
                url: self.url.text(),
                headers: kv_rows(&self.headers),
                subprotocols: parse_list(&self.ws_subprotocols.text()),
                messages: self.message.text(),
                auth: self.auth(),
                options: WsOptions {
                    timeout_ms: parse_ms(&self.timeout.text()),
                    deadline_ms: parse_ms(&self.deadline.text()),
                    verify_tls: self.grpc_verify,
                    tls: self.tls,
                    cookies: self.cookies,
                },
                scripts: self.scripts(),
                description: self.description.text(),
            }),
            Kind::Mqtt => {
                let will_topic = self.will_topic.text();
                let will_payload = self.will_payload.text();
                Request::Mqtt(MqttRequest {
                    url: self.url.text(),
                    client_id: self.mqtt_client_id.text(),
                    username: self.mqtt_user.text(),
                    password: self.mqtt_pass.text(),
                    protocol: self.mqtt_protocol,
                    clean_session: self.clean_session,
                    keep_alive_s: parse_ms(&self.keep_alive.text()).unwrap_or(MQTT_KEEP_ALIVE_S),
                    options: MqttOptions {
                        timeout_ms: parse_ms(&self.timeout.text()),
                        deadline_ms: parse_ms(&self.deadline.text()),
                        verify_tls: self.verify,
                    },
                    subscriptions: self
                        .mqtt_subs
                        .values()
                        .into_iter()
                        .map(|r| MqttSubscription {
                            topic: r.cells[0].text(),
                            qos: r.cells[1].choice().min(2) as u8,
                            enabled: r.enabled,
                        })
                        .collect(),
                    publish: self
                        .mqtt_publish
                        .values()
                        .into_iter()
                        .map(mqtt_publish_row)
                        .collect(),
                    last_will: (!will_topic.is_empty() || !will_payload.is_empty()).then_some(
                        MqttMessage {
                            topic: will_topic,
                            payload: MqttPayload {
                                text: will_payload,
                                base64: self.will_base64,
                            },
                            qos: self.will_qos,
                            retain: self.will_retain,
                        },
                    ),
                    auth: self.auth(),
                    scripts: self.scripts(),
                    description: self.description.text(),
                })
            }
            Kind::Mcp => Request::Mcp(McpRequest {
                server: self.mcp_server.text(),
                tool: self.mcp_tool.text(),
                arguments: self.message.text(),
                timeout_ms: parse_ms(&self.timeout.text()),
                scripts: self.scripts(),
                description: self.description.text(),
            }),
            Kind::Folder => folder_request(&FolderSettings {
                headers: kv_rows(&self.headers),
                auth: self.auth(),
                vars: kv_rows(&self.vars),
                scripts: self.scripts(),
                description: self.description.text(),
            }),
        }
    }

    /// Fields of a request sub-tab, top to bottom.
    pub fn fields(&self, tab: usize) -> Vec<Field> {
        let auth_fields = |out: &mut Vec<Field>| {
            out.push(Field::AuthKind);
            match self.auth_kind {
                AuthKind::Inherit | AuthKind::None => {}
                AuthKind::Basic | AuthKind::Digest => {
                    out.extend([Field::AuthUser, Field::AuthPass])
                }
                AuthKind::Bearer => out.push(Field::AuthToken),
                AuthKind::OAuth => out.push(Field::AuthOAuthName),
                AuthKind::ApiKey => out.extend([Field::AuthKey, Field::AuthValue, Field::AuthIn]),
                AuthKind::AwsSigV4 => out.extend([
                    Field::AwsAccessKey,
                    Field::AwsSecretKey,
                    Field::AwsSessionToken,
                    Field::AwsRegion,
                    Field::AwsService,
                ]),
                AuthKind::Jwt => {
                    out.push(Field::JwtAlgorithm);
                    out.push(if self.jwt_alg.uses_secret() {
                        Field::JwtSecret
                    } else {
                        Field::JwtPrivateKey
                    });
                    out.extend([Field::JwtExpires, Field::JwtClaims]);
                }
            }
        };
        // Docs is the last tab of every kind.
        if tab + 1 == self.tab_names().len() {
            return vec![
                Field::DocsView,
                if self.docs_preview {
                    Field::DocsPreview
                } else {
                    Field::Description
                },
            ];
        }
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
                        BodyKind::GraphQL => v.extend([Field::GraphqlSection, Field::BodyText]),
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
            Kind::Sse => match tab {
                0 => v.push(Field::Params),
                1 => v.push(Field::Headers),
                2 => auth_fields(&mut v),
                3 => v.extend([Field::ScriptSection, self.script_table()]),
                _ => v.extend([
                    Field::Timeout,
                    Field::Deadline,
                    Field::MaxEvents,
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
            Kind::Ws => match tab {
                0 => v.push(Field::Headers),
                1 => v.push(Field::WsMessages),
                2 => auth_fields(&mut v),
                3 => v.extend([Field::ScriptSection, self.script_table()]),
                _ => v.extend([
                    Field::Timeout,
                    Field::Deadline,
                    Field::WsSubprotocols,
                    Field::Tls,
                    Field::GrpcVerify,
                    Field::Cookies,
                ]),
            },
            Kind::Mqtt => match tab {
                0 => v.push(Field::MqttSubscriptions),
                1 => v.push(Field::MqttPublish),
                2 => v.extend([
                    Field::MqttWillTopic,
                    Field::MqttWillQos,
                    Field::MqttWillRetain,
                    Field::MqttWillBase64,
                    Field::MqttWillPayload,
                ]),
                3 => {
                    v.extend([Field::MqttUser, Field::MqttPass]);
                    auth_fields(&mut v);
                }
                4 => v.extend([Field::ScriptSection, self.script_table()]),
                _ => v.extend([
                    Field::MqttClientId,
                    Field::MqttProtocol,
                    Field::MqttCleanSession,
                    Field::MqttKeepAlive,
                    Field::Timeout,
                    Field::Deadline,
                    Field::VerifyTls,
                ]),
            },
            Kind::Mcp => match tab {
                0 => v.extend([Field::McpServer, Field::McpTool]),
                1 => v.push(Field::Message),
                2 => v.extend([Field::ScriptSection, self.script_table()]),
                _ => v.push(Field::Timeout),
            },
            Kind::Folder => match tab {
                0 => v.push(Field::Headers),
                1 => auth_fields(&mut v),
                2 => v.push(Field::Vars),
                _ => v.extend([Field::ScriptSection, self.script_table()]),
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
            Kind::Sse => SSE_TABS,
            Kind::Grpc => GRPC_TABS,
            Kind::Ws => WS_TABS,
            Kind::Mqtt => MQTT_TABS,
            Kind::Mcp => MCP_TABS,
            Kind::Folder => FOLDER_TABS,
        }
    }

    pub fn text_mut(&mut self, f: Field) -> Option<&mut TextEditor> {
        Some(match f {
            Field::BodyText if self.body_kind == BodyKind::GraphQL => match self.graphql_section {
                1 => &mut self.graphql_variables,
                2 => &mut self.graphql_operation,
                _ => &mut self.body_text,
            },
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
            Field::MaxEvents => &mut self.max_events,
            Field::McpServer => &mut self.mcp_server,
            Field::McpTool => &mut self.mcp_tool,
            Field::WsSubprotocols => &mut self.ws_subprotocols,
            Field::WsMessages => &mut self.message,
            Field::AwsAccessKey => &mut self.aws_access,
            Field::AwsSecretKey => &mut self.aws_secret,
            Field::AwsSessionToken => &mut self.aws_token,
            Field::AwsRegion => &mut self.aws_region,
            Field::AwsService => &mut self.aws_service,
            Field::JwtSecret => &mut self.jwt_secret,
            Field::JwtPrivateKey => &mut self.jwt_key,
            Field::JwtClaims => &mut self.jwt_claims,
            Field::JwtExpires => &mut self.jwt_expires,
            Field::MqttWillTopic => &mut self.will_topic,
            Field::MqttWillPayload => &mut self.will_payload,
            Field::MqttUser => &mut self.mqtt_user,
            Field::MqttPass => &mut self.mqtt_pass,
            Field::MqttClientId => &mut self.mqtt_client_id,
            Field::MqttKeepAlive => &mut self.keep_alive,
            Field::Description => &mut self.description,
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
            Field::Vars => &mut self.vars,
            Field::MqttSubscriptions => &mut self.mqtt_subs,
            Field::MqttPublish => &mut self.mqtt_publish,
            _ => return None,
        })
    }

    /// Current value label of a choice or toggle field.
    pub fn choice_label(&self, f: Field) -> String {
        match f {
            Field::GraphqlSection => {
                ["Query", "Variables", "Operation name"][self.graphql_section].into()
            }
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
            Field::JwtAlgorithm => self.jwt_alg.as_str().into(),
            Field::MqttWillQos => self.will_qos.to_string(),
            Field::MqttProtocol => self.mqtt_protocol.as_str().into(),
            Field::MqttWillRetain => onoff(self.will_retain),
            Field::MqttWillBase64 => onoff(self.will_base64),
            Field::MqttCleanSession => onoff(self.clean_session),
            Field::DocsView => DOCS_VIEWS[usize::from(self.docs_preview)].into(),
            _ => String::new(),
        }
    }

    pub fn choice_options(&self, f: Field) -> Vec<String> {
        match f {
            Field::GraphqlSection => ["Query", "Variables", "Operation name"]
                .into_iter()
                .map(str::to_string)
                .collect(),
            Field::BodyKind => BodyKind::ALL
                .iter()
                .map(|k| k.label().to_string())
                .collect(),
            Field::AuthKind => self
                .auth_kinds()
                .iter()
                .map(|k| k.label().to_string())
                .collect(),
            Field::JwtAlgorithm => JwtAlgorithm::ALL
                .iter()
                .map(|a| a.as_str().to_string())
                .collect(),
            Field::AuthIn => vec!["Header".into(), "Query parameter".into()],
            Field::SchemaSource => vec!["Server reflection".into(), "Proto files".into()],
            Field::ScriptSection => self.script_options(),
            Field::DocsView => DOCS_VIEWS.iter().map(|v| v.to_string()).collect(),
            Field::Cookies => vec!["Config default".into(), "On".into(), "Off".into()],
            Field::MqttWillQos => QOS.iter().map(|q| q.to_string()).collect(),
            Field::MqttProtocol => MqttProtocol::ALL
                .iter()
                .map(|p| p.as_str().to_string())
                .collect(),
            _ => vec![],
        }
    }

    /// Cycles a choice (dir = ±1) or flips a toggle. Returns true if changed.
    pub fn cycle(&mut self, f: Field, dir: isize) -> bool {
        let step = |i: usize, n: usize| crate::list::cycle(i, dir, n);
        match f {
            Field::BodyKind => {
                let i = BodyKind::ALL
                    .iter()
                    .position(|k| *k == self.body_kind)
                    .unwrap_or(0);
                self.set_body_kind(BodyKind::ALL[step(i, BodyKind::ALL.len())]);
            }
            Field::AuthKind => {
                let kinds = self.auth_kinds();
                let i = kinds.iter().position(|k| *k == self.auth_kind).unwrap_or(0);
                self.auth_kind = kinds[step(i, kinds.len())];
            }
            Field::JwtAlgorithm => {
                let all = JwtAlgorithm::ALL;
                let i = all.iter().position(|a| *a == self.jwt_alg).unwrap_or(0);
                self.jwt_alg = all[step(i, all.len())];
            }
            Field::AuthIn => {
                self.auth_in = match self.auth_in {
                    ApiKeyLocation::Header => ApiKeyLocation::Query,
                    ApiKeyLocation::Query => ApiKeyLocation::Header,
                }
            }
            Field::SchemaSource => self.schema_files = !self.schema_files,
            Field::GraphqlSection => self.graphql_section = step(self.graphql_section, 3),
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
            Field::MqttWillQos => self.will_qos = step(usize::from(self.will_qos), 3) as u8,
            Field::MqttProtocol => {
                let all = MqttProtocol::ALL;
                let i = all
                    .iter()
                    .position(|p| *p == self.mqtt_protocol)
                    .unwrap_or(0);
                self.mqtt_protocol = all[step(i, all.len())];
            }
            Field::MqttWillRetain => self.will_retain = !self.will_retain,
            Field::MqttWillBase64 => self.will_base64 = !self.will_base64,
            Field::MqttCleanSession => self.clean_session = !self.clean_session,
            Field::DocsView => self.docs_preview = !self.docs_preview,
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
                if let Some(k) = self.auth_kinds().get(index) {
                    self.auth_kind = *k;
                }
            }
            Field::JwtAlgorithm => {
                if let Some(a) = JwtAlgorithm::ALL.get(index) {
                    self.jwt_alg = *a;
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
            Field::GraphqlSection => self.graphql_section = index.min(2),
            Field::ScriptSection => self.script_section = index.min(2),
            Field::DocsView => self.docs_preview = index == 1,
            Field::Cookies => {
                self.cookies = match index {
                    1 => Some(true),
                    2 => Some(false),
                    _ => None,
                }
            }
            Field::MqttWillQos => self.will_qos = index.min(2) as u8,
            Field::MqttProtocol => {
                if let Some(p) = MqttProtocol::ALL.get(index) {
                    self.mqtt_protocol = *p;
                }
            }
            _ => {}
        }
    }

    /// The auth kinds this tab offers: the HTTP-only ones for HTTP and for a
    /// folder (whose HTTP requests may use them); MQTT offers inherit, none
    /// and basic only.
    fn auth_kinds(&self) -> Vec<AuthKind> {
        if self.kind == Kind::Mqtt {
            return vec![AuthKind::Inherit, AuthKind::None, AuthKind::Basic];
        }
        let http = matches!(self.kind, Kind::Http | Kind::Sse | Kind::Folder);
        AuthKind::ALL
            .into_iter()
            .filter(|k| http || !k.http_only())
            .collect()
    }

    pub fn set_body_kind(&mut self, k: BodyKind) {
        if (k == BodyKind::GraphQL || self.body_kind == BodyKind::GraphQL)
            && k != self.body_kind
            && let Request::Http(request) = self.to_request()
        {
            match request.body.convert(k) {
                Body::GraphQL {
                    query,
                    variables,
                    operation_name,
                } => {
                    self.body_text.set_text(&query);
                    self.graphql_variables.set_text(&variables);
                    self.graphql_operation.set_text(&operation_name);
                    self.graphql_section = 0;
                }
                Body::Json { text } | Body::Raw { text, .. } => self.body_text.set_text(&text),
                _ => {}
            }
        }
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
            BodyKind::Json | BodyKind::GraphQL => Some("application/json".to_string()),
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
            AuthKind::Digest if !has("authorization") => v.push((
                "Authorization".into(),
                "Digest … (answers the server's 401 challenge)".into(),
            )),
            AuthKind::AwsSigV4 if !has("authorization") => {
                v.push((
                    "Authorization".into(),
                    "AWS4-HMAC-SHA256 … (signed when sent)".into(),
                ));
                v.push(("X-Amz-Date".into(), "(time of the send)".into()));
            }
            AuthKind::Jwt if !has("authorization") => v.push((
                "Authorization".into(),
                format!("Bearer <{} JWT signed when sent>", self.jwt_alg.as_str()),
            )),
            _ => {}
        }
        if self.kind == Kind::Sse && !has("accept") {
            v.push(("Accept".into(), "text/event-stream".into()));
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
            &mut self.graphql_variables,
            &mut self.graphql_operation,
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
            &mut self.max_events,
            &mut self.proto_files,
            &mut self.import_paths,
            &mut self.ws_subprotocols,
            &mut self.aws_access,
            &mut self.aws_secret,
            &mut self.aws_token,
            &mut self.aws_region,
            &mut self.aws_service,
            &mut self.jwt_secret,
            &mut self.jwt_key,
            &mut self.jwt_claims,
            &mut self.jwt_expires,
            &mut self.will_topic,
            &mut self.will_payload,
            &mut self.mqtt_user,
            &mut self.mqtt_pass,
            &mut self.mqtt_client_id,
            &mut self.keep_alive,
            &mut self.description,
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
            &mut self.vars,
            &mut self.mqtt_subs,
            &mut self.mqtt_publish,
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

pub(super) fn onoff(b: bool) -> String {
    if b { "On".into() } else { "Off".into() }
}
