//! Export a request as an `.http` file, the format the JetBrains HTTP client
//! and the VS Code REST Client open directly.

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
    if let Auth::OAuth { name } = &src.auth
        && !r
            .headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("authorization"))
    {
        r.headers.push((
            "Authorization".into(),
            format!("Bearer {}", crate::oauth_placeholder(&sc.render(name).text)),
        ));
        r.header_origins.push(HeaderOrigin::Auth);
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
    out.push_str(&format!("{} {}\n", r.method, r.url));
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
            out.push_str(&format!("{n}: {v}\n"));
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
}
