//! Request files: `<name>.toml`, one request per file. Schema in FORMAT.md.

use serde::Deserialize;

use sendito_core::model::*;

use crate::tomlfmt::Writer;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawKv {
    name: String,
    #[serde(default)]
    value: String,
    #[serde(default = "yes")]
    enabled: bool,
}

fn yes() -> bool {
    true
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawOptions {
    timeout_ms: Option<u64>,
    follow_redirects: Option<bool>,
    verify_tls: Option<bool>,
    cookies: Option<bool>,
    tls: Option<bool>,
    deadline_ms: Option<u64>,
    max_events: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPart {
    name: String,
    value: Option<String>,
    file: Option<String>,
    filename: Option<String>,
    #[serde(default)]
    content_type: String,
    #[serde(default = "yes")]
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBody {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    text: String,
    #[serde(default)]
    content_type: String,
    #[serde(default)]
    path: String,
    #[serde(default)]
    fields: Vec<RawKv>,
    #[serde(default)]
    parts: Vec<RawPart>,
    #[serde(default)]
    query: String,
    #[serde(default)]
    variables: String,
    #[serde(default)]
    operation_name: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawAuth {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: String,
    #[serde(rename = "in", default)]
    location: Option<String>,
    #[serde(default)]
    access_key: String,
    #[serde(default)]
    secret_key: String,
    #[serde(default)]
    session_token: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    service: String,
    algorithm: Option<String>,
    #[serde(default)]
    secret: String,
    #[serde(default)]
    private_key: String,
    claims: Option<String>,
    expires_in_s: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSchema {
    source: String,
    #[serde(default)]
    files: Vec<String>,
    #[serde(default)]
    import_paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSet {
    var: String,
    #[serde(default)]
    value: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExtract {
    var: String,
    from: String,
    #[serde(default)]
    expr: String,
    #[serde(default)]
    secret: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAssert {
    from: String,
    #[serde(default)]
    expr: String,
    op: String,
    #[serde(default)]
    value: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct RawScripts {
    #[serde(default)]
    pre: Vec<RawSet>,
    #[serde(default)]
    extract: Vec<RawExtract>,
    #[serde(default, rename = "assert")]
    asserts: Vec<RawAssert>,
    lua: Option<RawLua>,
}

/// `[scripts.lua]`: the Lua code that runs before the send and after the
/// response.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawLua {
    #[serde(default)]
    pre: String,
    #[serde(default)]
    post: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSubscription {
    topic: String,
    #[serde(default)]
    qos: u8,
    #[serde(default = "yes")]
    enabled: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMessage {
    #[serde(default)]
    topic: String,
    payload: Option<String>,
    payload_base64: Option<String>,
    #[serde(default)]
    qos: u8,
    #[serde(default)]
    retain: bool,
}

fn mqtt_message(m: RawMessage, what: &str) -> Result<MqttMessage, String> {
    let payload = match (m.payload, m.payload_base64) {
        (Some(_), Some(_)) => {
            return Err(format!("{what}: set payload or payload_base64, not both"));
        }
        (Some(text), None) => MqttPayload::text(text),
        (None, Some(text)) => MqttPayload { text, base64: true },
        (None, None) => MqttPayload::default(),
    };
    if m.qos > 2 {
        return Err(format!("{what}: qos must be 0, 1 or 2, not {}", m.qos));
    }
    Ok(MqttMessage {
        topic: m.topic,
        payload,
        qos: m.qos,
        retain: m.retain,
    })
}

fn mqtt(r: RawRequest) -> Result<Request, String> {
    if r.method.is_some() || r.body.is_some() || !r.headers.is_empty() || !r.params.is_empty() {
        return Err("MQTT does not support method, body, headers or params fields".into());
    }
    // Auth kinds MQTT does not support are refused at send time, so a
    // draft the editor saves can always be opened again.
    let auth = auth(r.auth)?;
    let protocol = match r.protocol.as_deref() {
        None => MqttProtocol::default(),
        Some(p) => MqttProtocol::parse(p)
            .ok_or_else(|| format!("protocol must be \"3.1.1\" or \"5\", not {p:?}"))?,
    };
    let subscriptions = r
        .subscriptions
        .into_iter()
        .map(|s| {
            if s.qos > 2 {
                return Err(format!(
                    "subscription {:?}: qos must be 0, 1 or 2, not {}",
                    s.topic, s.qos
                ));
            }
            Ok(MqttSubscription {
                topic: s.topic,
                qos: s.qos,
                enabled: s.enabled,
            })
        })
        .collect::<Result<_, String>>()?;
    Ok(Request::Mqtt(MqttRequest {
        url: r.url.unwrap_or_default(),
        client_id: r.client_id.unwrap_or_default(),
        username: r.username.unwrap_or_default(),
        password: r.password.unwrap_or_default(),
        protocol,
        clean_session: r.clean_session.unwrap_or(true),
        keep_alive_s: r.keep_alive_s.unwrap_or(MQTT_KEEP_ALIVE_S),
        options: MqttOptions {
            timeout_ms: r.options.timeout_ms,
            deadline_ms: r.options.deadline_ms,
            verify_tls: r.options.verify_tls.unwrap_or(true),
        },
        subscriptions,
        publish: r
            .publish
            .into_iter()
            .map(|m| mqtt_message(m, "publish"))
            .collect::<Result<_, String>>()?,
        last_will: r
            .last_will
            .map(|m| mqtt_message(m, "last_will"))
            .transpose()?,
        auth,
        scripts: scripts(r.scripts)?,
        description: r.description.unwrap_or_default(),
    }))
}

fn write_mqtt_message(w: &mut Writer, m: &MqttMessage) {
    w.str("topic", &m.topic);
    w.str(
        if m.payload.base64 {
            "payload_base64"
        } else {
            "payload"
        },
        &m.payload.text,
    );
    if m.qos != 0 {
        w.int("qos", u64::from(m.qos));
    }
    if m.retain {
        w.bool("retain", true);
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    kind: String,
    description: Option<String>,
    method: Option<String>,
    url: Option<String>,
    target: Option<String>,
    service: Option<String>,
    message: Option<String>,
    messages: Option<String>,
    server: Option<String>,
    tool: Option<String>,
    arguments: Option<String>,
    #[serde(default)]
    subprotocols: Vec<String>,
    #[serde(default)]
    options: RawOptions,
    #[serde(default)]
    params: Vec<RawKv>,
    #[serde(default)]
    headers: Vec<RawKv>,
    #[serde(default)]
    metadata: Vec<RawKv>,
    body: Option<RawBody>,
    auth: Option<RawAuth>,
    schema: Option<RawSchema>,
    #[serde(default)]
    scripts: RawScripts,
    client_id: Option<String>,
    username: Option<String>,
    password: Option<String>,
    protocol: Option<String>,
    clean_session: Option<bool>,
    keep_alive_s: Option<u64>,
    #[serde(default)]
    subscriptions: Vec<RawSubscription>,
    #[serde(default)]
    publish: Vec<RawMessage>,
    last_will: Option<RawMessage>,
}

pub(crate) fn kvs(v: Vec<RawKv>) -> Vec<KeyValue> {
    v.into_iter()
        .map(|k| KeyValue {
            name: k.name,
            value: k.value,
            enabled: k.enabled,
        })
        .collect()
}

pub(crate) fn auth(a: Option<RawAuth>) -> Result<Auth, String> {
    // No `[auth]` table: the request inherits its folder's auth.
    let Some(a) = a else { return Ok(Auth::Inherit) };
    Ok(match a.kind.as_str() {
        "inherit" => Auth::Inherit,
        "none" => Auth::None,
        "basic" => Auth::Basic {
            username: a.username,
            password: a.password,
        },
        "bearer" => Auth::Bearer { token: a.token },
        "oauth" => Auth::OAuth { name: a.name },
        "apikey" => Auth::ApiKey {
            key: a.key,
            value: a.value,
            location: match a.location.as_deref() {
                None | Some("header") => ApiKeyLocation::Header,
                Some("query") => ApiKeyLocation::Query,
                Some(o) => {
                    return Err(format!(
                        "auth.in must be \"header\" or \"query\", got {o:?}"
                    ));
                }
            },
        },
        "digest" => Auth::Digest {
            username: a.username,
            password: a.password,
        },
        "awsv4" => Auth::AwsSigV4 {
            access_key: a.access_key,
            secret_key: a.secret_key,
            session_token: a.session_token,
            region: a.region,
            service: a.service,
        },
        "jwt" => Auth::Jwt {
            algorithm: match a.algorithm.as_deref() {
                None => JwtAlgorithm::Hs256,
                Some(s) => JwtAlgorithm::parse(s).ok_or_else(|| {
                    format!("auth.algorithm must be HS256, HS384, HS512, RS256 or ES256, got {s:?}")
                })?,
            },
            secret: a.secret,
            private_key: a.private_key,
            claims: a.claims.unwrap_or_else(|| "{}".into()),
            expires_in_s: a.expires_in_s,
        },
        o => return Err(format!("unknown auth type {o:?}")),
    })
}

pub(crate) fn scripts(s: RawScripts) -> Result<Scripts, String> {
    let src = |s: &str| {
        Source::parse(s).ok_or_else(|| {
            let names: Vec<_> = Source::ALL.iter().map(|s| s.as_str()).collect();
            format!("unknown script source {s:?} ({})", names.join(", "))
        })
    };
    let lua = s.lua.unwrap_or_default();
    Ok(Scripts {
        lua_pre: LuaScript::own(lua.pre).into_iter().collect(),
        lua_post: LuaScript::own(lua.post).into_iter().collect(),
        pre: s
            .pre
            .into_iter()
            .map(|p| SetVar {
                var: p.var,
                value: p.value,
            })
            .collect(),
        extract: s
            .extract
            .into_iter()
            .map(|e| {
                Ok(Extract {
                    var: e.var,
                    source: src(&e.from)?,
                    expr: e.expr,
                    secret: e.secret,
                })
            })
            .collect::<Result<_, String>>()?,
        asserts: s
            .asserts
            .into_iter()
            .map(|a| {
                Ok(Assertion {
                    source: src(&a.from)?,
                    expr: a.expr,
                    op: AssertOp::parse(&a.op)
                        .ok_or_else(|| format!("unknown assert op {:?}", a.op))?,
                    value: a.value,
                })
            })
            .collect::<Result<_, String>>()?,
    })
}

fn body(b: Option<RawBody>) -> Result<Body, String> {
    let Some(b) = b else { return Ok(Body::None) };
    Ok(match b.kind.as_str() {
        "none" => Body::None,
        "raw" => Body::Raw {
            content_type: b.content_type,
            text: b.text,
        },
        "json" => Body::Json { text: b.text },
        "form" => Body::Form {
            fields: kvs(b.fields),
        },
        "file" => Body::File {
            path: b.path,
            content_type: b.content_type,
        },
        "multipart" => Body::Multipart {
            parts: b
                .parts
                .into_iter()
                .map(|p| {
                    let kind = match (p.value, p.file) {
                        (Some(value), None) => PartKind::Text { value },
                        (None, Some(path)) => PartKind::File {
                            path,
                            filename: p.filename,
                        },
                        (None, None) => PartKind::Text {
                            value: String::new(),
                        },
                        (Some(_), Some(_)) => {
                            return Err(format!(
                                "multipart part {:?} has both value and file",
                                p.name
                            ));
                        }
                    };
                    Ok(MultipartPart {
                        name: p.name,
                        kind,
                        content_type: p.content_type,
                        enabled: p.enabled,
                    })
                })
                .collect::<Result<_, String>>()?,
        },
        // `variables` is JSON text kept as written (a half-typed object is a
        // valid draft); the sender checks that it is an object.
        "graphql" => Body::GraphQL {
            query: b.query,
            variables: b.variables,
            operation_name: b.operation_name.unwrap_or_default(),
        },
        o => return Err(format!("unknown body type {o:?}")),
    })
}

/// Parses a request file.
pub fn parse(text: &str) -> Result<Request, String> {
    let r: RawRequest = toml::from_str(text).map_err(|e| e.to_string())?;
    match r.kind.as_str() {
        "http" => Ok(Request::Http(HttpRequest {
            method: r.method.unwrap_or_else(|| "GET".into()),
            url: r.url.unwrap_or_default(),
            params: kvs(r.params),
            headers: kvs(r.headers),
            body: body(r.body)?,
            auth: auth(r.auth)?,
            options: HttpOptions {
                timeout_ms: r.options.timeout_ms,
                follow_redirects: r.options.follow_redirects.unwrap_or(true),
                verify_tls: r.options.verify_tls.unwrap_or(true),
                cookies: r.options.cookies,
            },
            scripts: scripts(r.scripts)?,
            description: r.description.unwrap_or_default(),
        })),
        "sse" if r.method.is_some() || r.body.is_some() => {
            Err("SSE does not support method or body fields".into())
        }
        "sse" => Ok(Request::Sse(SseRequest {
            url: r.url.unwrap_or_default(),
            params: kvs(r.params),
            headers: kvs(r.headers),
            auth: auth(r.auth)?,
            scripts: scripts(r.scripts)?,
            description: r.description.unwrap_or_default(),
            options: SseOptions {
                timeout_ms: r.options.timeout_ms,
                deadline_ms: r.options.deadline_ms,
                max_events: r.options.max_events,
                follow_redirects: r.options.follow_redirects.unwrap_or(true),
                verify_tls: r.options.verify_tls.unwrap_or(true),
                cookies: r.options.cookies,
            },
        })),
        "grpc" => Ok(Request::Grpc(GrpcRequest {
            target: r.target.unwrap_or_default(),
            service: r.service.unwrap_or_default(),
            method: r.method.unwrap_or_default(),
            message: r.message.unwrap_or_default(),
            metadata: kvs(r.metadata),
            auth: auth(r.auth)?,
            tls: r.options.tls.unwrap_or(false),
            verify_tls: r.options.verify_tls.unwrap_or(true),
            deadline_ms: r.options.deadline_ms,
            schema: match r.schema {
                None => SchemaSource::Reflection,
                Some(s) => match s.source.as_str() {
                    "reflection" => SchemaSource::Reflection,
                    "files" => SchemaSource::Files {
                        files: s.files,
                        import_paths: s.import_paths,
                    },
                    o => return Err(format!("unknown schema source {o:?}")),
                },
            },
            scripts: scripts(r.scripts)?,
            description: r.description.unwrap_or_default(),
        })),
        "mcp" => {
            // `arguments` is JSON text. It is not validated here: a half-typed
            // object is a valid draft and saving must never produce a file the
            // app cannot reopen. `sendito-mcp` rejects a non-object at send time.
            let arguments = r.arguments.unwrap_or_else(|| "{}".into());
            Ok(Request::Mcp(McpRequest {
                server: r.server.unwrap_or_default(),
                tool: r.tool.unwrap_or_default(),
                arguments,
                timeout_ms: r.options.timeout_ms,
                scripts: scripts(r.scripts)?,
                description: r.description.unwrap_or_default(),
            }))
        }
        "websocket" => Ok(Request::Ws(WebSocketRequest {
            url: r.url.unwrap_or_default(),
            headers: kvs(r.headers),
            subprotocols: r.subprotocols,
            // The message document is JSON text, stored and read as written
            // (like MCP `arguments`); the sender parses it.
            messages: r.messages.unwrap_or_else(|| "[]".into()),
            auth: auth(r.auth)?,
            options: WsOptions {
                timeout_ms: r.options.timeout_ms,
                deadline_ms: r.options.deadline_ms,
                verify_tls: r.options.verify_tls.unwrap_or(true),
                tls: r.options.tls.unwrap_or(false),
                cookies: r.options.cookies,
            },
            scripts: scripts(r.scripts)?,
            description: r.description.unwrap_or_default(),
        })),
        "mqtt" => mqtt(r),
        o => Err(format!(
            "unknown request kind {o:?} (expected \"http\", \"grpc\", \"websocket\", \"sse\", \"mqtt\" or \"mcp\")"
        )),
    }
}

pub(crate) fn write_kvs(w: &mut Writer, table: &str, v: &[KeyValue]) {
    for kv in v {
        w.array_table(table);
        w.str("name", &kv.name);
        w.str("value", &kv.value);
        if !kv.enabled {
            w.bool("enabled", false);
        }
    }
}

pub(crate) fn write_auth(w: &mut Writer, a: &Auth) {
    match a {
        Auth::Inherit => {}
        // Written out: an absent `[auth]` means "inherit".
        Auth::None => {
            w.table("auth");
            w.str("type", "none");
        }
        Auth::Basic { username, password } => {
            w.table("auth");
            w.str("type", "basic");
            w.str("username", username);
            w.str("password", password);
        }
        Auth::Bearer { token } => {
            w.table("auth");
            w.str("type", "bearer");
            w.str("token", token);
        }
        Auth::OAuth { name } => {
            w.table("auth");
            w.str("type", "oauth");
            w.str("name", name);
        }
        Auth::ApiKey {
            key,
            value,
            location,
        } => {
            w.table("auth");
            w.str("type", "apikey");
            w.str("key", key);
            w.str("value", value);
            w.str(
                "in",
                match location {
                    ApiKeyLocation::Header => "header",
                    ApiKeyLocation::Query => "query",
                },
            );
        }
        Auth::Digest { username, password } => {
            w.table("auth");
            w.str("type", "digest");
            w.str("username", username);
            w.str("password", password);
        }
        Auth::AwsSigV4 {
            access_key,
            secret_key,
            session_token,
            region,
            service,
        } => {
            w.table("auth");
            w.str("type", "awsv4");
            w.str("access_key", access_key);
            w.str("secret_key", secret_key);
            if !session_token.is_empty() {
                w.str("session_token", session_token);
            }
            w.str("region", region);
            w.str("service", service);
        }
        Auth::Jwt {
            algorithm,
            secret,
            private_key,
            claims,
            expires_in_s,
        } => {
            w.table("auth");
            w.str("type", "jwt");
            w.str("algorithm", algorithm.as_str());
            if !secret.is_empty() {
                w.str("secret", secret);
            }
            if !private_key.is_empty() {
                w.str("private_key", private_key);
            }
            w.str("claims", claims);
            if let Some(s) = expires_in_s {
                w.int("expires_in_s", *s);
            }
        }
    }
}

/// Writes a script rule's `expr`. Status reads no expression, and an empty
/// one is the default (`total` for time and size), so neither is written.
fn write_expr(w: &mut Writer, source: Source, expr: &str) {
    if source != Source::Status && !expr.is_empty() {
        w.str("expr", expr);
    }
}

pub(crate) fn write_scripts(w: &mut Writer, s: &Scripts) {
    for p in &s.pre {
        w.array_table("scripts.pre");
        w.str("var", &p.var);
        w.str("value", &p.value);
    }
    for e in &s.extract {
        w.array_table("scripts.extract");
        w.str("var", &e.var);
        w.str("from", e.source.as_str());
        write_expr(w, e.source, &e.expr);
        if e.secret {
            w.bool("secret", true);
        }
    }
    for a in &s.asserts {
        w.array_table("scripts.assert");
        w.str("from", a.source.as_str());
        write_expr(w, a.source, &a.expr);
        w.str("op", a.op.as_str());
        if a.op.takes_value() {
            w.str("value", &a.value);
        }
    }
    // A file holds the request's (or folder's) own script only.
    let code = |v: &[LuaScript]| v.first().map(|r| r.code.clone()).unwrap_or_default();
    let (pre, post) = (code(&s.lua_pre), code(&s.lua_post));
    if !pre.trim().is_empty() || !post.trim().is_empty() {
        w.table("scripts.lua");
        if !pre.trim().is_empty() {
            w.str("pre", &pre);
        }
        if !post.trim().is_empty() {
            w.str("post", &post);
        }
    }
}

/// Serializes a request in canonical form.
pub fn to_toml(req: &Request) -> String {
    let mut w = Writer::new();
    match req {
        Request::Http(h) => {
            w.str("kind", "http");
            w.str("method", &h.method);
            w.str("url", &h.url);
            write_description(&mut w, req.description());
            let o = &h.options;
            if o.timeout_ms.is_some() || !o.follow_redirects || !o.verify_tls || o.cookies.is_some()
            {
                w.table("options");
                if let Some(t) = o.timeout_ms {
                    w.int("timeout_ms", t);
                }
                if !o.follow_redirects {
                    w.bool("follow_redirects", false);
                }
                if !o.verify_tls {
                    w.bool("verify_tls", false);
                }
                if let Some(c) = o.cookies {
                    w.bool("cookies", c);
                }
            }
            write_kvs(&mut w, "params", &h.params);
            write_kvs(&mut w, "headers", &h.headers);
            write_auth(&mut w, &h.auth);
            match &h.body {
                Body::None => {}
                Body::Raw { content_type, text } => {
                    w.table("body");
                    w.str("type", "raw");
                    w.str("content_type", content_type);
                    w.str("text", text);
                }
                Body::Json { text } => {
                    w.table("body");
                    w.str("type", "json");
                    w.str("text", text);
                }
                Body::Form { fields } => {
                    w.table("body");
                    w.str("type", "form");
                    write_kvs(&mut w, "body.fields", fields);
                }
                Body::File { path, content_type } => {
                    w.table("body");
                    w.str("type", "file");
                    w.str("path", path);
                    if !content_type.is_empty() {
                        w.str("content_type", content_type);
                    }
                }
                Body::Multipart { parts } => {
                    w.table("body");
                    w.str("type", "multipart");
                    for p in parts {
                        w.array_table("body.parts");
                        w.str("name", &p.name);
                        match &p.kind {
                            PartKind::Text { value } => w.str("value", value),
                            PartKind::File { path, filename } => {
                                w.str("file", path);
                                if let Some(f) = filename {
                                    w.str("filename", f);
                                }
                            }
                        }
                        if !p.content_type.is_empty() {
                            w.str("content_type", &p.content_type);
                        }
                        if !p.enabled {
                            w.bool("enabled", false);
                        }
                    }
                }
                Body::GraphQL {
                    query,
                    variables,
                    operation_name,
                } => {
                    w.table("body");
                    w.str("type", "graphql");
                    w.str("query", query);
                    if !variables.is_empty() {
                        w.str("variables", variables);
                    }
                    if !operation_name.is_empty() {
                        w.str("operation_name", operation_name);
                    }
                }
            }
            write_scripts(&mut w, &h.scripts);
        }
        Request::Grpc(g) => {
            w.str("kind", "grpc");
            w.str("target", &g.target);
            w.str("service", &g.service);
            w.str("method", &g.method);
            w.str("message", &g.message);
            write_description(&mut w, req.description());
            if g.tls || !g.verify_tls || g.deadline_ms.is_some() {
                w.table("options");
                if g.tls {
                    w.bool("tls", true);
                }
                if !g.verify_tls {
                    w.bool("verify_tls", false);
                }
                if let Some(d) = g.deadline_ms {
                    w.int("deadline_ms", d);
                }
            }
            if let SchemaSource::Files {
                files,
                import_paths,
            } = &g.schema
            {
                w.table("schema");
                w.str("source", "files");
                w.str_array("files", files);
                w.str_array("import_paths", import_paths);
            }
            write_kvs(&mut w, "metadata", &g.metadata);
            write_auth(&mut w, &g.auth);
            write_scripts(&mut w, &g.scripts);
        }
        Request::Mcp(m) => {
            w.str("kind", "mcp");
            w.str("server", &m.server);
            w.str("tool", &m.tool);
            if m.arguments != "{}" {
                w.str("arguments", &m.arguments);
            }
            write_description(&mut w, req.description());
            if let Some(t) = m.timeout_ms {
                w.table("options");
                w.int("timeout_ms", t);
            }
            write_scripts(&mut w, &m.scripts);
        }
        Request::Sse(s) => {
            w.str("kind", "sse");
            w.str("url", &s.url);
            write_description(&mut w, req.description());
            let o = &s.options;
            if o != &SseOptions::default() {
                w.table("options");
                if let Some(v) = o.timeout_ms {
                    w.int("timeout_ms", v);
                }
                if let Some(v) = o.deadline_ms {
                    w.int("deadline_ms", v);
                }
                if let Some(v) = o.max_events {
                    w.int("max_events", v);
                }
                if !o.follow_redirects {
                    w.bool("follow_redirects", false);
                }
                if !o.verify_tls {
                    w.bool("verify_tls", false);
                }
                if let Some(v) = o.cookies {
                    w.bool("cookies", v);
                }
            }
            write_kvs(&mut w, "params", &s.params);
            write_kvs(&mut w, "headers", &s.headers);
            write_auth(&mut w, &s.auth);
            write_scripts(&mut w, &s.scripts);
        }
        Request::Mqtt(m) => {
            w.str("kind", "mqtt");
            w.str("url", &m.url);
            for (key, value) in [
                ("client_id", &m.client_id),
                ("username", &m.username),
                ("password", &m.password),
            ] {
                if !value.is_empty() {
                    w.str(key, value);
                }
            }
            if m.protocol != MqttProtocol::default() {
                w.str("protocol", m.protocol.as_str());
            }
            if !m.clean_session {
                w.bool("clean_session", false);
            }
            if m.keep_alive_s != MQTT_KEEP_ALIVE_S {
                w.int("keep_alive_s", m.keep_alive_s);
            }
            write_description(&mut w, req.description());
            let o = &m.options;
            if o != &MqttOptions::default() {
                w.table("options");
                if let Some(v) = o.timeout_ms {
                    w.int("timeout_ms", v);
                }
                if let Some(v) = o.deadline_ms {
                    w.int("deadline_ms", v);
                }
                if !o.verify_tls {
                    w.bool("verify_tls", false);
                }
            }
            for sub in &m.subscriptions {
                w.array_table("subscriptions");
                w.str("topic", &sub.topic);
                if sub.qos != 0 {
                    w.int("qos", u64::from(sub.qos));
                }
                if !sub.enabled {
                    w.bool("enabled", false);
                }
            }
            for p in &m.publish {
                w.array_table("publish");
                write_mqtt_message(&mut w, p);
            }
            if let Some(will) = &m.last_will {
                w.table("last_will");
                write_mqtt_message(&mut w, will);
            }
            write_auth(&mut w, &m.auth);
            write_scripts(&mut w, &m.scripts);
        }
        Request::Ws(wq) => {
            w.str("kind", "websocket");
            w.str("url", &wq.url);
            if !wq.subprotocols.is_empty() {
                w.str_array("subprotocols", &wq.subprotocols);
            }
            if wq.messages != "[]" {
                w.str("messages", &wq.messages);
            }
            write_description(&mut w, req.description());
            let o = &wq.options;
            if o.timeout_ms.is_some()
                || o.deadline_ms.is_some()
                || !o.verify_tls
                || o.tls
                || o.cookies.is_some()
            {
                w.table("options");
                if let Some(t) = o.timeout_ms {
                    w.int("timeout_ms", t);
                }
                if let Some(d) = o.deadline_ms {
                    w.int("deadline_ms", d);
                }
                if !o.verify_tls {
                    w.bool("verify_tls", false);
                }
                if o.tls {
                    w.bool("tls", true);
                }
                if let Some(c) = o.cookies {
                    w.bool("cookies", c);
                }
            }
            write_kvs(&mut w, "headers", &wq.headers);
            write_auth(&mut w, &wq.auth);
            write_scripts(&mut w, &wq.scripts);
        }
    }
    w.finish()
}

/// `description` after the top-level keys and before the first table, so
/// [`sniff_badge`] reads `kind` and `method` first.
fn write_description(w: &mut Writer, text: &str) {
    if !text.is_empty() {
        w.str("description", text);
    }
}

/// Cheap sniff of `kind`/`method` from the top of a request file (for tree badges
/// without a full parse). Returns `None` when the file is not a request.
pub fn sniff_badge(text: &str) -> Option<String> {
    let mut kind = None;
    let mut method = None;
    // The closing quotes of the multi-line string the scan is in: its lines
    // (a description, for example) are text, not keys.
    let mut open: Option<&str> = None;
    for line in text.lines() {
        if let Some(d) = open {
            if line.contains(d) {
                open = None;
            }
            continue;
        }
        let l = line.trim();
        if l.starts_with('[') {
            break;
        }
        open = ["'''", "\"\"\""]
            .into_iter()
            .find(|d| l.matches(d).count() == 1);
        if open.is_some() {
            continue;
        }
        if !l.contains("kind") && !l.contains("method") {
            continue;
        }
        // Each line is parsed as TOML, so quoted keys, either quote style and
        // trailing comments read the same as in the full parser.
        let Ok(table) = l.parse::<toml::Table>() else {
            continue;
        };
        for (k, v) in table {
            let Some(v) = v.as_str() else { continue };
            match k.as_str() {
                "kind" => kind = Some(v.to_string()),
                "method" => method = Some(v.to_string()),
                _ => {}
            }
        }
    }
    match kind.as_deref() {
        Some("http") => Some(
            method
                .map(|m| m.to_ascii_uppercase())
                .unwrap_or_else(|| "GET".into()),
        ),
        Some("grpc") => Some("GRPC".into()),
        Some("websocket") => Some("WS".into()),
        Some("sse") => Some("SSE".into()),
        Some("mqtt") => Some("MQTT".into()),
        Some("mcp") => Some("MCP".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full_http() -> Request {
        Request::Http(HttpRequest {
            method: "POST".into(),
            url: "{{base}}/upload?x=1".into(),
            params: vec![
                KeyValue::new("q", "a b"),
                KeyValue::new("off", "").disabled(),
            ],
            headers: vec![KeyValue::new("X-Quote", "say \"hi\"")],
            body: Body::Multipart {
                parts: vec![
                    MultipartPart {
                        name: "meta".into(),
                        kind: PartKind::Text {
                            value: "{\"a\":1}".into(),
                        },
                        content_type: "application/json".into(),
                        enabled: true,
                    },
                    MultipartPart {
                        name: "file".into(),
                        kind: PartKind::File {
                            path: "files/a.png".into(),
                            filename: Some("b.png".into()),
                        },
                        content_type: String::new(),
                        enabled: false,
                    },
                ],
            },
            auth: Auth::ApiKey {
                key: "k".into(),
                value: "{{secret}}".into(),
                location: ApiKeyLocation::Query,
            },
            options: HttpOptions {
                timeout_ms: Some(1500),
                follow_redirects: false,
                verify_tls: false,
                cookies: None,
            },
            description: String::new(),
            scripts: Scripts {
                pre: vec![SetVar {
                    var: "ts".into(),
                    value: "{{$timestamp}}".into(),
                }],
                extract: vec![
                    Extract {
                        var: "tok".into(),
                        source: Source::Body,
                        expr: "$.token".into(),
                        secret: true,
                    },
                    Extract {
                        var: "code".into(),
                        source: Source::Status,
                        expr: String::new(),
                        secret: false,
                    },
                ],
                asserts: vec![
                    Assertion {
                        source: Source::Status,
                        expr: String::new(),
                        op: AssertOp::Eq,
                        value: "200".into(),
                    },
                    Assertion {
                        source: Source::Header,
                        expr: "content-type".into(),
                        op: AssertOp::Exists,
                        value: String::new(),
                    },
                ],
                ..Scripts::default()
            },
        })
    }

    #[test]
    fn roundtrip_http_variants() {
        let mut reqs = vec![full_http(), Request::new_http()];
        for body in [
            Body::Json {
                text: "{\n  \"a\": \"b\"\n}\n".into(),
            },
            Body::Raw {
                content_type: "text/plain".into(),
                text: "line'''\r\n".into(),
            },
            Body::Form {
                fields: vec![KeyValue::new("a", "1"), KeyValue::new("b", "2").disabled()],
            },
            Body::File {
                path: "x.bin".into(),
                content_type: "application/octet-stream".into(),
            },
            Body::File {
                path: "x.bin".into(),
                content_type: String::new(),
            },
            Body::GraphQL {
                query: "query {{user}} {\n  me { id }\n}\n".into(),
                variables: "{\n  \"n\": {{count}}\n}\n".into(),
                operation_name: "Me".into(),
            },
            Body::GraphQL {
                query: "{ a }".into(),
                variables: String::new(),
                operation_name: String::new(),
            },
            // A half-typed variables document is kept as written.
            Body::GraphQL {
                query: "{ a }".into(),
                variables: "{\"n\":".into(),
                operation_name: String::new(),
            },
        ] {
            for auth in [
                Auth::Inherit,
                Auth::None,
                Auth::Basic {
                    username: "u".into(),
                    password: "p".into(),
                },
                Auth::Bearer { token: "t".into() },
                Auth::OAuth {
                    name: "identity".into(),
                },
                Auth::Digest {
                    username: "u".into(),
                    password: "{{p}}".into(),
                },
                Auth::AwsSigV4 {
                    access_key: "{{ak}}".into(),
                    secret_key: "{{sk}}".into(),
                    session_token: String::new(),
                    region: "eu-west-1".into(),
                    service: "execute-api".into(),
                },
                Auth::AwsSigV4 {
                    access_key: "a".into(),
                    secret_key: "s".into(),
                    session_token: "{{st}}".into(),
                    region: "us-east-1".into(),
                    service: "s3".into(),
                },
                Auth::Jwt {
                    algorithm: JwtAlgorithm::Hs512,
                    secret: "{{jwt_secret}}".into(),
                    private_key: String::new(),
                    claims: "{\n  \"sub\": \"{{user}}\"\n}\n".into(),
                    expires_in_s: Some(300),
                },
                Auth::Jwt {
                    algorithm: JwtAlgorithm::Es256,
                    secret: String::new(),
                    private_key: "keys/es.pem".into(),
                    claims: "{}".into(),
                    expires_in_s: None,
                },
            ] {
                reqs.push(Request::Http(HttpRequest {
                    body: body.clone(),
                    auth,
                    ..Default::default()
                }));
            }
        }
        for r in reqs {
            let t = to_toml(&r);
            let back = parse(&t).unwrap_or_else(|e| panic!("{t}\n{e}"));
            assert_eq!(back, r, "{t}");
            assert_eq!(to_toml(&back), t);
        }
    }

    #[test]
    fn absent_auth_inherits_and_none_is_written() {
        let Request::Http(h) = parse("kind = \"http\"\nurl = \"x\"\n").unwrap() else {
            panic!()
        };
        assert_eq!(h.auth, Auth::Inherit);
        let none = Request::Http(HttpRequest {
            auth: Auth::None,
            ..Default::default()
        });
        assert!(to_toml(&none).contains("[auth]\ntype = \"none\""));
        let inherit = parse("kind = \"http\"\n[auth]\ntype = \"inherit\"\n").unwrap();
        assert!(!to_toml(&inherit).contains("[auth]"));
    }

    #[test]
    fn roundtrip_grpc() {
        let g = Request::Grpc(GrpcRequest {
            target: "localhost:50051".into(),
            service: "echo.Echo".into(),
            method: "Unary".into(),
            message: "{\n  \"text\": \"hi\"\n}".into(),
            metadata: vec![KeyValue::new("x-a", "1")],
            auth: Auth::Bearer {
                token: "{{t}}".into(),
            },
            tls: true,
            verify_tls: false,
            deadline_ms: Some(100),
            schema: SchemaSource::Files {
                files: vec!["echo.proto".into()],
                import_paths: vec!["protos".into()],
            },
            scripts: Scripts::default(),
            // Lines that look like keys stay text for the badge sniff.
            description: "# Echo\n\nkind = \"mcp\"\n[x](y) say \"hi\" '''\n".into(),
        });
        let t = to_toml(&g);
        assert_eq!(parse(&t).ok(), Some(g.clone()));
        let d = Request::new_grpc();
        assert_eq!(parse(&to_toml(&d)).ok(), Some(d));
        assert_eq!(sniff_badge(&t).as_deref(), Some("GRPC"));
    }

    #[test]
    fn roundtrip_mcp() {
        let m = Request::Mcp(McpRequest {
            server: "github".into(),
            tool: "create_issue".into(),
            arguments: "{\n  \"title\": \"{{version}}\"\n}".into(),
            timeout_ms: Some(150),
            scripts: Scripts::default(),
            description: "Creates an *issue*.".into(),
        });
        let t = to_toml(&m);
        assert_eq!(parse(&t).ok(), Some(m.clone()));
        assert_eq!(sniff_badge(&t).as_deref(), Some("MCP"));
        // The default `{}` arguments and no timeout are omitted.
        let d = Request::new_mcp();
        let text = to_toml(&d);
        assert_eq!(text, "kind = \"mcp\"\nserver = \"\"\ntool = \"\"\n");
        assert_eq!(parse(&text).ok(), Some(d));
        // Empty arguments omit the wire property and must survive save/load.
        for arguments in ["", "  ", " {} "] {
            let request = Request::Mcp(McpRequest {
                arguments: arguments.into(),
                ..McpRequest::default()
            });
            assert_eq!(parse(&to_toml(&request)).ok(), Some(request));
        }
        // Invalid JSON is kept as written: it must round-trip so a half-typed
        // object can be saved and reopened; the caller validates before sending.
        for arguments in ["[1]", "nope", "{\"a\":"] {
            let request = Request::Mcp(McpRequest {
                arguments: arguments.into(),
                ..McpRequest::default()
            });
            assert_eq!(parse(&to_toml(&request)).ok(), Some(request));
        }
    }

    #[test]
    fn roundtrip_every_script_source() {
        let asserts = Source::ALL
            .into_iter()
            .map(|source| Assertion {
                source,
                expr: match source {
                    Source::Status | Source::Time => String::new(),
                    Source::Hash => "sha256".into(),
                    _ => "x".into(),
                },
                op: AssertOp::Exists,
                value: String::new(),
            })
            .collect();
        let m = Request::Mcp(McpRequest {
            scripts: Scripts {
                asserts,
                ..Scripts::default()
            },
            ..McpRequest::default()
        });
        let t = to_toml(&m);
        assert_eq!(parse(&t).ok(), Some(m));
        // An empty time expression means `total` and is not written.
        assert!(t.contains("from = \"time\"\nop = \"exists\""), "{t}");
        let bad = "kind = \"mcp\"\n[[scripts.assert]]\nfrom = \"nope\"\nop = \"exists\"\n";
        let err = parse(bad).unwrap_err();
        assert!(err.contains("xpath, regex, cookie"), "{err}");
    }

    #[test]
    fn roundtrip_lua_scripts() {
        let pre = "vars.set(\"a\", 1)\nrequest:set_header(\"X\", '''q''')\n";
        let h = Request::Http(HttpRequest {
            url: "https://x".into(),
            scripts: Scripts {
                pre: vec![SetVar {
                    var: "v".into(),
                    value: "1".into(),
                }],
                lua_pre: LuaScript::own(pre).into_iter().collect(),
                lua_post: LuaScript::own("test(\"ok\", response.status == 200);")
                    .into_iter()
                    .collect(),
                ..Scripts::default()
            },
            ..HttpRequest::default()
        });
        let t = to_toml(&h);
        assert_eq!(parse(&t).ok(), Some(h.clone()));
        assert_eq!(to_toml(&parse(&t).unwrap()), t);
        assert!(t.contains("[[scripts.pre]]"), "{t}");
        assert!(t.contains("\n[scripts.lua]\npre = "), "{t}");
        // A blank script is not written; a post script alone is.
        let mut only_post = h.clone();
        only_post.scripts_mut().lua_pre = Vec::new();
        let t = to_toml(&only_post);
        assert!(!t.contains("pre = "), "{t}");
        assert_eq!(parse(&t).ok(), Some(only_post));
        let blank = "kind = \"mcp\"\n[scripts.lua]\npre = \"  \"\n";
        assert!(parse(blank).unwrap().scripts().is_empty());
        let bad = "kind = \"mcp\"\n[scripts.lua]\nbefore = \"x\"\n";
        assert!(parse(bad).is_err());
    }

    #[test]
    fn roundtrip_ws() {
        let w = Request::Ws(WebSocketRequest {
            url: "wss://{{host}}/chat".into(),
            headers: vec![
                KeyValue::new("origin", "https://app.example"),
                KeyValue::new("x-off", "1").disabled(),
            ],
            subprotocols: vec!["chat.v2".into(), "chat.v1".into()],
            messages: "[\n  \"hello\",\n  {\"binary\": \"AAEC\"}\n]".into(),
            auth: Auth::Bearer {
                token: "{{t}}".into(),
            },
            options: WsOptions {
                timeout_ms: Some(2000),
                deadline_ms: Some(60_000),
                verify_tls: false,
                tls: true,
                cookies: Some(false),
            },
            scripts: Scripts::default(),
            description: "- one\n- two\nmethod = 'post'\n".into(),
        });
        let t = to_toml(&w);
        assert_eq!(parse(&t).ok(), Some(w.clone()));
        assert_eq!(to_toml(&w), t);
        assert_eq!(sniff_badge(&t).as_deref(), Some("WS"));
        let d = Request::new_ws();
        assert_eq!(parse(&to_toml(&d)).ok(), Some(d));
        // A half-typed message document is kept as written; the sender parses it.
        for messages in ["", "  ", "[", "nope", "{\"binary\":"] {
            let request = Request::Ws(WebSocketRequest {
                messages: messages.into(),
                ..WebSocketRequest::default()
            });
            assert_eq!(parse(&to_toml(&request)).ok(), Some(request));
        }
    }

    #[test]
    fn graphql_body_text() {
        let r = Request::Http(HttpRequest {
            method: "POST".into(),
            url: "/graphql".into(),
            body: Body::GraphQL {
                query: "{ me { id } }".into(),
                variables: "{\"x\":1}".into(),
                operation_name: String::new(),
            },
            ..Default::default()
        });
        let t = to_toml(&r);
        assert_eq!(
            t,
            "kind = \"http\"\nmethod = \"POST\"\nurl = \"/graphql\"\n\n[body]\ntype = \"graphql\"\nquery = \"{ me { id } }\"\nvariables = \"{\\\"x\\\":1}\"\n"
        );
        assert_eq!(parse(&t).ok(), Some(r));
    }

    #[test]
    fn canonical_text() {
        let t = to_toml(&full_http());
        let expected = r#"kind = "http"
method = "POST"
url = "{{base}}/upload?x=1"

[options]
timeout_ms = 1500
follow_redirects = false
verify_tls = false

[[params]]
name = "q"
value = "a b"

[[params]]
name = "off"
value = ""
enabled = false

[[headers]]
name = "X-Quote"
value = "say \"hi\""

[auth]
type = "apikey"
key = "k"
value = "{{secret}}"
in = "query"

[body]
type = "multipart"

[[body.parts]]
name = "meta"
value = "{\"a\":1}"
content_type = "application/json"

[[body.parts]]
name = "file"
file = "files/a.png"
filename = "b.png"
enabled = false

[[scripts.pre]]
var = "ts"
value = "{{$timestamp}}"

[[scripts.extract]]
var = "tok"
from = "jsonpath"
expr = "$.token"
secret = true

[[scripts.extract]]
var = "code"
from = "status"

[[scripts.assert]]
from = "status"
op = "eq"
value = "200"

[[scripts.assert]]
from = "header"
expr = "content-type"
op = "exists"
"#;
        assert_eq!(t, expected);
        assert_eq!(sniff_badge(&t).as_deref(), Some("POST"));
    }

    #[test]
    fn errors() {
        assert!(parse("kind = \"ftp\"").is_err());
        assert!(parse("kind = \"http\"\nbogus = 1").is_err());
        assert!(parse("kind = \"http\"\n[body]\ntype = \"weird\"").is_err());
        assert!(parse("kind = \"http\"\n[auth]\ntype = \"weird\"").is_err());
        assert!(parse("kind = \"http\"\n[auth]\ntype = \"apikey\"\nin = \"cookie\"").is_err());
        assert!(parse("kind = \"grpc\"\n[schema]\nsource = \"x\"").is_err());
        assert!(parse("kind = \"http\"\n[[scripts.extract]]\nvar = \"a\"\nfrom = \"x\"").is_err());
        assert!(
            parse("kind = \"http\"\n[[scripts.assert]]\nfrom = \"status\"\nop = \"zz\"").is_err()
        );
        assert!(parse("kind = \"http\"\n[body]\ntype = \"multipart\"\n[[body.parts]]\nname = \"a\"\nvalue = \"1\"\nfile = \"f\"").is_err());
        assert!(
            parse("kind = \"http\"\n[body]\ntype = \"multipart\"\n[[body.parts]]\nname = \"a\"")
                .is_ok()
        );
        assert!(parse("kind = \"http\"\n[auth]\ntype = \"none\"\n[body]\ntype = \"none\"").is_ok());
        assert_eq!(sniff_badge("[package]\nname = \"x\""), None);
        assert_eq!(sniff_badge("kind = 'http'\n"), Some("GET".into()));
        assert_eq!(sniff_badge("x = 1"), None);
        assert_eq!(
            sniff_badge(
                "kind = \"http\"\ndescription = '''\nmethod = \"PUT\"\n[a](b)\n'''\nmethod = \"post\"\n"
            ),
            Some("POST".into())
        );
        assert_eq!(sniff_badge("kind = \"http\" # main\n"), Some("GET".into()));
        assert_eq!(
            sniff_badge("\"kind\" = \"http\"\nmethod = 'post' # create\n"),
            Some("POST".into())
        );
        assert_eq!(sniff_badge("kind = \"grpc\"   \n"), Some("GRPC".into()));
    }
}
