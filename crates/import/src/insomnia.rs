//! Import Insomnia JSON exports (v4/v5) into the common import tree.

use std::collections::{BTreeMap, HashSet};

use crate::{ImportEnvironment, ImportFolder, ImportResult, Skipped};
use sendito_core::model::{
    ApiKeyLocation, Auth, Body, HttpRequest, KeyValue, MultipartPart, PartKind, Request,
};
use serde_json::Value;

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}
fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}
fn skip(out: &mut Vec<Skipped>, item: &str, reason: impl Into<String>) {
    out.push(Skipped {
        item: item.into(),
        reason: reason.into(),
    });
}

/// Translate Insomnia's `{{ _.name }}` references when they can be expressed
/// as ordinary Sendito variable names. Leave other expressions untouched and
/// report them rather than guessing.
fn template(s: &str, item: &str, skipped: &mut Vec<Skipped>) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let tail = &rest[i + 2..];
        let Some(end) = tail.find("}}") else {
            out.push_str(&rest[i..]);
            return out;
        };
        let expr = tail[..end].trim();
        if let Some(name) = expr.strip_prefix("_.") {
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            {
                out.push_str(&format!("{{{{{name}}}}}"));
            } else {
                out.push_str(&rest[i..i + end + 4]);
                skip(
                    skipped,
                    item,
                    "an Insomnia template expression is not supported",
                );
            }
        } else {
            out.push_str(&rest[i..i + end + 4]);
            if expr.is_empty()
                || !expr
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '$'))
            {
                skip(
                    skipped,
                    item,
                    "an Insomnia template expression is not supported",
                );
            }
        }
        rest = &tail[end + 2..];
    }
    out.push_str(rest);
    if s.contains("{%") {
        skip(skipped, item, "an Insomnia template tag is not supported");
    }
    out
}

fn rows(v: Option<&Value>, item: &str, skipped: &mut Vec<Skipped>) -> Vec<KeyValue> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|row| KeyValue {
            name: template(text(row, "name"), item, skipped),
            value: template(text(row, "value"), item, skipped),
            enabled: !flag(row, "disabled"),
        })
        .collect()
}

fn auth(v: Option<&Value>, inherited: &Auth, item: &str, skipped: &mut Vec<Skipped>) -> Auth {
    let Some(v) = v.filter(|v| v.is_object()) else {
        return inherited.clone();
    };
    if flag(v, "disabled") {
        return Auth::None;
    }
    match text(v, "type") {
        "" | "inherit" => inherited.clone(),
        "none" => Auth::None,
        "basic" => Auth::Basic {
            username: template(text(v, "username"), item, skipped),
            password: template(text(v, "password"), item, skipped),
        },
        "bearer" => Auth::Bearer {
            token: template(text(v, "token"), item, skipped),
        },
        "apikey" => Auth::ApiKey {
            key: template(text(v, "key"), item, skipped),
            value: template(text(v, "value"), item, skipped),
            location: if text(v, "addTo") == "query" {
                ApiKeyLocation::Query
            } else {
                ApiKeyLocation::Header
            },
        },
        other => {
            skip(
                skipped,
                item,
                format!("authentication type {other:?} is not supported"),
            );
            Auth::None
        }
    }
}

fn body(v: Option<&Value>, item: &str, skipped: &mut Vec<Skipped>) -> Body {
    let Some(v) = v.filter(|v| v.is_object()) else {
        return Body::None;
    };
    let mime = text(v, "mimeType");
    let value = template(text(v, "text"), item, skipped);
    match mime {
        "application/json" => Body::Json { text: value },
        "application/x-www-form-urlencoded" => Body::Form {
            fields: rows(v.get("params"), item, skipped),
        },
        "multipart/form-data" => {
            let mut parts = Vec::new();
            for p in v
                .get("params")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let filename = text(p, "fileName");
                let kind = if text(p, "type") == "file" || !filename.is_empty() {
                    if filename.is_empty() {
                        skip(skipped, item, "multipart part has no file selected");
                    } else {
                        skip(
                            skipped,
                            item,
                            "file path may need to be updated relative to the Sendito collection",
                        );
                    }
                    PartKind::File {
                        path: template(filename, item, skipped),
                        filename: None,
                    }
                } else {
                    PartKind::Text {
                        value: template(text(p, "value"), item, skipped),
                    }
                };
                parts.push(MultipartPart {
                    name: template(text(p, "name"), item, skipped),
                    kind,
                    content_type: text(p, "contentType").into(),
                    enabled: !flag(p, "disabled"),
                });
            }
            Body::Multipart { parts }
        }
        "" if value.is_empty() && text(v, "fileName").is_empty() => Body::None,
        _ if !text(v, "fileName").is_empty() => {
            skip(
                skipped,
                item,
                "file path may need to be updated relative to the Sendito collection",
            );
            Body::File {
                path: template(text(v, "fileName"), item, skipped),
                content_type: mime.into(),
            }
        }
        _ => Body::Raw {
            text: value,
            content_type: mime.into(),
        },
    }
}

fn walk(
    parent: &str,
    name: &str,
    inherited: &Auth,
    children: &BTreeMap<String, Vec<&Value>>,
    seen: &mut HashSet<String>,
    skipped: &mut Vec<Skipped>,
) -> ImportFolder {
    let mut folder = ImportFolder {
        name: name.into(),
        ..Default::default()
    };
    for node in children.get(parent).into_iter().flatten() {
        let id = text(node, "_id");
        if id.is_empty() || !seen.insert(id.to_string()) {
            skip(
                skipped,
                name,
                "duplicate or cyclic resource was not imported",
            );
            continue;
        }
        let label = text(node, "name");
        let label = if label.is_empty() { "untitled" } else { label };
        let item = format!("{name} / {label}");
        match text(node, "_type") {
            "request_group" => {
                let group_auth = auth(node.get("authentication"), inherited, &item, skipped);
                folder
                    .folders
                    .push(walk(id, label, &group_auth, children, seen, skipped));
            }
            "request" => {
                let a = auth(node.get("authentication"), inherited, &item, skipped);
                let r = HttpRequest {
                    method: if text(node, "method").is_empty() {
                        "GET".into()
                    } else {
                        text(node, "method").to_ascii_uppercase()
                    },
                    url: template(text(node, "url"), &item, skipped),
                    headers: rows(node.get("headers"), &item, skipped),
                    params: rows(node.get("parameters"), &item, skipped),
                    body: body(node.get("body"), &item, skipped),
                    auth: a,
                    ..Default::default()
                };
                if !text(node, "description").is_empty() {
                    skip(skipped, &item, "description was not imported");
                }
                for key in [
                    "scripts",
                    "preRequestScript",
                    "afterResponseScript",
                    "settingSendCookies",
                    "settingStoreCookies",
                    "settingFollowRedirects",
                ] {
                    if node.get(key).is_some_and(|v| !v.is_null() && v != "") {
                        skip(skipped, &item, format!("{key} was not imported"));
                    }
                }
                folder.requests.push((label.into(), Request::Http(r)));
            }
            _ => {}
        }
    }
    folder
}

fn flatten(
    value: &Value,
    prefix: &str,
    vars: &mut Vec<(String, String)>,
    item: &str,
    skipped: &mut Vec<Skipped>,
) {
    if let Some(map) = value.as_object() {
        for (k, v) in map {
            let name = if prefix.is_empty() {
                k.to_string()
            } else {
                format!("{prefix}.{k}")
            };
            if k.is_empty()
                || !k
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
            {
                skip(skipped, item, "environment variable name is not supported");
                continue;
            }
            flatten(v, &name, vars, item, skipped);
        }
    } else if let Some(s) = value.as_str() {
        vars.push((prefix.into(), template(s, item, skipped)));
    } else if value.is_number() || value.is_boolean() {
        vars.push((prefix.into(), value.to_string()));
    } else {
        skip(
            skipped,
            item,
            format!("environment variable {prefix:?} has an unsupported value"),
        );
    }
}

pub fn import(json: &str) -> Result<ImportResult, String> {
    let data: Value = serde_json::from_str(json).map_err(|e| format!("not valid JSON: {e}"))?;
    let version = data
        .get("__export_format")
        .and_then(Value::as_u64)
        .ok_or("not an Insomnia export")?;
    if !matches!(version, 4 | 5) {
        return Err(format!("unsupported Insomnia export version {version}"));
    }
    let resources = data
        .get("resources")
        .and_then(Value::as_array)
        .ok_or("Insomnia export has no resources array")?;
    let mut children: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for node in resources {
        if matches!(text(node, "_type"), "request" | "request_group") {
            children
                .entry(text(node, "parentId").into())
                .or_default()
                .push(node);
        }
    }
    let workspaces: Vec<_> = resources
        .iter()
        .filter(|v| text(v, "_type") == "workspace")
        .collect();
    if workspaces.is_empty() {
        return Err("Insomnia export has no workspace".into());
    }
    let mut workspace_ids = HashSet::new();
    if workspaces
        .iter()
        .any(|w| text(w, "_id").is_empty() || !workspace_ids.insert(text(w, "_id")))
    {
        return Err("Insomnia export contains a missing or duplicate workspace ID".into());
    }
    let mut result = ImportResult::default();
    let mut seen = HashSet::new();
    if workspaces.len() == 1 {
        let ws = workspaces[0];
        let name = if text(ws, "name").is_empty() {
            "Insomnia"
        } else {
            text(ws, "name")
        };
        let inherited = auth(
            ws.get("authentication"),
            &Auth::None,
            name,
            &mut result.skipped,
        );
        result.root = walk(
            text(ws, "_id"),
            name,
            &inherited,
            &children,
            &mut seen,
            &mut result.skipped,
        );
    } else {
        result.root.name = "Insomnia".into();
        for ws in &workspaces {
            let name = if text(ws, "name").is_empty() {
                "Workspace"
            } else {
                text(ws, "name")
            };
            let inherited = auth(
                ws.get("authentication"),
                &Auth::None,
                name,
                &mut result.skipped,
            );
            result.root.folders.push(walk(
                text(ws, "_id"),
                name,
                &inherited,
                &children,
                &mut seen,
                &mut result.skipped,
            ));
        }
    }
    for node in resources {
        let kind = text(node, "_type");
        if kind == "environment" {
            let name = text(node, "name");
            if flag(node, "isPrivate") {
                skip(
                    &mut result.skipped,
                    name,
                    "private environment was not imported; configure secrets separately",
                );
                continue;
            }
            let mut vars = Vec::new();
            if let Some(data) = node.get("data") {
                flatten(data, "", &mut vars, name, &mut result.skipped);
            }
            let parent = text(node, "parentId");
            if workspaces.iter().any(|w| text(w, "_id") == parent) && workspaces.len() == 1 {
                result.variables.extend(vars);
            } else {
                result.environments.push(ImportEnvironment {
                    name: if name.is_empty() {
                        "environment".into()
                    } else {
                        name.into()
                    },
                    variables: vars,
                });
            }
        } else if matches!(kind, "request" | "request_group") && !seen.contains(text(node, "_id")) {
            skip(
                &mut result.skipped,
                text(node, "name"),
                "orphaned request or folder was not imported",
            );
        } else if !matches!(
            kind,
            "workspace" | "request" | "request_group" | "environment"
        ) {
            skip(
                &mut result.skipped,
                text(node, "name"),
                format!("Insomnia {kind:?} resource was not imported"),
            );
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collection_and_environments() {
        let json = r#"{"__export_format":4,"resources":[
          {"_type":"workspace","_id":"w","name":"API"},
          {"_type":"environment","_id":"base","parentId":"w","name":"Base","data":{"base":"http://example.test","nested":{"value":3}}},
          {"_type":"environment","_id":"dev","parentId":"base","name":"dev","data":{"base":"http://localhost"}},
          {"_type":"environment","_id":"private","parentId":"base","name":"private","isPrivate":true,"data":{"token":"do-not-import"}},
          {"_type":"request_group","_id":"f","parentId":"w","name":"Users"},
          {"_type":"request","_id":"r","parentId":"f","name":"Create","method":"POST","url":"{{ _.base }}/users","headers":[{"name":"x-on","value":"yes"},{"name":"x-off","value":"no","disabled":true}],"authentication":{"type":"bearer","token":"{{ _.token }}"},"body":{"mimeType":"application/json","text":"{}"}}
        ]}"#;
        let result = import(json).unwrap();
        assert_eq!(result.root.count_requests(), 1);
        assert_eq!(result.environments.len(), 1);
        assert!(
            result
                .variables
                .contains(&("nested.value".into(), "3".into()))
        );
        let Request::Http(req) = &result.root.folders[0].requests[0].1 else {
            panic!("not http")
        };
        assert_eq!(req.url, "{{base}}/users");
        assert!(!req.headers[1].enabled);
        assert_eq!(
            req.auth,
            Auth::Bearer {
                token: "{{token}}".into()
            }
        );
        assert!(
            result
                .skipped
                .iter()
                .any(|s| s.reason.contains("private environment"))
        );
        assert!(!format!("{result:?}").contains("do-not-import"));
    }

    #[test]
    fn rejects_other_json_and_reports_orphans() {
        assert!(import("{}").is_err());
        let r = import(r#"{"__export_format":5,"resources":[{"_type":"workspace","_id":"w"},{"_type":"request","_id":"r","parentId":"missing"}]}"#).unwrap();
        assert_eq!(r.root.count_requests(), 0);
        assert!(r.skipped.iter().any(|s| s.reason.contains("orphaned")));
    }
}
