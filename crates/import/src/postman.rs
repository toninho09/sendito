//! Postman Collection v2.1 import.
//!
//! Imported: folders, requests (method, URL, query, headers, all body modes,
//! basic/bearer/API-key auth with inheritance), collection variables.
//! Everything else is listed in [`ImportResult::skipped`] with a reason.

use serde_json::Value;

use sendito_core::model::*;

use crate::{ImportFolder, ImportResult, Skipped};

#[derive(Debug, thiserror::Error)]
pub enum PostmanError {
    #[error("not valid JSON: {0}")]
    Json(String),
    #[error("not a Postman collection: {0}")]
    Format(String),
}

struct Ctx {
    skipped: Vec<Skipped>,
}

impl Ctx {
    fn skip(&mut self, item: &str, reason: impl Into<String>) {
        self.skipped.push(Skipped {
            item: item.to_string(),
            reason: reason.into(),
        });
    }
}

fn s(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// A Postman description: a string or `{ "content": "…", "type": "text/markdown" }`.
fn description(v: Option<&Value>) -> String {
    match v {
        Some(Value::Object(o)) => o.get("content").map(s).unwrap_or_default(),
        Some(d) => s(d),
        None => String::new(),
    }
}

fn get_str(v: &Value, k: &str) -> String {
    v.get(k).map(s).unwrap_or_default()
}

fn disabled(v: &Value) -> bool {
    v.get("disabled").and_then(Value::as_bool).unwrap_or(false)
}

/// Postman dynamic variables with a Sendito equivalent.
fn map_dynamic(text: &str, item: &str, ctx: &mut Ctx) -> String {
    if !text.contains("{{$") {
        return text.to_string();
    }
    for alias in ["{{$guid}}", "{{$randomUUID}}"] {
        if text.contains(alias) {
            ctx.skip(
                item,
                format!("{alias} became {{{{$uuid}}}} (both give a random UUID)"),
            );
        }
    }
    let mut out = text
        .replace("{{$guid}}", "{{$uuid}}")
        .replace("{{$randomUUID}}", "{{$uuid}}");
    for name in sendito_core::template::var_names(&out.clone()) {
        if name.starts_with('$') && !sendito_core::vars::is_builtin(name) {
            ctx.skip(
                item,
                format!("{{{{{name}}}}} has no Sendito equivalent; it is sent as written"),
            );
        }
    }
    out.shrink_to_fit();
    out
}

fn kv_list(v: Option<&Value>, item: &str, ctx: &mut Ctx) -> Vec<KeyValue> {
    v.and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|e| KeyValue {
                    name: map_dynamic(&get_str(e, "key"), item, ctx),
                    value: map_dynamic(&get_str(e, "value"), item, ctx),
                    enabled: !disabled(e),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn auth_param(arr: Option<&Value>, key: &str) -> String {
    arr.and_then(Value::as_array)
        .and_then(|a| a.iter().find(|e| get_str(e, "key") == key))
        .map(|e| get_str(e, "value"))
        .unwrap_or_default()
}

/// Converts a Postman auth object. `None` means "inherit".
fn auth(v: Option<&Value>, item: &str, ctx: &mut Ctx) -> Option<Auth> {
    let v = v?;
    if v.is_null() {
        return None;
    }
    let t = get_str(v, "type");
    let d = |k: &str, ctx: &mut Ctx| map_dynamic(k, item, ctx);
    match t.as_str() {
        "inherit" | "" => None,
        "noauth" => Some(Auth::None),
        "basic" => {
            let username = auth_param(v.get("basic"), "username");
            let password = auth_param(v.get("basic"), "password");
            Some(Auth::Basic {
                username: d(&username, ctx),
                password: d(&password, ctx),
            })
        }
        "bearer" => {
            let token = auth_param(v.get("bearer"), "token");
            Some(Auth::Bearer {
                token: d(&token, ctx),
            })
        }
        "apikey" => {
            let a = v.get("apikey");
            let key = auth_param(a, "key");
            let value = auth_param(a, "value");
            let location = if auth_param(a, "in") == "query" {
                ApiKeyLocation::Query
            } else {
                ApiKeyLocation::Header
            };
            Some(Auth::ApiKey {
                key: d(&key, ctx),
                value: d(&value, ctx),
                location,
            })
        }
        "digest" => {
            let a = v.get("digest");
            Some(Auth::Digest {
                username: d(&auth_param(a, "username"), ctx),
                password: d(&auth_param(a, "password"), ctx),
            })
        }
        "awsv4" => {
            let a = v.get("awsv4");
            Some(Auth::AwsSigV4 {
                access_key: d(&auth_param(a, "accessKey"), ctx),
                secret_key: d(&auth_param(a, "secretKey"), ctx),
                session_token: d(&auth_param(a, "sessionToken"), ctx),
                region: d(&auth_param(a, "region"), ctx),
                service: d(&auth_param(a, "service"), ctx),
            })
        }
        "jwt" => {
            let a = v.get("jwt");
            let name = auth_param(a, "algorithm");
            let Some(algorithm) =
                JwtAlgorithm::parse(if name.is_empty() { "HS256" } else { &name })
            else {
                ctx.skip(
                    item,
                    format!(
                        "its JWT algorithm {name} is not supported, so the request has no auth"
                    ),
                );
                return Some(Auth::None);
            };
            if !algorithm.uses_secret() {
                ctx.skip(
                    item,
                    "its JWT private key was not imported: save the PEM in a file and set the key path",
                );
            }
            if auth_param(a, "isSecretBase64Encoded") == "true" {
                ctx.skip(
                    item,
                    "its JWT secret is base64-encoded in Postman; Sendito uses it as text",
                );
            }
            if auth_param(a, "addTokenTo") == "queryParam" {
                ctx.skip(
                    item,
                    "its JWT went in the query; Sendito sends it in the Authorization header",
                );
            }
            let claims = auth_param(a, "payload");
            Some(Auth::Jwt {
                algorithm,
                secret: d(&auth_param(a, "secret"), ctx),
                private_key: String::new(),
                claims: if claims.trim().is_empty() {
                    "{}".into()
                } else {
                    d(&claims, ctx)
                },
                expires_in_s: None,
            })
        }
        other => {
            ctx.skip(
                item,
                format!("its \"{other}\" auth is not supported, so the request has no auth"),
            );
            Some(Auth::None)
        }
    }
}

/// A request's saved responses (`response[]`): name, status code and text,
/// headers and body.
fn examples(v: Option<&Value>) -> Vec<sendito_store::examples::Example> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|r| {
            let status = r.get("code").and_then(Value::as_u64).unwrap_or(200) as u16;
            let reason = get_str(r, "status");
            let name = get_str(r, "name");
            sendito_store::examples::Example {
                name: if name.trim().is_empty() {
                    sendito_store::examples::Example::default_name(status, &reason)
                } else {
                    name
                },
                status,
                reason,
                headers: r
                    .get("header")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(|h| (get_str(h, "key"), get_str(h, "value")))
                    .filter(|(k, _)| !k.is_empty())
                    .collect(),
                body: get_str(r, "body").into_bytes(),
            }
        })
        .collect()
}

fn url_of(v: Option<&Value>, item: &str, ctx: &mut Ctx) -> (String, Vec<KeyValue>) {
    match v {
        None | Some(Value::Null) => (String::new(), vec![]),
        Some(Value::String(raw)) => (map_dynamic(raw, item, ctx), vec![]),
        Some(u) => {
            let mut raw = get_str(u, "raw");
            if !raw.is_empty() {
                // The structured parts can disagree with `raw`; Sendito keeps
                // `raw` and says so.
                let host = match u.get("host") {
                    Some(Value::Array(a)) => a.iter().map(s).collect::<Vec<_>>().join("."),
                    Some(h) => s(h),
                    None => String::new(),
                };
                if !host.is_empty() && !raw.contains(&host) {
                    let proto = get_str(u, "protocol");
                    let structured = if proto.is_empty() {
                        host
                    } else {
                        format!("{proto}://{host}")
                    };
                    ctx.skip(
                        item,
                        format!(
                            "the URL comes from the \"raw\" field; the host field \
                             (\"{structured}\") differs from it and was ignored"
                        ),
                    );
                }
            }
            if raw.is_empty() {
                // Rebuild from parts when `raw` is missing.
                let host = match u.get("host") {
                    Some(Value::Array(a)) => a.iter().map(s).collect::<Vec<_>>().join("."),
                    Some(h) => s(h),
                    None => String::new(),
                };
                let path = match u.get("path") {
                    Some(Value::Array(a)) => a.iter().map(s).collect::<Vec<_>>().join("/"),
                    Some(p) => s(p),
                    None => String::new(),
                };
                let proto = get_str(u, "protocol");
                let port = get_str(u, "port");
                raw = String::new();
                if !proto.is_empty() {
                    raw.push_str(&proto);
                    raw.push_str("://");
                }
                raw.push_str(&host);
                if !port.is_empty() {
                    raw.push(':');
                    raw.push_str(&port);
                }
                if !path.is_empty() {
                    raw.push('/');
                    raw.push_str(&path);
                }
                let q: Vec<String> = u
                    .get("query")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter(|e| !disabled(e))
                            .map(|e| match e.get("value") {
                                Some(Value::Null) | None => get_str(e, "key"),
                                Some(v) => format!("{}={}", get_str(e, "key"), s(v)),
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                if !q.is_empty() {
                    raw.push('?');
                    raw.push_str(&q.join("&"));
                }
            }
            // Path variables (`:id`) are substituted with their values.
            if let Some(vars) = u.get("variable").and_then(Value::as_array) {
                for var in vars {
                    let key = get_str(var, "key");
                    if key.is_empty() {
                        continue;
                    }
                    let value = get_str(var, "value");
                    let pat = format!(":{key}");
                    if let Some(pos) = find_path_var(&raw, &pat) {
                        raw.replace_range(pos..pos + pat.len(), &value);
                        ctx.skip(
                            item,
                            format!(":{key} in the URL was replaced by its value \"{value}\""),
                        );
                    }
                }
            }
            // The raw URL already carries the enabled query; disabled entries are
            // kept as disabled params so they stay visible.
            let disabled_params = u
                .get("query")
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter(|e| disabled(e))
                        .map(|e| KeyValue {
                            name: get_str(e, "key"),
                            value: get_str(e, "value"),
                            enabled: false,
                        })
                        .collect()
                })
                .unwrap_or_default();
            (map_dynamic(&raw, item, ctx), disabled_params)
        }
    }
}

fn find_path_var(raw: &str, pat: &str) -> Option<usize> {
    let path_end = raw.find(['?', '#']).unwrap_or(raw.len());
    let mut from = 0;
    while let Some(i) = raw[from..path_end].find(pat) {
        let at = from + i;
        let end = at + pat.len();
        let boundary_ok =
            raw[..at].ends_with('/') && (end == path_end || raw[end..].starts_with('/'));
        if boundary_ok {
            return Some(at);
        }
        from = end;
    }
    None
}

/// Postman stores file paths from the machine that made the collection; an
/// absolute one is kept as is but will often not exist here.
fn note_file_path(path: &str, item: &str, ctx: &mut Ctx) {
    if path.starts_with('/') || path.get(1..3) == Some(":\\") {
        ctx.skip(
            item,
            format!(
                "file {path} is an absolute path from the computer that made the collection; \
                 kept as is (copy the file into the collection and use a relative path)"
            ),
        );
    }
}

fn raw_content_type(lang: &str) -> &'static str {
    match lang {
        "javascript" => "application/javascript",
        "html" => "text/html",
        "xml" => "application/xml",
        "json" => "application/json",
        _ => "text/plain",
    }
}

fn body(v: Option<&Value>, item: &str, ctx: &mut Ctx) -> Body {
    let Some(b) = v.filter(|b| !b.is_null()) else {
        return Body::None;
    };
    if b.get("disabled").and_then(Value::as_bool) == Some(true) {
        return Body::None;
    }
    let mode = get_str(b, "mode");
    match mode.as_str() {
        "raw" => {
            let text = map_dynamic(&get_str(b, "raw"), item, ctx);
            let lang = b
                .get("options")
                .and_then(|o| o.get("raw"))
                .and_then(|r| r.get("language"))
                .map(s)
                .unwrap_or_default();
            if lang == "json" {
                Body::Json { text }
            } else {
                // Postman's default raw language is "text", which it sends
                // as `Content-Type: text/plain`.
                Body::Raw {
                    content_type: raw_content_type(&lang).into(),
                    text,
                }
            }
        }
        "urlencoded" => Body::Form {
            fields: kv_list(b.get("urlencoded"), item, ctx),
        },
        "formdata" => {
            let mut parts = Vec::new();
            for e in b
                .get("formdata")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let name = get_str(e, "key");
                let ct = get_str(e, "contentType");
                let enabled = !disabled(e);
                if get_str(e, "type") == "file" {
                    let srcs: Vec<String> = match e.get("src") {
                        Some(Value::Array(a)) => a.iter().map(s).collect(),
                        Some(Value::Null) | None => vec![],
                        Some(v) => vec![s(v)],
                    };
                    if srcs.is_empty() {
                        ctx.skip(item, format!("form-data file part \"{name}\" has no file selected; imported with an empty path"));
                        parts.push(MultipartPart {
                            name: name.clone(),
                            kind: PartKind::File {
                                path: String::new(),
                                filename: None,
                            },
                            content_type: ct.clone(),
                            enabled,
                        });
                    }
                    for src in srcs {
                        note_file_path(&src, item, ctx);
                        parts.push(MultipartPart {
                            name: name.clone(),
                            kind: PartKind::File {
                                path: src,
                                filename: None,
                            },
                            content_type: ct.clone(),
                            enabled,
                        });
                    }
                } else {
                    parts.push(MultipartPart {
                        name,
                        kind: PartKind::Text {
                            value: map_dynamic(&get_str(e, "value"), item, ctx),
                        },
                        content_type: ct,
                        enabled,
                    });
                }
            }
            Body::Multipart { parts }
        }
        "file" => {
            let path = b.get("file").map(|f| get_str(f, "src")).unwrap_or_default();
            note_file_path(&path, item, ctx);
            // Postman sends a file body with the type of the file; the type is
            // not stored in the collection, so it is derived from the extension
            // here and kept in the request (and reported).
            let content_type = mime_guess::from_path(&path)
                .first_raw()
                .filter(|t| *t != "application/octet-stream")
                .map(str::to_string)
                .unwrap_or_default();
            if !content_type.is_empty() {
                ctx.skip(
                    item,
                    format!(
                        "the file body's Content-Type ({content_type}) is not in the collection; \
                         it was set from the file extension, as Postman does when sending"
                    ),
                );
            }
            Body::File { path, content_type }
        }
        "graphql" => {
            let g = b.get("graphql").cloned().unwrap_or(Value::Null);
            Body::GraphQL {
                query: get_str(&g, "query"),
                // Postman keeps the variables as JSON text; an unparsable text
                // is kept as written and reported when the request is sent.
                variables: get_str(&g, "variables"),
                operation_name: String::new(),
            }
        }
        "" => Body::None,
        other => {
            ctx.skip(
                item,
                format!("its \"{other}\" body is not supported, so the request has no body"),
            );
            Body::None
        }
    }
}

fn request(item: &Value, path: &str, ctx: &mut Ctx) -> Request {
    let r = item.get("request").cloned().unwrap_or(Value::Null);
    if let Value::String(url) = &r {
        return Request::Http(HttpRequest {
            url: url.clone(),
            ..Default::default()
        });
    }
    let method = {
        let m = get_str(&r, "method");
        if m.is_empty() {
            "GET".into()
        } else {
            m.to_ascii_uppercase()
        }
    };
    let (url, params) = url_of(r.get("url"), path, ctx);
    let headers = kv_list(r.get("header"), path, ctx);
    // No auth (or "inherit") inherits from the folder settings, as in Postman.
    let auth = auth(r.get("auth"), path, ctx).unwrap_or_default();
    let body = body(r.get("body"), path, ctx);
    let description = description(r.get("description").or_else(|| item.get("description")));
    if item
        .get("event")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
    {
        ctx.skip(path, "its JavaScript scripts were not imported (Sendito runs no JavaScript; rebuild checks in the Scripts tab)");
    }
    if item
        .get("protocolProfileBehavior")
        .is_some_and(|p| p.as_object().is_some_and(|o| !o.is_empty()))
    {
        ctx.skip(
            path,
            "its advanced Postman settings (protocolProfileBehavior) were not imported",
        );
    }
    Request::Http(HttpRequest {
        method,
        url,
        params,
        headers,
        body,
        auth,
        // Postman follows redirects and verifies TLS by default, like Sendito.
        options: HttpOptions::default(),
        scripts: Scripts::default(),
        description,
    })
}

fn folder(items: &[Value], name: &str, path: &str, auth_of: Auth, ctx: &mut Ctx) -> ImportFolder {
    let mut f = ImportFolder {
        name: name.to_string(),
        ..Default::default()
    };
    f.settings.auth = auth_of;
    for it in items {
        let iname = {
            let n = get_str(it, "name");
            if n.is_empty() {
                "untitled".to_string()
            } else {
                n
            }
        };
        let ipath = if path.is_empty() {
            iname.clone()
        } else {
            format!("{path} / {iname}")
        };
        if let Some(children) = it.get("item").and_then(Value::as_array) {
            let auth = auth(it.get("auth"), &ipath, ctx).unwrap_or_default();
            if it
                .get("event")
                .and_then(Value::as_array)
                .is_some_and(|a| !a.is_empty())
            {
                ctx.skip(&ipath, "the folder's JavaScript scripts were not imported (Sendito runs no JavaScript)");
            }
            let mut sub = folder(children, &iname, &ipath, auth, ctx);
            sub.settings.description = description(it.get("description"));
            f.folders.push(sub);
        } else if it.get("request").is_some() {
            let req = request(it, &ipath, ctx);
            let examples = examples(it.get("response"));
            if !examples.is_empty() {
                f.examples.insert(f.requests.len(), examples);
            }
            f.requests.push((iname, req));
        } else {
            ctx.skip(&ipath, "item has neither a request nor sub-items");
        }
    }
    f
}

/// Parses a Postman Collection v2.0/v2.1 JSON document.
pub fn import(json: &str) -> Result<ImportResult, PostmanError> {
    let v: Value = serde_json::from_str(json).map_err(|e| PostmanError::Json(e.to_string()))?;
    let info = v
        .get("info")
        .ok_or_else(|| PostmanError::Format("missing \"info\"".into()))?;
    let schema = get_str(info, "schema");
    if !schema.is_empty() && !schema.contains("v2.1") && !schema.contains("v2.0") {
        return Err(PostmanError::Format(format!("unsupported schema {schema}")));
    }
    let items = v
        .get("item")
        .and_then(Value::as_array)
        .ok_or_else(|| PostmanError::Format("missing \"item\" array".into()))?;
    let name = {
        let n = get_str(info, "name");
        if n.is_empty() {
            "Imported".to_string()
        } else {
            n
        }
    };
    let mut ctx = Ctx {
        skipped: Vec::new(),
    };
    let root_auth = auth(v.get("auth"), "(collection)", &mut ctx).unwrap_or_default();
    if v.get("event")
        .and_then(Value::as_array)
        .is_some_and(|a| !a.is_empty())
    {
        ctx.skip(
            "(collection)",
            "the collection's JavaScript scripts were not imported (Sendito runs no JavaScript)",
        );
    }
    let mut root = folder(items, &name, "", root_auth, &mut ctx);
    root.settings.description = description(info.get("description"));
    let mut variables = Vec::new();
    for var in v
        .get("variable")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let k = get_str(var, "key");
        if k.is_empty() {
            continue;
        }
        if disabled(var) {
            ctx.skip(
                "(collection)",
                format!("variable \"{k}\" was disabled in Postman and was not imported"),
            );
            continue;
        }
        variables.push((k, get_str(var, "value")));
    }
    report_undefined(&root, "", &variables, &mut ctx);
    // One entry per (item, reason).
    let mut seen = std::collections::HashSet::new();
    ctx.skipped
        .retain(|s| seen.insert((s.item.clone(), s.reason.clone())));
    Ok(ImportResult {
        root,
        variables,
        environments: vec![],
        skipped: ctx.skipped,
    })
}

/// Lists variables that requests use but the collection does not define
/// (Postman environments are not part of the collection file).
fn report_undefined(
    root: &ImportFolder,
    prefix: &str,
    defined: &[(String, String)],
    ctx: &mut Ctx,
) {
    let mut uses: Vec<(String, Vec<String>)> = Vec::new();
    collect_uses(root, prefix, &mut uses);
    for (name, items) in uses {
        if name.starts_with('$') || defined.iter().any(|(k, _)| *k == name) {
            continue;
        }
        let mut list = items.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
        if items.len() > 3 {
            list.push_str(&format!(" and {} more", items.len() - 3));
        }
        ctx.skip(
            "(collection)",
            format!(
                "{{{{{name}}}}} is used by {list} but the collection does not define it; \
                 set it in an environment"
            ),
        );
    }
}

fn collect_uses(f: &ImportFolder, prefix: &str, uses: &mut Vec<(String, Vec<String>)>) {
    // A folder's auth is used by the folder itself (its `folder.toml`).
    let folder = if prefix.is_empty() {
        format!("{} (folder)", f.name)
    } else {
        format!("{prefix} (folder)")
    };
    add_uses(&sendito_store::folder::to_toml(&f.settings), &folder, uses);
    for (name, req) in &f.requests {
        let item = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix} / {name}")
        };
        add_uses(&sendito_store::request_file::to_toml(req), &item, uses);
    }
    for sub in &f.folders {
        let p = if prefix.is_empty() {
            sub.name.clone()
        } else {
            format!("{prefix} / {}", sub.name)
        };
        collect_uses(sub, &p, uses);
    }
}

fn add_uses(text: &str, item: &str, uses: &mut Vec<(String, Vec<String>)>) {
    for v in sendito_core::template::var_names(text) {
        match uses.iter_mut().find(|(n, _)| n == v) {
            Some((_, items)) => {
                if !items.iter().any(|i| i == item) {
                    items.push(item.to_string());
                }
            }
            None => uses.push((v.to_string(), vec![item.to_string()])),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"{
      "info": {"name": "Demo", "description": "Demo *API*", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
      "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}", "type": "string"}]},
      "variable": [{"key": "base", "value": "https://api.example.com"}, {"key": "off", "value": "1", "disabled": true}],
      "event": [{"listen": "prerequest", "script": {"exec": ["console.log(1)"]}}],
      "item": [
        {"name": "Users", "description": {"content": "User *endpoints*", "type": "text/markdown"}, "auth": {"type": "basic", "basic": [{"key": "username", "value": "u"}, {"key": "password", "value": "p"}]},
         "item": [
          {"name": "List", "request": {"method": "get", "url": {"raw": "{{base}}/users?page=1", "query": [{"key": "page", "value": "1"}, {"key": "debug", "value": "1", "disabled": true}]}}},
          {"name": "Get/one", "request": {"method": "GET", "auth": {"type": "noauth"}, "url": {"raw": "{{base}}/users/:id", "variable": [{"key": "id", "value": "42"}]}}},
          {"name": "Create", "event": [{"listen": "test"}], "response": [{}], "request": {"method": "POST", "header": [{"key": "X-Id", "value": "{{$guid}}"}, {"key": "Off", "value": "x", "disabled": true}],
            "body": {"mode": "raw", "raw": "{\"a\":1}", "options": {"raw": {"language": "json"}}}, "url": "{{base}}/users"}}
        ]},
        {"name": "Form", "request": {"method": "POST", "url": "{{base}}/f", "body": {"mode": "urlencoded", "urlencoded": [{"key": "a", "value": "1"}]}}},
        {"name": "Upload", "request": {"method": "POST", "url": "{{base}}/u", "auth": {"type": "apikey", "apikey": [{"key": "key", "value": "k"}, {"key": "value", "value": "v"}, {"key": "in", "value": "query"}]},
          "body": {"mode": "formdata", "formdata": [{"key": "t", "value": "x", "type": "text"}, {"key": "f", "type": "file", "src": ["/a.png", "/b.png"]}, {"key": "e", "type": "file"}]}}},
        {"name": "Bin", "request": {"method": "PUT", "url": "{{base}}/b", "body": {"mode": "file", "file": {"src": "/data.bin"}}}},
        {"name": "GQL", "request": {"method": "POST", "url": "{{base}}/graphql", "body": {"mode": "graphql", "graphql": {"query": "{ me { id } }", "variables": "{\"x\":1}"}}}},
        {"name": "OAuth", "request": {"method": "GET", "url": "{{base}}/o", "auth": {"type": "oauth2"}, "description": "docs"}},
        {"name": "Parts", "request": {"method": "GET", "url": {"protocol": "https", "host": ["api", "example", "com"], "port": "8443", "path": ["v1", "x"], "query": [{"key": "flag", "value": null}, {"key": "a", "value": "b"}]}}},
        {"name": "Text", "request": {"method": "POST", "url": "x", "body": {"mode": "raw", "raw": "<a/>", "options": {"raw": {"language": "xml"}}}}},
        {"name": "Plain", "request": "https://example.com/plain"},
        {"name": "Weird"}
      ]
    }"#;

    #[test]
    fn process_env_is_kept_without_a_report() {
        let mut ctx = Ctx { skipped: vec![] };
        let out = map_dynamic("{{$processEnv.TOKEN}} {{$nope}}", "r", &mut ctx);
        assert_eq!(out, "{{$processEnv.TOKEN}} {{$nope}}");
        assert_eq!(ctx.skipped.len(), 1);
        assert!(ctx.skipped[0].reason.contains("$nope"));
    }

    #[test]
    fn imports_sample() {
        let r = import(SAMPLE).unwrap();
        assert_eq!(r.root.name, "Demo");
        assert_eq!(r.root.count_requests(), 11);
        assert_eq!(
            r.variables,
            vec![("base".to_string(), "https://api.example.com".to_string())]
        );
        let users = &r.root.folders[0];
        let Request::Http(list) = &users.requests[0].1 else {
            panic!()
        };
        assert_eq!(list.method, "GET");
        assert_eq!(list.url, "{{base}}/users?page=1");
        assert_eq!(list.params, vec![KeyValue::new("debug", "1").disabled()]);
        // The folder's auth goes to its settings; the request inherits it.
        assert_eq!(list.auth, Auth::Inherit);
        assert_eq!(
            users.settings.auth,
            Auth::Basic {
                username: "u".into(),
                password: "p".into()
            }
        );
        let Request::Http(one) = &users.requests[1].1 else {
            panic!()
        };
        assert_eq!(one.url, "{{base}}/users/42");
        assert_eq!(one.auth, Auth::None);
        let Request::Http(create) = &users.requests[2].1 else {
            panic!()
        };
        // Its saved response is an example (an empty one gets a default name).
        assert_eq!(users.examples[&2][0].name, "200");
        assert_eq!(create.headers[0].value, "{{$uuid}}");
        assert!(!create.headers[1].enabled);
        assert_eq!(
            create.body,
            Body::Json {
                text: "{\"a\":1}".into()
            }
        );
        let Request::Http(form) = &r.root.requests[0].1 else {
            panic!()
        };
        assert_eq!(form.auth, Auth::Inherit);
        assert_eq!(
            r.root.settings.auth,
            Auth::Bearer {
                token: "{{token}}".into()
            }
        );
        assert!(matches!(&form.body, Body::Form { fields } if fields.len() == 1));
        let Request::Http(up) = &r.root.requests[1].1 else {
            panic!()
        };
        assert!(matches!(&up.body, Body::Multipart { parts } if parts.len() == 4));
        assert!(matches!(
            up.auth,
            Auth::ApiKey {
                location: ApiKeyLocation::Query,
                ..
            }
        ));
        let Request::Http(bin) = &r.root.requests[2].1 else {
            panic!()
        };
        assert_eq!(
            bin.body,
            Body::File {
                path: "/data.bin".into(),
                content_type: String::new()
            }
        );
        let Request::Http(gql) = &r.root.requests[3].1 else {
            panic!()
        };
        assert_eq!(
            gql.body,
            Body::GraphQL {
                query: "{ me { id } }".into(),
                variables: "{\"x\":1}".into(),
                operation_name: String::new(),
            }
        );
        let Request::Http(parts) = &r.root.requests[5].1 else {
            panic!()
        };
        assert_eq!(parts.url, "https://api.example.com:8443/v1/x?flag&a=b");
        let Request::Http(text) = &r.root.requests[6].1 else {
            panic!()
        };
        assert_eq!(
            text.body,
            Body::Raw {
                content_type: "application/xml".into(),
                text: "<a/>".into()
            }
        );
        let Request::Http(plain) = &r.root.requests[7].1 else {
            panic!()
        };
        assert_eq!(plain.url, "https://example.com/plain");
        // Descriptions of the collection, a folder and a request.
        assert_eq!(r.root.settings.description, "Demo *API*");
        assert_eq!(users.settings.description, "User *endpoints*");
        let Request::Http(oauth) = &r.root.requests[4].1 else {
            panic!()
        };
        assert_eq!(oauth.description, "docs");
        let reasons: Vec<String> = r
            .skipped
            .iter()
            .map(|s| format!("{}: {}", s.item, s.reason))
            .collect();
        let joined = reasons.join("\n");
        for needle in [
            "collection's JavaScript scripts",
            "was disabled in Postman",
            ":id in the URL",
            "its JavaScript scripts",
            "oauth2",
            "Weird",
            "no file selected",
        ] {
            assert!(joined.contains(needle), "missing {needle} in\n{joined}");
        }
    }

    #[test]
    fn errors() {
        assert!(import("nope").is_err());
        assert!(import("{}").is_err());
        assert!(import(r#"{"info": {"schema": "v1"}, "item": []}"#).is_err());
        assert!(import(r#"{"info": {}}"#).is_err());
        let r = import(r#"{"info": {}, "item": []}"#).unwrap();
        assert_eq!(r.root.name, "Imported");
        assert_eq!(find_path_var("/a/:id/b", ":id"), Some(3));
        assert_eq!(find_path_var("/a/:idx", ":id"), None);
    }
}
