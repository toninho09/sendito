//! Import Bruno collections stored as `.bru` files.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::{ImportEnvironment, ImportFolder, ImportResult, Skipped};
use sendito_core::model::{
    ApiKeyLocation, Auth, Body, FolderSettings, HttpRequest, KeyValue, MultipartPart, PartKind,
    Request,
};

#[derive(Default)]
struct Blocks(BTreeMap<String, String>);

impl Blocks {
    fn get(&self, name: &str) -> &str {
        self.0.get(name).map(String::as_str).unwrap_or("")
    }
}

/// Top-level blocks end with a closing delimiter at column zero. JSON/script
/// braces within a block are indented by Bruno and remain part of the text.
fn blocks(input: &str) -> Result<Blocks, String> {
    let mut out = Blocks::default();
    let mut current: Option<(String, char, Vec<String>)> = None;
    for (i, line) in input.lines().enumerate() {
        let line = line.trim_end_matches('\r');
        if let Some((name, close, lines)) = &mut current {
            if line == close.to_string() {
                let content = lines.join("\n");
                if out.0.insert(name.clone(), content).is_some() {
                    return Err(format!("duplicate block at line {}", i + 1));
                }
                current = None;
            } else {
                lines.push(line.strip_prefix("  ").unwrap_or(line).to_string());
            }
        } else {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (name, close) = if let Some(n) = line.strip_suffix('{') {
                (n.trim(), '}')
            } else if let Some(n) = line.strip_suffix('[') {
                (n.trim(), ']')
            } else {
                return Err(format!("expected a block at line {}", i + 1));
            };
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_'))
            {
                return Err(format!("invalid block name at line {}", i + 1));
            }
            current = Some((name.to_string(), close, Vec::new()));
        }
    }
    if current.is_some() {
        return Err("unclosed block".into());
    }
    Ok(out)
}

fn rows(input: &str) -> Vec<KeyValue> {
    input
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (key, value) = line.split_once(':')?;
            let key = key.trim();
            let (enabled, name) = key
                .strip_prefix('~')
                .map_or((true, key), |k| (false, k.trim()));
            Some(KeyValue {
                name: name.into(),
                value: value.trim().into(),
                enabled,
            })
        })
        .collect()
}

fn field(input: &str, name: &str) -> String {
    rows(input)
        .into_iter()
        .find(|r| r.name == name && r.enabled)
        .map(|r| r.value)
        .unwrap_or_default()
}

fn note(skipped: &mut Vec<Skipped>, item: &str, reason: impl Into<String>) {
    skipped.push(Skipped {
        item: item.into(),
        reason: reason.into(),
    });
}

fn auth(b: &Blocks, choice: &str, item: &str, skipped: &mut Vec<Skipped>) -> Auth {
    match choice {
        "" | "inherit" => Auth::Inherit,
        "none" => Auth::None,
        "basic" => Auth::Basic {
            username: field(b.get("auth:basic"), "username"),
            password: field(b.get("auth:basic"), "password"),
        },
        "bearer" => Auth::Bearer {
            token: field(b.get("auth:bearer"), "token"),
        },
        "apikey" => {
            let a = b.get("auth:apikey");
            let placement = field(a, "placement");
            if !matches!(placement.as_str(), "" | "header" | "queryparams" | "query") {
                note(
                    skipped,
                    item,
                    "API key placement is not supported; imported as header",
                );
            }
            Auth::ApiKey {
                key: field(a, "key"),
                value: field(a, "value"),
                location: if matches!(placement.as_str(), "queryparams" | "query") {
                    ApiKeyLocation::Query
                } else {
                    ApiKeyLocation::Header
                },
            }
        }
        other => {
            note(
                skipped,
                item,
                format!("authentication type {other:?} is not supported"),
            );
            Auth::None
        }
    }
}

fn file_part(value: &str) -> Option<String> {
    value
        .strip_prefix("@file(")
        .and_then(|v| v.strip_suffix(')'))
        .map(|v| v.trim().trim_matches('"').trim_matches('\'').to_string())
}

fn request(b: &Blocks, item: &str, skipped: &mut Vec<Skipped>) -> Option<HttpRequest> {
    let methods = [
        "get", "post", "put", "patch", "delete", "head", "options", "trace", "connect",
    ];
    let present: Vec<_> = methods
        .into_iter()
        .filter(|m| b.0.contains_key(*m))
        .collect();
    if present.len() != 1 {
        note(
            skipped,
            item,
            "not an HTTP request with one supported method; skipped",
        );
        return None;
    }
    if !matches!(field(b.get("meta"), "type").as_str(), "" | "http") {
        note(skipped, item, "non-HTTP request was not imported");
        return None;
    }
    let method = present[0];
    let details = b.get(method);
    let body_kind = field(details, "body");
    let body = match body_kind.as_str() {
        "none" => Body::None,
        "" if b.get("body").is_empty() => Body::None,
        "json" => Body::Json {
            text: b.get("body:json").to_string(),
        },
        "graphql" => Body::GraphQL {
            query: if b.get("body:graphql:query").is_empty() {
                b.get("body:graphql").to_string()
            } else {
                b.get("body:graphql:query").to_string()
            },
            variables: b.get("body:graphql:variables").to_string(),
            operation_name: String::new(),
        },
        "text" | "xml" | "sparql" => Body::Raw {
            text: b.get(&format!("body:{body_kind}")).to_string(),
            content_type: match body_kind.as_str() {
                "xml" => "application/xml",
                "sparql" => "application/sparql-query",
                _ => "text/plain",
            }
            .into(),
        },
        "formUrlEncoded" | "form-urlencoded" => Body::Form {
            fields: rows(b.get("body:form-urlencoded")),
        },
        "multipartForm" | "multipart-form" => {
            let mut parts = Vec::new();
            for row in rows(b.get("body:multipart-form")) {
                let kind = if let Some(path) = file_part(&row.value) {
                    note(
                        skipped,
                        item,
                        "multipart file path may need to be updated relative to the Sendito collection",
                    );
                    PartKind::File {
                        path,
                        filename: None,
                    }
                } else {
                    PartKind::Text { value: row.value }
                };
                parts.push(MultipartPart {
                    name: row.name,
                    enabled: row.enabled,
                    content_type: String::new(),
                    kind,
                });
            }
            Body::Multipart { parts }
        }
        "" => Body::Raw {
            text: b.get("body").into(),
            content_type: String::new(),
        },
        other => {
            note(
                skipped,
                item,
                format!("body type {other:?} is not supported"),
            );
            Body::None
        }
    };
    let params = if b.0.contains_key("params:query") {
        rows(b.get("params:query"))
    } else {
        rows(b.get("query"))
    };
    if !b.get("params:path").is_empty() {
        note(
            skipped,
            item,
            "path parameters were not imported; define them in the URL or environment",
        );
    }
    let used: HashSet<&str> = [
        "meta",
        method,
        "headers",
        "params:query",
        "query",
        "params:path",
        "body",
        "body:json",
        "body:graphql",
        "body:graphql:query",
        "body:graphql:variables",
        "body:text",
        "body:xml",
        "body:sparql",
        "body:form-urlencoded",
        "body:multipart-form",
        "auth:basic",
        "auth:bearer",
        "auth:apikey",
    ]
    .into_iter()
    .collect();
    for key in b.0.keys().filter(|k| !used.contains(k.as_str())) {
        note(skipped, item, format!("block {key:?} was not imported"));
    }
    Some(HttpRequest {
        method: method.to_ascii_uppercase(),
        url: field(details, "url"),
        headers: rows(b.get("headers")),
        params,
        body,
        auth: auth(b, &field(details, "auth"), item, skipped),
        ..Default::default()
    })
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_file())
}

fn read_blocks(path: &Path) -> Result<Blocks, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    blocks(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The folder settings a `folder.bru` or `collection.bru` gives: its auth,
/// its headers and its pre-request variables.
fn settings(b: &Blocks, item: &str, skipped: &mut Vec<Skipped>) -> FolderSettings {
    FolderSettings {
        auth: auth(b, &field(b.get("auth"), "mode"), item, skipped),
        headers: rows(b.get("headers")),
        vars: rows(b.get("vars:pre-request")),
        ..Default::default()
    }
}

/// The blocks [`settings`] converts.
const SETTINGS_BLOCKS: &[&str] = &[
    "meta",
    "auth",
    "auth:basic",
    "auth:bearer",
    "auth:apikey",
    "headers",
    "vars:pre-request",
];

fn directory(
    path: &Path,
    name: &str,
    skipped: &mut Vec<Skipped>,
    depth: usize,
) -> Result<ImportFolder, String> {
    if depth > 64 {
        return Err("Bruno collection nesting exceeds 64 levels".into());
    }
    let mut folder = ImportFolder {
        name: name.into(),
        ..Default::default()
    };
    let config = path.join("folder.bru");
    if is_regular_file(&config) {
        let b = read_blocks(&config)?;
        folder.settings = settings(&b, name, skipped);
        for k in
            b.0.keys()
                .filter(|k| !SETTINGS_BLOCKS.contains(&k.as_str()))
        {
            note(
                skipped,
                name,
                format!("folder block {k:?} was not imported"),
            );
        }
    }
    let mut entries: Vec<_> = std::fs::read_dir(path)
        .map_err(|e| format!("{}: {e}", path.display()))?
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.starts_with('.') {
            continue;
        }
        if ty.is_dir() {
            if (depth == 0 && file_name == "environments") || file_name == "node_modules" {
                continue;
            }
            folder
                .folders
                .push(directory(&entry.path(), &file_name, skipped, depth + 1)?);
        } else if ty.is_file()
            && file_name.ends_with(".bru")
            && file_name != "folder.bru"
            && file_name != "collection.bru"
        {
            let b = read_blocks(&entry.path())?;
            let display = field(b.get("meta"), "name");
            let display = if display.is_empty() {
                file_name.trim_end_matches(".bru").to_string()
            } else {
                display
            };
            let item = format!("{name} / {display}");
            if let Some(r) = request(&b, &item, skipped) {
                folder.requests.push((display, Request::Http(r)));
            }
        }
    }
    Ok(folder)
}

/// Load a Bruno collection directory. Does not follow symlinked entries.
pub fn import(path: &Path) -> Result<ImportResult, String> {
    if !path.is_dir() {
        return Err("Bruno import needs a collection directory".into());
    }
    let config = path.join("bruno.json");
    if !is_regular_file(&config) {
        return Err("Bruno collection has no regular bruno.json".into());
    }
    let meta: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&config).map_err(|e| format!("{}: {e}", config.display()))?,
    )
    .map_err(|e| format!("invalid bruno.json: {e}"))?;
    let name = meta
        .get("name")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or("Bruno");
    let mut result = ImportResult::default();
    let mut root_settings = FolderSettings::default();
    let collection = path.join("collection.bru");
    if is_regular_file(&collection) {
        let b = read_blocks(&collection)?;
        let secrets: HashSet<_> = b
            .get("vars:secret")
            .lines()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect();
        for v in rows(b.get("vars")) {
            if secrets.contains(v.name.as_str()) {
                note(
                    &mut result.skipped,
                    name,
                    "secret variable was not imported; configure it separately",
                );
            } else if v.enabled {
                result.variables.push((v.name, v.value));
            } else {
                note(
                    &mut result.skipped,
                    name,
                    "disabled collection variable was not imported",
                );
            }
        }
        root_settings = settings(&b, name, &mut result.skipped);
        for k in b.0.keys().filter(|k| {
            !SETTINGS_BLOCKS.contains(&k.as_str()) && !matches!(k.as_str(), "vars" | "vars:secret")
        }) {
            note(
                &mut result.skipped,
                name,
                format!("collection block {k:?} was not imported"),
            );
        }
    }
    result.root = directory(path, name, &mut result.skipped, 0)?;
    result.root.settings = root_settings;
    let envdir = path.join("environments");
    if std::fs::symlink_metadata(&envdir).is_ok_and(|m| m.file_type().is_dir()) {
        let mut entries: Vec<_> = std::fs::read_dir(&envdir)
            .map_err(|e| e.to_string())?
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?;
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            if !entry.file_type().map_err(|e| e.to_string())?.is_file()
                || entry.path().extension().is_none_or(|x| x != "bru")
            {
                continue;
            }
            let b = read_blocks(&entry.path())?;
            let name = entry
                .path()
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "environment".into());
            let secrets: HashSet<_> = b
                .get("vars:secret")
                .lines()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .collect();
            let mut variables = Vec::new();
            for v in rows(b.get("vars")) {
                if secrets.contains(v.name.as_str()) {
                    note(
                        &mut result.skipped,
                        &name,
                        "secret variable was not imported; configure it separately",
                    );
                } else if v.enabled {
                    variables.push((v.name, v.value));
                } else {
                    note(
                        &mut result.skipped,
                        &name,
                        format!("disabled variable {:?} was not imported", v.name),
                    );
                }
            }
            for secret in secrets {
                if !rows(b.get("vars")).iter().any(|v| v.name == secret) {
                    note(
                        &mut result.skipped,
                        &name,
                        "secret variable was not imported; configure it separately",
                    );
                }
            }
            for k in
                b.0.keys()
                    .filter(|k| !matches!(k.as_str(), "vars" | "vars:secret"))
            {
                note(
                    &mut result.skipped,
                    &name,
                    format!("environment block {k:?} was not imported"),
                );
            }
            result
                .environments
                .push(ImportEnvironment { name, variables });
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_body_and_disabled_rows() {
        let b = blocks("meta {\n  name: Create\n  type: http\n}\npost {\n  url: {{base}}/users\n  body: json\n  auth: bearer\n}\nheaders {\n  ~X-Off: value\n  X-On: yes\n}\nauth:bearer {\n  token: {{token}}\n}\nbody:json {\n  {\n    \"x\": 1\n  }\n}\nscript:pre-request {\n  foo();\n}\n").unwrap();
        let mut skipped = Vec::new();
        let req = request(&b, "Create", &mut skipped).unwrap();
        assert_eq!(req.url, "{{base}}/users");
        assert!(!req.headers[0].enabled);
        assert_eq!(
            req.body,
            Body::Json {
                text: "{\n  \"x\": 1\n}".into()
            }
        );
        assert_eq!(
            req.auth,
            Auth::Bearer {
                token: "{{token}}".into()
            }
        );
        assert!(
            skipped
                .iter()
                .any(|s| s.reason.contains("script:pre-request"))
        );
    }

    #[test]
    fn graphql_body_blocks() {
        let b = blocks(
            "meta {
  name: Me
  type: http
}
post {
  url: {{base}}/graphql
  body: graphql
}
body:graphql:query {
  query Me { me { id } }
}
body:graphql:variables {\n  {\"limit\": 2}\n}\n",
        )
        .unwrap();
        let mut skipped = Vec::new();
        let req = request(&b, "Me", &mut skipped).unwrap();
        assert_eq!(
            req.body,
            Body::GraphQL {
                query: "query Me { me { id } }".into(),
                variables: "{\"limit\": 2}".into(),
                operation_name: String::new(),
            }
        );
        assert!(skipped.is_empty(), "{skipped:?}");
        // Without blocks of its own, the `body:graphql` block is the query.
        let b = blocks(
            "meta {
  name: Me
  type: http
}
post {
  url: x
  body: graphql
}
body:graphql {
  { hero { name } }
}
",
        )
        .unwrap();
        let req = request(&b, "Me", &mut Vec::new()).unwrap();
        assert!(matches!(req.body, Body::GraphQL { query, .. } if query == "{ hero { name } }"));
    }

    #[test]
    fn imports_tree_and_skips_secret_environment_values() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("bruno.json"), "{\"name\":\"Example\"}").unwrap();
        std::fs::create_dir(d.path().join("Users")).unwrap();
        std::fs::write(
            d.path().join("Users/get.bru"),
            "meta {\n  name: list\n  type: http\n}\nget {\n  url: {{base}}/users\n}\n",
        )
        .unwrap();
        std::fs::create_dir(d.path().join("environments")).unwrap();
        std::fs::write(d.path().join("environments/dev.bru"), "vars {\n  base: http://localhost\n  token: secret-value\n}\nvars:secret [\n  token\n]\n").unwrap();
        let r = import(d.path()).unwrap();
        assert_eq!(r.root.count_requests(), 1);
        assert_eq!(
            r.environments[0].variables,
            vec![("base".into(), "http://localhost".into())]
        );
        assert!(!format!("{r:?}").contains("secret-value"));
        assert!(r.skipped.iter().any(|s| s.reason.contains("secret")));
    }

    #[test]
    fn folder_and_collection_files_become_folder_settings() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("bruno.json"), "{\"name\":\"Example\"}").unwrap();
        std::fs::write(
            d.path().join("collection.bru"),
            "auth {\n  mode: bearer\n}\nauth:bearer {\n  token: {{token}}\n}\nheaders {\n  X-App: demo\n}\n",
        )
        .unwrap();
        std::fs::create_dir(d.path().join("Users")).unwrap();
        std::fs::write(
            d.path().join("Users/folder.bru"),
            "auth {\n  mode: none\n}\nvars:pre-request {\n  page: 1\n}\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("Users/get.bru"),
            "meta {\n  name: list\n}\nget {\n  url: {{base}}/users\n}\n",
        )
        .unwrap();
        let r = import(d.path()).unwrap();
        assert_eq!(
            r.root.settings.auth,
            Auth::Bearer {
                token: "{{token}}".into()
            }
        );
        assert_eq!(
            r.root.settings.headers,
            vec![KeyValue::new("X-App", "demo")]
        );
        let users = &r.root.folders[0];
        assert_eq!(users.settings.auth, Auth::None);
        assert_eq!(users.settings.vars, vec![KeyValue::new("page", "1")]);
        assert_eq!(users.requests[0].1.auth(), &Auth::Inherit);
        assert!(r.skipped.is_empty(), "{:?}", r.skipped);
    }
}
