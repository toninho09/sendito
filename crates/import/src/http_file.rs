//! Export a request as an `.http` file, the format the JetBrains HTTP client
//! and the VS Code REST Client open directly, and import such a file.

use std::path::Path;

use sendito_core::model::{Auth, HttpRequest, Request};
use sendito_core::resolve::{HeaderOrigin, ResolvedBody, resolve_http};
use sendito_core::vars::Scope;

/// The boundary used in exported multipart bodies. The short fixed name keeps
/// the output reproducible; when any part mentions it, the name escalates
/// until no part does, so the delimiters cannot occur in the content.
fn multipart_boundary(parts: &[sendito_core::resolve::ResolvedPart]) -> String {
    for n in 0.. {
        let candidate = if n == 0 {
            "sendito".to_string()
        } else {
            format!("sendito-{n}")
        };
        let used = parts.iter().any(|p| {
            p.name.contains(&candidate)
                || p.content_type
                    .as_deref()
                    .is_some_and(|c| c.contains(&candidate))
                || match &p.data {
                    sendito_core::resolve::PartData::Text(t) => t.contains(&candidate),
                    sendito_core::resolve::PartData::File { path, filename } => {
                        path.to_string_lossy().contains(&candidate) || filename.contains(&candidate)
                    }
                }
        });
        if !used {
            return candidate;
        }
    }
    unreachable!("the loop returns on the first free name")
}

/// Keeps a value on one line: a CR or LF in it would start a new header or
/// request line in the file.
fn one_line(s: &str) -> std::borrow::Cow<'_, str> {
    if s.contains(['\r', '\n']) {
        s.replace(['\r', '\n'], " ").into()
    } else {
        s.into()
    }
}

/// Exports a request as one `###` request block.
///
/// With `include_secrets` false, variables whose value is secret stay as
/// `{{placeholders}}` — the same syntax both editors use for their own
/// variables; everything else is substituted.
pub fn export(
    name: &str,
    req: &HttpRequest,
    scope: &Scope,
    include_secrets: bool,
    base_dir: &Path,
) -> String {
    let (template, empty);
    let (src, sc): (&HttpRequest, &Scope) = if include_secrets {
        (req, scope)
    } else {
        template = match sendito_core::resolve::mask_request(&Request::Http(req.clone()), scope) {
            Request::Http(h) => h,
            _ => req.clone(),
        };
        empty = Scope::new();
        (&template, &empty)
    };
    let mut resolved_request = src.clone();
    if matches!(src.auth, Auth::OAuth { .. }) {
        resolved_request.auth = Auth::None;
    }
    let mut r = resolve_http(&resolved_request, sc, base_dir);
    crate::preserve_export_templates(&resolved_request, sc, &mut r);
    if !r
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("authorization"))
    {
        // The forms both editors read: `Digest user password`; VS Code REST
        // Client also reads `AWS key secret region:… service:…`.
        let value = match &src.auth {
            Auth::OAuth { name } => Some(format!(
                "Bearer {}",
                crate::oauth_placeholder(&sc.render(name).text)
            )),
            Auth::Digest { username, password } => Some(format!(
                "Digest {} {}",
                sc.render(username).text,
                sc.render(password).text
            )),
            Auth::AwsSigV4 {
                access_key,
                secret_key,
                session_token,
                region,
                service,
            } => {
                let mut v = format!(
                    "AWS {} {}",
                    sc.render(access_key).text,
                    sc.render(secret_key).text
                );
                if !session_token.is_empty() {
                    v.push_str(&format!(" token:{}", sc.render(session_token).text));
                }
                v.push_str(&format!(
                    " region:{} service:{}",
                    sc.render(region).text,
                    sc.render(service).text
                ));
                Some(v)
            }
            Auth::Jwt { .. } => Some(format!("Bearer {}", crate::JWT_PLACEHOLDER)),
            _ => None,
        };
        if let Some(value) = value {
            r.headers.push(("Authorization".into(), value));
            r.header_origins.push(HeaderOrigin::Auth);
        }
    }
    // Basic credentials cannot be base64-encoded until secret placeholders
    // have been resolved. Never encode the literal placeholder as credentials.
    let basic_placeholder = match &src.auth {
        Auth::Basic { username, password } => {
            sc.render(username).text.contains("{{") || sc.render(password).text.contains("{{")
        }
        _ => false,
    };
    let multipart = matches!(r.body, ResolvedBody::Multipart(_));
    let boundary = match &r.body {
        ResolvedBody::Multipart(parts) => multipart_boundary(parts),
        _ => String::new(),
    };
    let mut out = String::new();
    out.push_str(&format!("### {name}\n"));
    // Directives VS Code REST Client understands; JetBrains ignores them as
    // comments.
    if let Some(t) = r.timeout_ms {
        out.push_str(&format!("# @timeout {t}\n"));
    }
    if !r.follow_redirects {
        out.push_str("# @no-redirect\n");
    }
    if !r.verify_tls {
        out.push_str(
            "# TLS verification is off in Sendito; this file sends with the editor's default.\n",
        );
    }
    if basic_placeholder && r.header_origins.contains(&HeaderOrigin::Auth) {
        out.push_str(
            "# Set sendito_basic_credentials to base64(username:password) before sending.\n",
        );
    }
    if let ResolvedBody::Bytes(b) = &r.body
        && String::from_utf8_lossy(b)
            .lines()
            .any(|l| l.starts_with("###"))
    {
        out.push_str(
            "# The body has a line that starts with ###: editors read it as a new request.\n",
        );
    }
    out.push_str(&format!("{} {}\n", one_line(&r.method), one_line(&r.url)));
    for ((n, v), origin) in r.headers.iter().zip(&r.header_origins) {
        if basic_placeholder
            && *origin == HeaderOrigin::Auth
            && n.eq_ignore_ascii_case("authorization")
        {
            out.push_str("Authorization: Basic {{sendito_basic_credentials}}\n");
            continue;
        }
        // The multipart boundary is ours: resolve leaves the Content-Type to
        // the transport, which an .http file does not have.
        if multipart && n.eq_ignore_ascii_case("content-type") {
            out.push_str(&format!("{n}: multipart/form-data; boundary={boundary}\n"));
        } else {
            out.push_str(&format!("{}: {}\n", one_line(n), one_line(v)));
        }
    }
    if multipart
        && !r
            .headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("content-type"))
    {
        out.push_str(&format!(
            "Content-Type: multipart/form-data; boundary={boundary}\n"
        ));
    }
    match &r.body {
        ResolvedBody::None => {}
        ResolvedBody::Bytes(b) => {
            out.push('\n');
            out.push_str(&String::from_utf8_lossy(b));
        }
        ResolvedBody::File(p) => {
            out.push('\n');
            // Both editors read the body from a file with `< path`.
            out.push_str(&format!("< {}\n", p.display()));
        }
        ResolvedBody::Multipart(parts) => {
            out.push('\n');
            for p in parts {
                out.push_str(&format!("--{boundary}\n"));
                match &p.data {
                    sendito_core::resolve::PartData::Text(t) => {
                        out.push_str(&format!(
                            "Content-Disposition: form-data; name=\"{}\"\n",
                            p.name
                        ));
                        if let Some(ct) = &p.content_type {
                            out.push_str(&format!("Content-Type: {ct}\n"));
                        }
                        out.push('\n');
                        out.push_str(t);
                        out.push('\n');
                    }
                    sendito_core::resolve::PartData::File { path, filename } => {
                        out.push_str(&format!(
                            "Content-Disposition: form-data; name=\"{}\"; filename=\"{filename}\"\n",
                            p.name
                        ));
                        let ct = p.content_type.clone().unwrap_or_else(|| {
                            mime_guess::from_path(filename)
                                .first_raw()
                                .unwrap_or("application/octet-stream")
                                .to_string()
                        });
                        out.push_str(&format!("Content-Type: {ct}\n"));
                        out.push('\n');
                        out.push_str(&format!("< {}\n", path.display()));
                    }
                }
            }
            out.push_str(&format!("--{boundary}--\n"));
        }
    }
    out
}

/// Imports an `.http` file and the `http-client.env.json` next to it (its
/// environments). `text` is the file's content.
pub fn import_path(path: &Path, text: &str) -> Result<crate::ImportResult, String> {
    let mut r = import(text)?;
    if let Some(name) = path.file_stem() {
        r.root.name = name.to_string_lossy().into_owned();
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    let envs = dir.join("http-client.env.json");
    if let Ok(t) = std::fs::read_to_string(&envs) {
        let v: serde_json::Value =
            serde_json::from_str(&t).map_err(|e| format!("{}: {e}", envs.display()))?;
        for (name, vars) in v.as_object().into_iter().flatten() {
            r.environments.push(crate::ImportEnvironment {
                name: name.clone(),
                variables: vars
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| {
                        let v = match v {
                            serde_json::Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        (k.clone(), v)
                    })
                    .collect(),
            });
        }
    }
    if dir.join("http-client.private.env.json").is_file() {
        r.skipped.push(crate::Skipped {
            item: "(environments)".into(),
            reason: "http-client.private.env.json holds secrets and was not imported: \
                     set them as environment secrets"
                .into(),
        });
    }
    Ok(r)
}

/// Imports the requests of an `.http` file: `###` separates them; `@name =
/// value` lines are collection variables; `# @name`, `# @timeout` and
/// `# @no-redirect` set the name and options.
pub fn import(text: &str) -> Result<crate::ImportResult, String> {
    use sendito_core::model::{Body, KeyValue};
    let mut r = crate::ImportResult {
        root: crate::ImportFolder {
            name: "http".into(),
            ..Default::default()
        },
        ..Default::default()
    };
    let lines: Vec<&str> = text.lines().collect();
    let mut blocks: Vec<(String, Vec<&str>)> = vec![(String::new(), Vec::new())];
    for line in &lines {
        if let Some(rest) = line.strip_prefix("###") {
            blocks.push((rest.trim().to_string(), Vec::new()));
        } else if let Some(b) = blocks.last_mut() {
            b.1.push(line);
        }
    }
    for (title, block) in blocks {
        let mut name = title;
        let mut options = sendito_core::model::HttpOptions::default();
        let mut i = 0;
        // Comments, directives and variables before the request line.
        let mut request_line = None;
        while i < block.len() {
            let line = block[i].trim();
            i += 1;
            if line.is_empty() {
                continue;
            }
            let comment = line
                .strip_prefix('#')
                .or_else(|| line.strip_prefix("//"))
                .map(str::trim);
            if let Some(c) = comment {
                if let Some(n) = c.strip_prefix("@name") {
                    name = n.trim().trim_start_matches('=').trim().to_string();
                } else if let Some(t) = c.strip_prefix("@timeout") {
                    options.timeout_ms = t.trim().parse().ok();
                } else if c.starts_with("@no-redirect") {
                    options.follow_redirects = false;
                }
                continue;
            }
            if let Some(var) = line.strip_prefix('@')
                && let Some((k, v)) = var.split_once('=')
            {
                r.variables
                    .push((k.trim().to_string(), v.trim().to_string()));
                continue;
            }
            request_line = Some(line.to_string());
            break;
        }
        let Some(mut line) = request_line else {
            continue;
        };
        // A query on the following lines: `  ?a=1` and `  &b=2`.
        while i < block.len() {
            let next = block[i].trim_start();
            if block[i].starts_with(char::is_whitespace)
                && (next.starts_with('?') || next.starts_with('&'))
            {
                line.push_str(next.trim_end());
                i += 1;
            } else {
                break;
            }
        }
        let mut words: Vec<&str> = line.split_whitespace().collect();
        if words.last().is_some_and(|w| w.starts_with("HTTP/")) {
            words.pop();
        }
        let (method, url) = match words.as_slice() {
            [url] => ("GET".to_string(), url.to_string()),
            [method, url] => (method.to_ascii_uppercase(), url.to_string()),
            _ => {
                r.skipped.push(crate::Skipped {
                    item: if name.is_empty() {
                        line.clone()
                    } else {
                        name.clone()
                    },
                    reason: "the request line is not METHOD URL; skipped".into(),
                });
                continue;
            }
        };
        if name.is_empty() {
            let path = url.split(['?', '#']).next().unwrap_or(&url);
            name = format!(
                "{method} {}",
                path.rsplit('/')
                    .next()
                    .filter(|p| !p.is_empty())
                    .unwrap_or(path)
            );
        }
        let mut headers: Vec<KeyValue> = Vec::new();
        while i < block.len() && !block[i].trim().is_empty() {
            let h = block[i].trim();
            i += 1;
            if h.starts_with('#') || h.starts_with("//") {
                continue;
            }
            if let Some((n, v)) = h.split_once(':') {
                headers.push(KeyValue::new(n.trim(), v.trim()));
            }
        }
        // The body: up to a response handler (`> …`) or the end of the block.
        let mut body_lines: Vec<&str> = Vec::new();
        for l in &block[i..] {
            if l.starts_with("> ") || l.starts_with(">>") || l.starts_with("<> ") {
                r.skipped.push(crate::Skipped {
                    item: name.clone(),
                    reason: "its response handler or output redirect was not imported".into(),
                });
                break;
            }
            body_lines.push(l);
        }
        while body_lines.first().is_some_and(|l| l.trim().is_empty()) {
            body_lines.remove(0);
        }
        while body_lines.last().is_some_and(|l| l.trim().is_empty()) {
            body_lines.pop();
        }
        let body_text = body_lines.join("\n");
        let content_type = headers
            .iter()
            .position(|h| h.name.eq_ignore_ascii_case("content-type"));
        let ct = content_type
            .map(|i| headers[i].value.clone())
            .unwrap_or_default();
        let body = if body_text.is_empty() {
            Body::None
        } else if let Some(file) = body_text.strip_prefix("< ").filter(|f| !f.contains('\n')) {
            r.skipped.push(crate::Skipped {
                item: name.clone(),
                reason: format!(
                    "its body file {} is relative to the .http file; Sendito reads it from the collection",
                    file.trim()
                ),
            });
            Body::File {
                path: file.trim().to_string(),
                content_type: ct.clone(),
            }
        } else if ct.to_ascii_lowercase().starts_with("application/json")
            || ct.is_empty() && body_text.trim_start().starts_with(['{', '['])
        {
            Body::Json { text: body_text }
        } else if ct
            .to_ascii_lowercase()
            .starts_with("application/x-www-form-urlencoded")
        {
            Body::Form {
                fields: body_text
                    .trim()
                    .split('&')
                    .filter(|p| !p.is_empty())
                    .map(|p| {
                        let (k, v) = p.split_once('=').unwrap_or((p, ""));
                        KeyValue::new(form_decode(k), form_decode(v))
                    })
                    .collect(),
            }
        } else {
            Body::Raw {
                content_type: ct.clone(),
                text: body_text,
            }
        };
        // The body carries its Content-Type (and a file body its own).
        if let Some(i) = content_type
            && matches!(
                body,
                Body::Json { .. } | Body::Form { .. } | Body::Raw { .. } | Body::File { .. }
            )
            && !matches!(&body, Body::Json { .. } if !ct.to_ascii_lowercase().starts_with("application/json"))
        {
            headers.remove(i);
        }
        // `Authorization: Basic user password` and the like, as both editors write it.
        let mut auth = Auth::Inherit;
        if let Some(i) = headers
            .iter()
            .position(|h| h.name.eq_ignore_ascii_case("authorization"))
        {
            let words: Vec<&str> = headers[i].value.split_whitespace().collect();
            let found = match words.as_slice() {
                ["Bearer" | "bearer", token] => Some(Auth::Bearer {
                    token: token.to_string(),
                }),
                ["Basic" | "basic", user, pass] => Some(Auth::Basic {
                    username: user.to_string(),
                    password: pass.to_string(),
                }),
                ["Digest" | "digest", user, pass] => Some(Auth::Digest {
                    username: user.to_string(),
                    password: pass.to_string(),
                }),
                _ => None,
            };
            if let Some(a) = found {
                auth = a;
                headers.remove(i);
            }
        }
        r.root.requests.push((
            name,
            Request::Http(HttpRequest {
                method,
                url,
                headers,
                body,
                auth,
                options,
                ..Default::default()
            }),
        ));
    }
    if r.root.requests.is_empty() {
        return Err("no request found in the .http file".into());
    }
    Ok(r)
}

/// Decodes a form field (`+` is a space).
fn form_decode(s: &str) -> String {
    percent_encoding::percent_decode_str(&s.replace('+', " "))
        .decode_utf8_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sendito_core::model::{Auth, Body, KeyValue, MultipartPart, PartKind};
    use sendito_core::vars::{Scope, VarValue};

    fn scope() -> Scope {
        let mut s = Scope::new();
        s.set("test", "base_url", VarValue::plain("http://x"));
        s.set("test", "token", VarValue::secret("s3cr3t"));
        s
    }

    #[test]
    fn get_with_bearer_and_timeout() {
        let mut req = HttpRequest {
            url: "{{base_url}}/users".into(),
            headers: vec![KeyValue::new("Accept", "application/json")],
            auth: Auth::Bearer {
                token: "{{token}}".into(),
            },
            ..Default::default()
        };
        req.options.timeout_ms = Some(2500);
        let out = export("list users", &req, &scope(), false, Path::new("."));
        assert_eq!(
            out,
            "### list users\n\
             # @timeout 2500\n\
             GET http://x/users\n\
             Accept: application/json\n\
             Authorization: Bearer {{token}}\n"
        );
        // With secrets, the value is written out.
        let out = export("list users", &req, &scope(), true, Path::new("."));
        assert!(out.contains("Authorization: Bearer s3cr3t\n"), "{out}");
    }

    #[test]
    fn basic_secret_is_not_encoded_as_a_literal_placeholder() {
        let req = HttpRequest {
            url: "http://x".into(),
            auth: Auth::Basic {
                username: "user".into(),
                password: "{{token}}".into(),
            },
            ..Default::default()
        };
        let out = export("basic", &req, &scope(), false, Path::new("."));
        assert!(out.contains("Authorization: Basic {{sendito_basic_credentials}}"));
        assert!(!out.contains("s3cr3t"));
    }

    #[test]
    fn json_body_and_options() {
        let mut req = HttpRequest {
            method: "POST".into(),
            url: "http://x/users".into(),
            body: Body::Json {
                text: "{\"name\": \"Zé\"}".into(),
            },
            ..Default::default()
        };
        req.options.follow_redirects = false;
        let out = export("create", &req, &scope(), false, Path::new("."));
        assert!(out.starts_with("### create\n# @no-redirect\nPOST http://x/users\n"));
        assert!(out.contains("Content-Type: application/json\n"));
        assert!(out.ends_with("\n{\"name\": \"Zé\"}"));
    }

    #[test]
    fn boundary_avoids_the_parts_content() {
        let req = HttpRequest {
            method: "POST".into(),
            url: "http://x/upload".into(),
            body: Body::Multipart {
                parts: vec![MultipartPart {
                    name: "sendito".into(),
                    enabled: true,
                    kind: PartKind::Text {
                        value: "x--sendito--y".into(),
                    },
                    content_type: String::new(),
                }],
            },
            ..Default::default()
        };
        let out = export("upload", &req, &scope(), false, Path::new("."));
        assert!(!out.contains("boundary=sendito\n"), "{out}");
        assert!(out.contains("boundary=sendito-1\n"), "{out}");
        assert!(out.contains("--sendito-1\n"), "{out}");
        assert!(out.ends_with("--sendito-1--\n"), "{out}");
        // The original content is untouched.
        assert!(out.contains("x--sendito--y"), "{out}");
    }

    #[test]
    fn multipart_writes_the_boundary() {
        let dir = std::env::temp_dir().join(format!("multipart-part-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.json"), b"{}").unwrap();
        let req = HttpRequest {
            method: "POST".into(),
            url: "http://x/upload".into(),
            body: Body::Multipart {
                parts: vec![
                    MultipartPart {
                        name: "note".into(),
                        enabled: true,
                        kind: PartKind::Text {
                            value: "hello".into(),
                        },
                        content_type: "text/plain".into(),
                    },
                    MultipartPart {
                        name: "file".into(),
                        enabled: true,
                        kind: PartKind::File {
                            path: dir.join("a.json").display().to_string(),
                            filename: None,
                        },
                        content_type: String::new(),
                    },
                ],
            },
            ..Default::default()
        };
        let out = export("upload", &req, &scope(), false, Path::new("."));
        assert!(
            out.contains("Content-Type: multipart/form-data; boundary=sendito\n"),
            "{out}"
        );
        assert!(out.contains("--sendito\n"), "{out}");
        assert!(out.contains("name=\"note\""), "{out}");
        assert!(
            out.contains("name=\"file\"; filename=\"a.json\"\nContent-Type: application/json\n"),
            "{out}"
        );
        assert!(out.contains("< "), "{out}");
        assert!(out.ends_with("--sendito--\n"), "{out}");
        assert_eq!(out.matches("Content-Type: multipart").count(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn imports_requests_variables_and_options() {
        let text = "@host = https://api.example.com\n\
                    \n\
                    ### List users\n\
                    # @timeout 1500\n\
                    GET {{host}}/users\n  ?page=1\n  &size=20\n\
                    Accept: application/json\n\
                    Authorization: Bearer {{token}}\n\
                    \n\
                    ###\n\
                    # @name create\n\
                    # @no-redirect\n\
                    POST {{host}}/users HTTP/1.1\n\
                    Content-Type: application/json\n\
                    \n\
                    {\"name\": \"Ana\"}\n\
                    \n\
                    > {% client.test(\"ok\", function() {}); %}\n\
                    \n\
                    ### form\n\
                    POST {{host}}/login\n\
                    Content-Type: application/x-www-form-urlencoded\n\
                    Authorization: Basic ana s3cret\n\
                    \n\
                    user=ana&note=a+b%21\n";
        let r = import(text).unwrap();
        assert_eq!(
            r.variables,
            vec![("host".into(), "https://api.example.com".into())]
        );
        let names: Vec<&str> = r.root.requests.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["List users", "create", "form"]);
        let Request::Http(list) = &r.root.requests[0].1 else {
            panic!()
        };
        assert_eq!(list.url, "{{host}}/users?page=1&size=20");
        assert_eq!(list.options.timeout_ms, Some(1500));
        assert_eq!(
            list.headers,
            vec![KeyValue::new("Accept", "application/json")]
        );
        assert_eq!(
            list.auth,
            Auth::Bearer {
                token: "{{token}}".into()
            }
        );
        let Request::Http(create) = &r.root.requests[1].1 else {
            panic!()
        };
        assert!(!create.options.follow_redirects);
        assert_eq!(
            create.body,
            Body::Json {
                text: "{\"name\": \"Ana\"}".into()
            }
        );
        assert!(create.headers.is_empty());
        assert!(
            r.skipped
                .iter()
                .any(|s| s.reason.contains("response handler"))
        );
        let Request::Http(form) = &r.root.requests[2].1 else {
            panic!()
        };
        assert_eq!(
            form.body,
            Body::Form {
                fields: vec![KeyValue::new("user", "ana"), KeyValue::new("note", "a b!")]
            }
        );
        assert!(matches!(&form.auth, Auth::Basic { username, .. } if username == "ana"));
        assert!(import("# nothing\n").is_err());
    }

    #[test]
    fn export_then_import_gives_the_request_back() {
        let req = HttpRequest {
            method: "PUT".into(),
            url: "https://{{host}}/items/1".into(),
            headers: vec![KeyValue::new("X-Trace", "abc")],
            body: Body::Json {
                text: "{\n  \"a\": 1\n}".into(),
            },
            ..Default::default()
        };
        let mut s = Scope::new();
        s.set("test", "host", VarValue::secret("x.example.com"));
        let out = export("put item", &req, &s, false, Path::new("."));
        let r = import(&out).unwrap();
        assert_eq!(r.root.requests[0].0, "put item");
        assert_eq!(r.root.requests[0].1, Request::Http(req));
    }

    #[test]
    fn environments_come_from_the_env_file_next_to_it() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("api.http");
        std::fs::write(&file, "GET {{host}}/ping\n").unwrap();
        std::fs::write(
            d.path().join("http-client.env.json"),
            "{\"dev\": {\"host\": \"http://localhost\", \"port\": 8080}}",
        )
        .unwrap();
        std::fs::write(d.path().join("http-client.private.env.json"), "{}").unwrap();
        let r = import_path(&file, "GET {{host}}/ping\n").unwrap();
        assert_eq!(r.root.name, "api");
        assert_eq!(r.environments[0].name, "dev");
        assert_eq!(
            r.environments[0].variables,
            vec![
                ("host".to_string(), "http://localhost".to_string()),
                ("port".to_string(), "8080".to_string())
            ]
        );
        assert!(r.skipped.iter().any(|s| s.reason.contains("private")));
    }
}
