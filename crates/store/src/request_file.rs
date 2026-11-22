//! Request files: `<name>.toml`, one request per file. Schema in FORMAT.md.

use serde::Deserialize;

use sendito_core::model::*;

use crate::tomlfmt::Writer;

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawKv {
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
    tls: Option<bool>,
    deadline_ms: Option<u64>,
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
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawAuth {
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    username: String,
    #[serde(default)]
    password: String,
    #[serde(default)]
    token: String,
    #[serde(default)]
    key: String,
    #[serde(default)]
    value: String,
    #[serde(rename = "in", default)]
    location: Option<String>,
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
struct RawScripts {
    #[serde(default)]
    pre: Vec<RawSet>,
    #[serde(default)]
    extract: Vec<RawExtract>,
    #[serde(default, rename = "assert")]
    asserts: Vec<RawAssert>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRequest {
    kind: String,
    method: Option<String>,
    url: Option<String>,
    target: Option<String>,
    service: Option<String>,
    message: Option<String>,
    server: Option<String>,
    tool: Option<String>,
    arguments: Option<String>,
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
}

fn kvs(v: Vec<RawKv>) -> Vec<KeyValue> {
    v.into_iter()
        .map(|k| KeyValue {
            name: k.name,
            value: k.value,
            enabled: k.enabled,
        })
        .collect()
}

fn auth(a: Option<RawAuth>) -> Result<Auth, String> {
    let Some(a) = a else { return Ok(Auth::None) };
    Ok(match a.kind.as_str() {
        "none" => Auth::None,
        "basic" => Auth::Basic {
            username: a.username,
            password: a.password,
        },
        "bearer" => Auth::Bearer { token: a.token },
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
        o => return Err(format!("unknown auth type {o:?}")),
    })
}

fn scripts(s: RawScripts) -> Result<Scripts, String> {
    let src = |s: &str| {
        Source::parse(s)
            .ok_or_else(|| format!("unknown script source {s:?} (jsonpath, header, status)"))
    };
    Ok(Scripts {
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
            },
            scripts: scripts(r.scripts)?,
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
            }))
        }
        o => Err(format!(
            "unknown request kind {o:?} (expected \"http\", \"grpc\" or \"mcp\")"
        )),
    }
}

fn write_kvs(w: &mut Writer, table: &str, v: &[KeyValue]) {
    for kv in v {
        w.array_table(table);
        w.str("name", &kv.name);
        w.str("value", &kv.value);
        if !kv.enabled {
            w.bool("enabled", false);
        }
    }
}

fn write_auth(w: &mut Writer, a: &Auth) {
    match a {
        Auth::None => {}
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
    }
}

fn write_scripts(w: &mut Writer, s: &Scripts) {
    for p in &s.pre {
        w.array_table("scripts.pre");
        w.str("var", &p.var);
        w.str("value", &p.value);
    }
    for e in &s.extract {
        w.array_table("scripts.extract");
        w.str("var", &e.var);
        w.str("from", e.source.as_str());
        if e.source != Source::Status {
            w.str("expr", &e.expr);
        }
        if e.secret {
            w.bool("secret", true);
        }
    }
    for a in &s.asserts {
        w.array_table("scripts.assert");
        w.str("from", a.source.as_str());
        if a.source != Source::Status {
            w.str("expr", &a.expr);
        }
        w.str("op", a.op.as_str());
        if a.op.takes_value() {
            w.str("value", &a.value);
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
            let o = &h.options;
            if o.timeout_ms.is_some() || !o.follow_redirects || !o.verify_tls {
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
            }
            write_scripts(&mut w, &h.scripts);
        }
        Request::Grpc(g) => {
            w.str("kind", "grpc");
            w.str("target", &g.target);
            w.str("service", &g.service);
            w.str("method", &g.method);
            w.str("message", &g.message);
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
            if let Some(t) = m.timeout_ms {
                w.table("options");
                w.int("timeout_ms", t);
            }
            write_scripts(&mut w, &m.scripts);
        }
    }
    w.finish()
}

/// Cheap sniff of `kind`/`method` from the top of a request file (for tree badges
/// without a full parse). Returns `None` when the file is not a request.
pub fn sniff_badge(text: &str) -> Option<String> {
    let mut kind = None;
    let mut method = None;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            break;
        }
        let Some((k, v)) = l.split_once('=') else {
            continue;
        };
        let v = v.trim().trim_matches('"').trim_matches('\'');
        match k.trim() {
            "kind" => kind = Some(v.to_string()),
            "method" => method = Some(v.to_string()),
            _ => {}
        }
    }
    match kind.as_deref() {
        Some("http") => Some(
            method
                .map(|m| m.to_ascii_uppercase())
                .unwrap_or_else(|| "GET".into()),
        ),
        Some("grpc") => Some("GRPC".into()),
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
            },
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
        ] {
            for auth in [
                Auth::None,
                Auth::Basic {
                    username: "u".into(),
                    password: "p".into(),
                },
                Auth::Bearer { token: "t".into() },
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
    }
}
