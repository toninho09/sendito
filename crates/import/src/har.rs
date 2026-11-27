//! HAR 1.2 (HTTP Archive): import the requests a browser recorded, and
//! export requests as a HAR document without responses.
//!
//! An import keeps the URL as recorded, query included, so the bytes of the
//! query do not change. A recorded response becomes a saved example.

use std::path::Path;

use base64::Engine;
use serde_json::{Value, json};

use sendito_core::model::{Auth, Body, HttpRequest, KeyValue, Request};
use sendito_core::resolve::{PartData, ResolvedBody};
use sendito_core::vars::Scope;
use sendito_store::examples::Example;

use crate::{ImportFolder, ImportResult, Skipped};

/// Headers that the HTTP client writes itself, or that HTTP/2 shows as
/// pseudo-headers: an import leaves them out.
fn framing_header(name: &str) -> bool {
    name.starts_with(':')
        || ["host", "content-length", "connection", "transfer-encoding"]
            .iter()
            .any(|h| name.eq_ignore_ascii_case(h))
}

fn s(v: &Value, key: &str) -> String {
    v.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn pairs(v: Option<&Value>) -> Vec<(String, String)> {
    v.and_then(Value::as_array)
        .map(|a| a.iter().map(|h| (s(h, "name"), s(h, "value"))).collect())
        .unwrap_or_default()
}

/// Whether a JSON document is a HAR file (`log.entries` is an array).
pub fn is_har(doc: &Value) -> bool {
    doc.get("log")
        .and_then(|l| l.get("entries"))
        .is_some_and(Value::is_array)
}

/// `scheme://host[:port]` and the path of a URL, without the query.
fn host_and_path(url: &str) -> Option<(String, String)> {
    let (scheme, rest) = url.split_once("://")?;
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let host = &rest[..end];
    let path = rest[end..].split(['?', '#']).next().unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };
    Some((format!("{scheme}://{host}"), path.to_string()))
}

fn body(post: Option<&Value>, item: &str, skipped: &mut Vec<Skipped>) -> Body {
    let Some(post) = post.filter(|p| p.is_object()) else {
        return Body::None;
    };
    let mime = s(post, "mimeType");
    let text = s(post, "text");
    if mime.to_ascii_lowercase().starts_with("multipart/") {
        skipped.push(Skipped {
            item: item.into(),
            reason: "the multipart body was imported as raw text: a HAR file does not hold the file parts"
                .into(),
        });
    }
    if text.is_empty() {
        return Body::None;
    }
    if mime.to_ascii_lowercase().contains("json") {
        Body::Json { text }
    } else {
        Body::Raw {
            content_type: mime,
            text,
        }
    }
}

fn example(resp: Option<&Value>) -> Option<Example> {
    let resp = resp?;
    let status = resp.get("status").and_then(Value::as_u64)?;
    let status = u16::try_from(status).ok().filter(|s| *s > 0)?;
    let content = resp.get("content").cloned().unwrap_or(Value::Null);
    let text = s(&content, "text");
    let body = if s(&content, "encoding") == "base64" {
        base64::engine::general_purpose::STANDARD
            .decode(text.as_bytes())
            .unwrap_or_else(|_| text.into_bytes())
    } else {
        text.into_bytes()
    };
    let reason = s(resp, "statusText");
    Some(Example {
        name: Example::default_name(status, &reason),
        status,
        reason,
        headers: pairs(resp.get("headers"))
            .into_iter()
            .filter(|(n, _)| !n.starts_with(':'))
            .collect(),
        body,
    })
}

/// Reads a HAR document: one folder per host, one request per entry.
pub fn import(text: &str) -> Result<ImportResult, String> {
    let doc: Value = serde_json::from_str(text).map_err(|e| format!("not valid JSON: {e}"))?;
    if !is_har(&doc) {
        return Err("not a HAR file: log.entries is missing".into());
    }
    let mut result = ImportResult {
        root: ImportFolder {
            name: "HAR".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let mut cookies_noted = false;
    let entries = doc["log"]["entries"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for (i, entry) in entries.iter().enumerate() {
        let req = entry.get("request").cloned().unwrap_or(Value::Null);
        let url = s(&req, "url");
        let item = format!("entry {}", i + 1);
        let Some((host, path)) = host_and_path(&url)
            .filter(|_| url.starts_with("http://") || url.starts_with("https://"))
        else {
            result.skipped.push(Skipped {
                item,
                reason: format!("not an HTTP request: {url:?}"),
            });
            continue;
        };
        let method = match s(&req, "method").to_ascii_uppercase() {
            m if m.is_empty() => "GET".to_string(),
            m => m,
        };
        // An export from Sendito names each request in `comment`.
        let name = match s(entry, "comment") {
            c if !c.trim().is_empty() => c.trim().to_string(),
            _ => format!("{method} {path}"),
        };
        let item = format!("{host} / {name}");
        let mut headers = Vec::new();
        for (n, v) in pairs(req.get("headers")) {
            if framing_header(&n) {
                continue;
            }
            if n.eq_ignore_ascii_case("cookie") && !cookies_noted {
                cookies_noted = true;
                result.skipped.push(Skipped {
                    item: item.clone(),
                    reason: "Cookie headers of the browser session were imported as they are: remove them or use the cookie jar".into(),
                });
            }
            headers.push(KeyValue::new(&n, &v));
        }
        let request = Request::Http(HttpRequest {
            method,
            url,
            headers,
            body: body(req.get("postData"), &item, &mut result.skipped),
            auth: Auth::None,
            ..Default::default()
        });
        let folder = match result.root.folders.iter().position(|f| f.name == host) {
            Some(i) => &mut result.root.folders[i],
            None => {
                result.root.folders.push(ImportFolder {
                    name: host.clone(),
                    ..Default::default()
                });
                let last = result.root.folders.len() - 1;
                &mut result.root.folders[last]
            }
        };
        if let Some(ex) = example(entry.get("response")) {
            folder.examples.insert(folder.requests.len(), vec![ex]);
        }
        folder.requests.push((name, request));
    }
    Ok(result)
}

/// The `Authorization` value an export writes for Basic auth: the encoded
/// credentials, or a placeholder while a secret stays a placeholder.
fn basic_header(user: &str, pass: &str) -> String {
    if user.contains("{{") || pass.contains("{{") {
        "Basic {{sendito_basic_credentials}}".into()
    } else {
        let enc = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{pass}"));
        format!("Basic {enc}")
    }
}

fn query_string(url: &str) -> Vec<Value> {
    let query = url
        .split_once('?')
        .map(|(_, q)| q.split('#').next().unwrap_or(""))
        .unwrap_or("");
    query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (n, v) = p.split_once('=').unwrap_or((p, ""));
            json!({"name": n, "value": v})
        })
        .collect()
}

fn entry(
    name: &str,
    req: &HttpRequest,
    scope: &Scope,
    include_secrets: bool,
    base_dir: &Path,
) -> Value {
    let mut comments = vec![];
    let p = match crate::prepare(req, scope, include_secrets, base_dir) {
        Ok(p) => p,
        Err(message) => {
            comments.push(format!("cannot export the body: {message}"));
            let mut no_body = req.clone();
            no_body.body = Body::None;
            match crate::prepare(&no_body, scope, include_secrets, base_dir) {
                Ok(p) => p,
                Err(_) => return json!({"comment": name}),
            }
        }
    };
    let r = &p.resolved;
    let mut headers: Vec<(String, String)> = r.headers.clone();
    if !p.own_authorization {
        match &p.src.auth {
            Auth::Basic { username, password } => headers.push((
                "Authorization".into(),
                basic_header(&p.render(username), &p.render(password)),
            )),
            Auth::Digest { .. } => {
                comments.push("Digest auth is not in the export: it answers a challenge".into())
            }
            Auth::AwsSigV4 { .. } => {
                comments.push("AWS Signature V4 is not in the export: it signs each send".into())
            }
            _ => {}
        }
    }
    let content_type = headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        .map(|(_, v)| v.clone())
        .unwrap_or_default();
    let post = match &r.body {
        ResolvedBody::None | ResolvedBody::Invalid(_) => None,
        ResolvedBody::Bytes(b) => Some(json!({
            "mimeType": content_type,
            "text": String::from_utf8_lossy(b),
        })),
        ResolvedBody::File(f) => match std::fs::read(f).map(String::from_utf8) {
            Ok(Ok(text)) => Some(json!({"mimeType": content_type, "text": text})),
            _ => {
                comments.push(format!(
                    "the body file {} is not UTF-8 text or cannot be read",
                    f.display()
                ));
                Some(json!({"mimeType": content_type, "text": ""}))
            }
        },
        ResolvedBody::Multipart(parts) => Some(json!({
            "mimeType": "multipart/form-data",
            "params": parts.iter().map(|part| {
                let mut v = match &part.data {
                    PartData::Text(t) => json!({"name": part.name, "value": t}),
                    PartData::File { filename, .. } => json!({"name": part.name, "fileName": filename}),
                };
                if let Some(ct) = &part.content_type {
                    v["contentType"] = ct.clone().into();
                }
                v
            }).collect::<Vec<_>>(),
        })),
    };
    let mut request = json!({
        "method": r.method,
        "url": r.url,
        "httpVersion": "HTTP/1.1",
        "cookies": [],
        "headers": headers.iter().map(|(n, v)| json!({"name": n, "value": v})).collect::<Vec<_>>(),
        "queryString": query_string(&r.url),
        "headersSize": -1,
        "bodySize": -1,
    });
    if let Some(post) = post {
        request["postData"] = post;
    }
    let mut e = json!({
        "comment": name,
        "startedDateTime": sendito_store::history::now_rfc3339(),
        "time": 0,
        "request": request,
        "response": {
            "status": 0,
            "statusText": "",
            "httpVersion": "",
            "cookies": [],
            "headers": [],
            "content": {"size": 0, "mimeType": ""},
            "redirectURL": "",
            "headersSize": -1,
            "bodySize": -1,
        },
        "cache": {},
        "timings": {"send": 0, "wait": 0, "receive": 0},
    });
    if !comments.is_empty() {
        e["request"]["comment"] = comments.join("; ").into();
    }
    e
}

/// One request of a HAR export: its name, the request with its folders
/// applied, and the scope that renders it.
pub type ExportItem = (String, HttpRequest, Scope);

/// A HAR 1.2 document with one entry per request and no responses. Each
/// entry's `comment` holds the request name.
pub fn export(requests: &[ExportItem], include_secrets: bool, base_dir: &Path) -> String {
    let doc = json!({
        "log": {
            "version": "1.2",
            "creator": {"name": "Sendito", "version": env!("CARGO_PKG_VERSION")},
            "entries": requests
                .iter()
                .map(|(name, req, scope)| entry(name, req, scope, include_secrets, base_dir))
                .collect::<Vec<_>>(),
        }
    });
    serde_json::to_string_pretty(&doc).unwrap_or_default() + "\n"
}

#[cfg(test)]
mod tests {
    use super::*;
    use sendito_core::model::{MultipartPart, PartKind};
    use sendito_core::vars::VarValue;
    use std::collections::BTreeMap;

    const HAR: &str = r#"{"log":{"version":"1.2","entries":[
      {"request":{"method":"get","url":"https://api.test/users?page=2&q=a%20b","headers":[
          {"name":":authority","value":"api.test"},{"name":"Accept","value":"application/json"},
          {"name":"Cookie","value":"sid=1"},{"name":"Host","value":"api.test"}]},
       "response":{"status":200,"statusText":"OK","headers":[{"name":"Content-Type","value":"application/json"}],
          "content":{"mimeType":"application/json","text":"eyJvayI6dHJ1ZX0=","encoding":"base64"}}},
      {"request":{"method":"POST","url":"https://api.test/users","headers":[{"name":"Content-Type","value":"application/json"},{"name":"Content-Length","value":"13"}],
          "postData":{"mimeType":"application/json","text":"{\"name\":\"a\"}"}},
       "response":{"status":0,"content":{}}},
      {"request":{"method":"POST","url":"http://other:8080/form","headers":[],
          "postData":{"mimeType":"application/x-www-form-urlencoded","text":"a=1&b=x%20y","params":[{"name":"a","value":"1"}]}}},
      {"request":{"method":"POST","url":"http://other:8080/up","headers":[],
          "postData":{"mimeType":"multipart/form-data; boundary=x","text":"--x--"}}},
      {"request":{"method":"GET","url":"wss://api.test/ws","headers":[]}},
      {"comment":"named one","request":{"method":"DELETE","url":"https://api.test/users/1","headers":[]}}
    ]}}"#;

    fn http(r: &Request) -> &HttpRequest {
        match r {
            Request::Http(h) => h,
            _ => panic!("not http"),
        }
    }

    #[test]
    fn imports_entries_by_host() {
        let r = import(HAR).unwrap();
        let names: Vec<&str> = r.root.folders.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["https://api.test", "http://other:8080"]);
        let api = &r.root.folders[0];
        let reqs: Vec<&str> = api.requests.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(reqs, ["GET /users", "POST /users", "named one"]);
        let get = http(&api.requests[0].1);
        // The URL keeps the query bytes as recorded.
        assert_eq!(get.url, "https://api.test/users?page=2&q=a%20b");
        assert_eq!(get.method, "GET");
        let names: Vec<&str> = get.headers.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, ["Accept", "Cookie"]);
        // The response becomes an example; a status 0 response does not.
        let ex = &api.examples[&0][0];
        assert_eq!((ex.status, ex.name.as_str()), (200, "200-ok"));
        assert_eq!(ex.body, br#"{"ok":true}"#);
        assert!(!api.examples.contains_key(&1));
        let post = http(&api.requests[1].1);
        assert_eq!(
            post.body,
            Body::Json {
                text: "{\"name\":\"a\"}".into()
            }
        );
        assert_eq!(post.headers.len(), 1);
        let other = &r.root.folders[1];
        assert_eq!(
            http(&other.requests[0].1).body,
            Body::Raw {
                content_type: "application/x-www-form-urlencoded".into(),
                text: "a=1&b=x%20y".into()
            }
        );
        let reasons: Vec<&str> = r.skipped.iter().map(|s| s.reason.as_str()).collect();
        assert_eq!(reasons.len(), 3, "{reasons:?}");
        assert!(reasons[0].starts_with("Cookie headers"));
        assert!(reasons[1].starts_with("the multipart body"));
        assert!(reasons[2].starts_with("not an HTTP request"));
        assert_eq!(r.root.count_requests(), 5);
    }

    #[test]
    fn refuses_a_document_without_entries() {
        assert!(import("{\"log\":{}}").unwrap_err().contains("log.entries"));
        assert!(import("[").is_err());
        assert!(is_har(&serde_json::from_str(HAR).unwrap()));
    }

    #[test]
    fn exports_and_imports_back() {
        let mut m = BTreeMap::new();
        m.insert("token".into(), VarValue::secret("s3cr3t"));
        let scope = Scope::new().with_layer("env", m);
        let reqs: Vec<ExportItem> = vec![
            (
                "create user".to_string(),
                HttpRequest {
                    method: "POST".into(),
                    url: "https://api.test/users".into(),
                    params: vec![KeyValue::new("q", "a b")],
                    headers: vec![KeyValue::new("X-Token", "{{token}}")],
                    body: Body::Json {
                        text: "{\"a\":1}".into(),
                    },
                    ..Default::default()
                },
                scope.clone(),
            ),
            (
                "upload".to_string(),
                HttpRequest {
                    method: "POST".into(),
                    url: "https://api.test/up".into(),
                    auth: Auth::Basic {
                        username: "ana".into(),
                        password: "pw".into(),
                    },
                    body: Body::Multipart {
                        parts: vec![MultipartPart {
                            name: "f".into(),
                            kind: PartKind::File {
                                path: "a.png".into(),
                                filename: None,
                            },
                            content_type: String::new(),
                            enabled: true,
                        }],
                    },
                    ..Default::default()
                },
                scope.clone(),
            ),
        ];
        let text = export(&reqs, false, Path::new("/c"));
        assert!(!text.contains("s3cr3t"), "{text}");
        let doc: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(doc["log"]["version"], "1.2");
        let first = &doc["log"]["entries"][0];
        assert_eq!(first["comment"], "create user");
        assert_eq!(first["request"]["url"], "https://api.test/users?q=a%20b");
        assert_eq!(first["request"]["queryString"][0]["value"], "a%20b");
        assert_eq!(first["request"]["postData"]["mimeType"], "application/json");
        assert_eq!(first["response"]["status"], 0);
        let second = &doc["log"]["entries"][1]["request"];
        assert_eq!(second["postData"]["params"][0]["fileName"], "a.png");
        // No part type in the request: Sendito guesses it when it sends.
        assert!(second["postData"]["params"][0].get("contentType").is_none());
        assert!(
            second["headers"]
                .as_array()
                .unwrap()
                .iter()
                .any(|h| h["value"] == "Basic YW5hOnB3")
        );

        let back = import(&text).unwrap();
        let api = &back.root.folders[0];
        assert_eq!(api.requests[0].0, "create user");
        let h = http(&api.requests[0].1);
        assert_eq!(h.url, "https://api.test/users?q=a%20b");
        assert_eq!(
            h.body,
            Body::Json {
                text: "{\"a\":1}".into()
            }
        );
        assert!(
            h.headers
                .iter()
                .any(|kv| kv.name == "X-Token" && kv.value == "{{token}}")
        );
    }

    #[test]
    fn basic_credentials_with_a_placeholder_stay_a_placeholder() {
        assert_eq!(
            basic_header("{{u}}", "p"),
            "Basic {{sendito_basic_credentials}}"
        );
        assert_eq!(basic_header("ana", "pw"), "Basic YW5hOnB3");
    }
}
