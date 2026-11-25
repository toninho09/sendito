//! Import an OpenAPI 3.0 / 3.1 or Swagger 2.0 document (JSON or YAML, read
//! into a `serde_json::Value` first): one request per operation, one folder
//! per first tag, the servers as `{{baseUrl}}`.

use std::collections::{BTreeSet, HashSet};

use sendito_core::json_schema;
use sendito_core::model::*;
use sendito_store::examples::Example;
use serde_json::Value;

use crate::{ImportEnvironment, ImportFolder, ImportResult, Skipped};

const METHODS: &[&str] = &[
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];

/// Whether `doc` is an OpenAPI 3.x or Swagger 2.0 document.
pub fn is_openapi(doc: &Value) -> bool {
    doc.get("openapi").is_some() || doc.get("swagger").is_some()
}

/// Whether `doc` is Swagger 2.0 (else OpenAPI 3.x).
pub fn is_swagger(doc: &Value) -> bool {
    doc.get("swagger").is_some()
}

struct Ctx<'a> {
    doc: &'a Value,
    swagger: bool,
    skipped: Vec<Skipped>,
    /// Path parameters, as collection variables (name, sample value).
    path_vars: Vec<(String, String)>,
    /// Variables an auth uses, reported once.
    auth_vars: BTreeSet<String>,
    reported: HashSet<String>,
}

impl Ctx<'_> {
    fn skip(&mut self, item: &str, reason: impl Into<String>) {
        let reason = reason.into();
        if self.reported.insert(format!("{item}\u{0}{reason}")) {
            self.skipped.push(Skipped {
                item: item.to_string(),
                reason,
            });
        }
    }

    /// Follows a local `$ref` (`#/components/...`); an external one is
    /// reported and gives `None`.
    fn deref(&mut self, v: &Value, item: &str) -> Option<Value> {
        let mut v = v.clone();
        for _ in 0..16 {
            let Some(r) = v.get("$ref").and_then(Value::as_str) else {
                return Some(v);
            };
            let Some(pointer) = r.strip_prefix('#') else {
                self.skip(item, format!("the external reference {r} was not followed"));
                return None;
            };
            v = self.doc.pointer(pointer)?.clone();
        }
        None
    }

    /// The `$ref` resolver the schema sampler uses.
    fn resolver(&self) -> impl Fn(&str) -> Option<Value> + '_ {
        |r: &str| {
            r.strip_prefix('#')
                .and_then(|p| self.doc.pointer(p))
                .cloned()
        }
    }

    fn sample(&self, schema: &Value) -> Value {
        json_schema::sample(schema, &self.resolver())
    }
}

/// A value as request text: strings as they are, the rest as JSON.
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// `{name}` path templates become `{{name}}`.
fn template_path(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(i) = rest.find('{') {
        out.push_str(&rest[..i]);
        match rest[i..].find('}') {
            Some(j) => {
                out.push_str(&format!("{{{{{}}}}}", &rest[i + 1..i + j]));
                rest = &rest[i + j + 1..];
            }
            None => {
                out.push_str(&rest[i..]);
                rest = "";
            }
        }
    }
    out.push_str(rest);
    out
}

/// The base URLs: OpenAPI `servers` (variables at their default) or Swagger
/// `schemes`, `host` and `basePath`; each with a name for its environment.
fn servers(doc: &Value, swagger: bool) -> Vec<(String, String)> {
    if swagger {
        let host = doc.get("host").and_then(Value::as_str).unwrap_or("");
        let base = doc.get("basePath").and_then(Value::as_str).unwrap_or("");
        let base = base.trim_end_matches('/');
        if host.is_empty() {
            return if base.is_empty() {
                vec![]
            } else {
                vec![("default".into(), base.to_string())]
            };
        }
        let schemes: Vec<&str> = doc
            .get("schemes")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        let schemes = if schemes.is_empty() {
            vec!["https"]
        } else {
            schemes
        };
        return schemes
            .iter()
            .map(|s| (s.to_string(), format!("{s}://{host}{base}")))
            .collect();
    }
    doc.get("servers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(i, s)| {
            let mut url = s.get("url").and_then(Value::as_str)?.to_string();
            if let Some(vars) = s.get("variables").and_then(Value::as_object) {
                for (k, v) in vars {
                    let d = v.get("default").map(text_of).unwrap_or_default();
                    url = url.replace(&format!("{{{k}}}"), &d);
                }
            }
            let name = s
                .get("description")
                .and_then(Value::as_str)
                .filter(|d| !d.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("server {}", i + 1));
            Some((name, url.trim_end_matches('/').to_string()))
        })
        .collect()
}

/// The trimmed string at a JSON pointer; empty when it is absent.
fn str_at(v: &Value, pointer: &str) -> String {
    v.pointer(pointer)
        .and_then(Value::as_str)
        .map(|t| t.trim().to_string())
        .unwrap_or_default()
}

/// The description of a tag in the top-level `tags` list.
fn tag_description(doc: &Value, tag: &str) -> String {
    doc.get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|t| t.get("name").and_then(Value::as_str) == Some(tag))
        .map(|t| str_at(t, "/description"))
        .unwrap_or_default()
}

/// Parses an OpenAPI or Swagger document.
pub fn import(doc: &Value) -> Result<ImportResult, String> {
    if !is_openapi(doc) {
        return Err("not an OpenAPI or Swagger document".into());
    }
    let swagger = is_swagger(doc);
    let version = doc
        .get(if swagger { "swagger" } else { "openapi" })
        .map(text_of)
        .unwrap_or_default();
    if !(swagger && version.starts_with('2') || !swagger && version.starts_with('3')) {
        return Err(format!(
            "unsupported version {version} (OpenAPI 3.x or Swagger 2.0)"
        ));
    }
    let paths = doc
        .get("paths")
        .and_then(Value::as_object)
        .ok_or("the document has no \"paths\"")?;
    let mut ctx = Ctx {
        doc,
        swagger,
        skipped: Vec::new(),
        path_vars: Vec::new(),
        auth_vars: BTreeSet::new(),
        reported: HashSet::new(),
    };
    let title = doc
        .pointer("/info/title")
        .and_then(Value::as_str)
        .filter(|t| !t.trim().is_empty())
        .unwrap_or("API");
    let mut root = ImportFolder {
        name: title.to_string(),
        ..Default::default()
    };
    root.settings.description = str_at(doc, "/info/description");
    let global_security = doc.get("security").cloned();
    root.settings.auth =
        security_auth(&mut ctx, global_security.as_ref(), "(document)").unwrap_or_default();
    for (path, item) in paths {
        let Some(item) = ctx.deref(item, path) else {
            continue;
        };
        let shared = item.get("parameters").cloned();
        for method in METHODS {
            let Some(op) = item.get(*method) else {
                continue;
            };
            let name = op
                .get("operationId")
                .or_else(|| op.get("summary"))
                .and_then(Value::as_str)
                .filter(|s| !s.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("{} {path}", method.to_ascii_uppercase()));
            let tag = op
                .get("tags")
                .and_then(Value::as_array)
                .and_then(|t| t.first())
                .and_then(Value::as_str)
                .map(str::to_string);
            let label = match &tag {
                Some(t) => format!("{t} / {name}"),
                None => name.clone(),
            };
            let (mut req, examples) = operation(
                &mut ctx,
                method,
                path,
                op,
                shared.as_ref(),
                global_security.as_ref(),
                &label,
            );
            // The summary when the name does not already show it, then the description.
            let summary = str_at(op, "/summary");
            req.common.description = [
                if summary == name {
                    String::new()
                } else {
                    summary
                },
                str_at(op, "/description"),
            ]
            .into_iter()
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
            let folder = match tag {
                Some(t) => {
                    let i = match root.folders.iter().position(|f| f.name == t) {
                        Some(i) => i,
                        None => {
                            let mut f = ImportFolder {
                                name: t,
                                ..Default::default()
                            };
                            f.settings.description = tag_description(doc, &f.name);
                            root.folders.push(f);
                            root.folders.len() - 1
                        }
                    };
                    &mut root.folders[i]
                }
                None => &mut root,
            };
            if !examples.is_empty() {
                folder.examples.insert(folder.requests.len(), examples);
            }
            folder.requests.push((name, Request::Http(req)));
        }
    }
    let servers = servers(doc, swagger);
    let mut variables = Vec::new();
    match servers.first() {
        Some((_, url)) => variables.push(("baseUrl".to_string(), url.clone())),
        None => ctx.skip(
            "(document)",
            "the document names no server: set {{baseUrl}} in an environment",
        ),
    }
    for (k, v) in std::mem::take(&mut ctx.path_vars) {
        if !variables.iter().any(|(n, _)| *n == k) {
            variables.push((k, v));
        }
    }
    let environments = if servers.len() > 1 {
        servers
            .iter()
            .map(|(name, url)| ImportEnvironment {
                name: name.clone(),
                variables: vec![("baseUrl".into(), url.clone())],
            })
            .collect()
    } else {
        Vec::new()
    };
    if !ctx.auth_vars.is_empty() {
        let names: Vec<String> = ctx
            .auth_vars
            .iter()
            .map(|v| format!("{{{{{v}}}}}"))
            .collect();
        ctx.skip(
            "(document)",
            format!(
                "the auth uses {}: define them in an environment (as secrets)",
                names.join(", ")
            ),
        );
    }
    Ok(ImportResult {
        root,
        variables,
        environments,
        skipped: ctx.skipped,
    })
}

/// One operation as a request, and its response examples.
fn operation(
    ctx: &mut Ctx,
    method: &str,
    path: &str,
    op: &Value,
    shared: Option<&Value>,
    global_security: Option<&Value>,
    label: &str,
) -> (HttpRequest, Vec<Example>) {
    let mut req = HttpRequest {
        method: method.to_ascii_uppercase(),
        url: format!("{{{{baseUrl}}}}{}", template_path(path)),
        ..Default::default()
    };
    // Operation parameters replace the path item's of the same name and place.
    let mut params: Vec<Value> = Vec::new();
    for list in [op.get("parameters"), shared] {
        for p in list.and_then(Value::as_array).into_iter().flatten() {
            let Some(p) = ctx.deref(p, label) else {
                continue;
            };
            let key = |v: &Value| (text_of(&v["name"]), text_of(&v["in"]));
            if !params.iter().any(|q| key(q) == key(&p)) {
                params.push(p);
            }
        }
    }
    let mut form: Vec<(String, Value, bool)> = Vec::new();
    for p in &params {
        let name = text_of(&p["name"]);
        let required = p.get("required").and_then(Value::as_bool).unwrap_or(false);
        let value = p
            .get("example")
            .or_else(|| p.pointer("/schema/example"))
            .or_else(|| p.pointer("/schema/default"))
            .or_else(|| p.get("default"))
            .map(text_of)
            .unwrap_or_default();
        match text_of(&p["in"]).as_str() {
            "path" => {
                if !ctx.path_vars.iter().any(|(n, _)| *n == name) {
                    ctx.path_vars.push((name, value));
                }
            }
            "query" => req.params.push(KeyValue {
                name,
                value,
                enabled: required,
            }),
            "header" => req.headers.push(KeyValue {
                name,
                value,
                enabled: required,
            }),
            "body" => {
                let schema = p.get("schema").cloned().unwrap_or(Value::Null);
                let sample = ctx.sample(&schema);
                req.body = Body::Json {
                    text: serde_json::to_string_pretty(&sample).unwrap_or_default(),
                };
            }
            "formData" => {
                let file = text_of(&p["type"]) == "file";
                form.push((name, Value::String(value), file));
            }
            other => ctx.skip(
                label,
                format!("its {other} parameter {name} was not imported"),
            ),
        }
    }
    if !form.is_empty() {
        let consumes = op
            .get("consumes")
            .or_else(|| ctx.doc.get("consumes"))
            .map(Value::to_string)
            .unwrap_or_default();
        req.body = form_body(form, consumes.contains("multipart"));
    }
    if !ctx.swagger
        && let Some(body) = op.get("requestBody")
        && let Some(body) = ctx.deref(body, label)
    {
        req.body = request_body(ctx, &body, label);
    }
    let security = op.get("security");
    req.auth = match security {
        // The document's security: inherited from the root folder.
        None => Auth::Inherit,
        Some(s) if Some(s) == global_security => Auth::Inherit,
        // An empty list turns security off for this operation.
        Some(Value::Array(a)) if a.is_empty() => Auth::None,
        Some(s) => security_auth(ctx, Some(s), label).unwrap_or(Auth::None),
    };
    (req, response_examples(ctx, op, label))
}

/// A form or multipart body from Swagger `formData` parameters or schema
/// properties: (name, sample value, is a file).
fn form_body(fields: Vec<(String, Value, bool)>, multipart: bool) -> Body {
    if multipart || fields.iter().any(|(_, _, file)| *file) {
        Body::Multipart {
            parts: fields
                .into_iter()
                .map(|(name, value, file)| MultipartPart {
                    name,
                    kind: if file {
                        PartKind::File {
                            path: String::new(),
                            filename: None,
                        }
                    } else {
                        PartKind::Text {
                            value: text_of(&value),
                        }
                    },
                    content_type: String::new(),
                    enabled: true,
                })
                .collect(),
        }
    } else {
        Body::Form {
            fields: fields
                .into_iter()
                .map(|(name, value, _)| KeyValue::new(name, text_of(&value)))
                .collect(),
        }
    }
}

/// An OpenAPI 3 request body: JSON first, then a form, multipart, or the
/// first other media type as raw text.
fn request_body(ctx: &mut Ctx, body: &Value, label: &str) -> Body {
    let Some(content) = body.get("content").and_then(Value::as_object) else {
        return Body::None;
    };
    let pick = |pred: &dyn Fn(&str) -> bool| content.iter().find(|(ct, _)| pred(ct));
    let json = pick(&|ct| ct == "application/json" || ct.ends_with("+json"));
    let urlencoded = pick(&|ct| ct == "application/x-www-form-urlencoded");
    let multipart = pick(&|ct| ct.starts_with("multipart/"));
    if let Some((_, media)) = json {
        let value = media_example(ctx, media, label);
        return Body::Json {
            text: serde_json::to_string_pretty(&value).unwrap_or_default(),
        };
    }
    if let Some((ct, media)) = urlencoded.or(multipart) {
        let schema = media
            .get("schema")
            .and_then(|s| ctx.deref(s, label))
            .unwrap_or(Value::Null);
        let sample = ctx.sample(&schema);
        let fields = schema
            .get("properties")
            .and_then(Value::as_object)
            .map(|props| {
                props
                    .iter()
                    .map(|(name, p)| {
                        let file = matches!(
                            p.get("format").and_then(Value::as_str),
                            Some("binary" | "base64")
                        );
                        (
                            name.clone(),
                            sample.get(name).cloned().unwrap_or(Value::Null),
                            file,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        return form_body(fields, ct.starts_with("multipart/"));
    }
    match content.iter().next() {
        Some((ct, media)) => Body::Raw {
            content_type: ct.clone(),
            text: text_of(&media_example(ctx, media, label)),
        },
        None => Body::None,
    }
}

/// The example of a media type: `example`, the first of `examples`, else a
/// sample of its schema.
fn media_example(ctx: &mut Ctx, media: &Value, label: &str) -> Value {
    if let Some(v) = explicit_example(ctx, media, label) {
        return v;
    }
    let schema = media.get("schema").cloned().unwrap_or(Value::Null);
    ctx.sample(&schema)
}

/// `example` or the first of `examples` (its `value`), when the document
/// gives one.
fn explicit_example(ctx: &mut Ctx, media: &Value, label: &str) -> Option<Value> {
    if let Some(v) = media.get("example") {
        return Some(v.clone());
    }
    let first = media.get("examples")?.as_object()?.values().next()?.clone();
    let first = ctx.deref(&first, label)?;
    first.get("value").cloned()
}

/// The examples the responses of an operation give, one per status (and
/// per named example).
fn response_examples(ctx: &mut Ctx, op: &Value, label: &str) -> Vec<Example> {
    let mut out = Vec::new();
    let Some(responses) = op.get("responses").and_then(Value::as_object) else {
        return out;
    };
    for (code, resp) in responses {
        let Ok(status) = code.parse::<u16>() else {
            continue;
        };
        let Some(resp) = ctx.deref(resp, label) else {
            continue;
        };
        // Swagger 2.0: examples by media type.
        let medias: Vec<(String, Value)> = if ctx.swagger {
            resp.get("examples")
                .and_then(Value::as_object)
                .map(|m| {
                    m.iter()
                        .map(|(ct, v)| (ct.clone(), serde_json::json!({ "example": v })))
                        .collect()
                })
                .unwrap_or_default()
        } else {
            resp.get("content")
                .and_then(Value::as_object)
                .map(|m| m.iter().map(|(ct, v)| (ct.clone(), v.clone())).collect())
                .unwrap_or_default()
        };
        for (ct, media) in medias {
            let Some(value) = explicit_example(ctx, &media, label) else {
                continue;
            };
            let body = match &value {
                Value::String(s) => s.clone().into_bytes(),
                v => serde_json::to_string_pretty(v)
                    .unwrap_or_default()
                    .into_bytes(),
            };
            out.push(Example {
                name: Example::default_name(status, ""),
                status,
                reason: String::new(),
                headers: vec![("Content-Type".into(), ct)],
                body,
            });
        }
    }
    out
}

/// The auth of a security requirement list: the first scheme of the first
/// requirement. `None` when there is none, or it cannot be imported.
fn security_auth(ctx: &mut Ctx, security: Option<&Value>, label: &str) -> Option<Auth> {
    let requirement = security?.as_array()?.first()?.as_object()?;
    let name = requirement.keys().next()?.clone();
    let schemes = if ctx.swagger {
        "/securityDefinitions/"
    } else {
        "/components/securitySchemes/"
    };
    let scheme = ctx.doc.pointer(&format!(
        "{schemes}{}",
        name.replace('~', "~0").replace('/', "~1")
    ))?;
    let scheme = ctx.deref(&scheme.clone(), label)?;
    let var = |ctx: &mut Ctx, v: &str| {
        ctx.auth_vars.insert(v.to_string());
        format!("{{{{{v}}}}}")
    };
    match (
        text_of(&scheme["type"]).as_str(),
        text_of(&scheme["scheme"]).to_ascii_lowercase().as_str(),
    ) {
        ("basic", _) | ("http", "basic") => Some(Auth::Basic {
            username: var(ctx, "username"),
            password: var(ctx, "password"),
        }),
        ("http", "bearer") => Some(Auth::Bearer {
            token: var(ctx, "token"),
        }),
        ("http", "digest") => Some(Auth::Digest {
            username: var(ctx, "username"),
            password: var(ctx, "password"),
        }),
        ("apiKey", _) => {
            let location = match text_of(&scheme["in"]).as_str() {
                "header" => ApiKeyLocation::Header,
                "query" => ApiKeyLocation::Query,
                other => {
                    ctx.skip(
                        label,
                        format!("its {other} API key ({name}) was not imported"),
                    );
                    return None;
                }
            };
            Some(Auth::ApiKey {
                key: text_of(&scheme["name"]),
                value: var(ctx, "apiKey"),
                location,
            })
        }
        (kind, _) => {
            ctx.skip(
                label,
                format!("its {kind} security ({name}) was not imported: set the auth by hand"),
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn yaml(text: &str) -> Value {
        serde_norway::from_str(text).unwrap()
    }

    const PETSTORE: &str = r##"
openapi: 3.0.3
info: {title: Petstore, description: "# Pets"}
tags: [{name: pets, description: Pet calls}]
servers:
  - url: https://{env}.example.com/v1
    description: production
    variables: {env: {default: api}}
  - url: http://localhost:8080/v1
security: [{bearer: []}]
paths:
  /pets:
    get:
      operationId: listPets
      summary: List pets
      tags: [pets]
      parameters:
        - {name: limit, in: query, schema: {type: integer, default: 20}}
        - {name: X-Trace, in: header, required: true, schema: {type: string}}
      responses:
        "200":
          description: ok
          content:
            application/json:
              example: [{id: 1, name: Rex}]
    post:
      operationId: createPet
      tags: [pets]
      requestBody:
        $ref: "#/components/requestBodies/Pet"
      responses: {"201": {description: created}}
  /pets/{petId}:
    parameters:
      - {name: petId, in: path, required: true, example: "7"}
    get:
      summary: Show a pet
      description: Returns *one* pet.
      tags: [pets]
      security: []
      responses: {}
    delete:
      tags: [admin]
      security: [{key: []}]
  /upload:
    post:
      requestBody:
        content:
          multipart/form-data:
            schema:
              properties:
                file: {type: string, format: binary}
                note: {type: string}
components:
  requestBodies:
    Pet:
      content:
        application/json:
          schema: {$ref: "#/components/schemas/Pet"}
  schemas:
    Pet:
      type: object
      properties:
        name: {type: string, example: Rex}
        tag: {type: string}
  securitySchemes:
    bearer: {type: http, scheme: bearer}
    key: {type: apiKey, in: header, name: X-API-Key}
"##;

    #[test]
    fn imports_petstore() {
        let r = import(&yaml(PETSTORE)).unwrap();
        assert_eq!(r.root.name, "Petstore");
        assert_eq!(
            r.root.settings.auth,
            Auth::Bearer {
                token: "{{token}}".into()
            }
        );
        assert_eq!(
            r.variables,
            vec![
                (
                    "baseUrl".to_string(),
                    "https://api.example.com/v1".to_string()
                ),
                ("petId".to_string(), "7".to_string())
            ]
        );
        assert_eq!(r.environments.len(), 2);
        assert_eq!(r.environments[0].name, "production");
        let pets = &r.root.folders[0];
        assert_eq!(pets.name, "pets");
        let names: Vec<&str> = pets.requests.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["listPets", "createPet", "Show a pet"]);
        let Request::Http(list) = &pets.requests[0].1 else {
            panic!()
        };
        assert_eq!(list.url, "{{baseUrl}}/pets");
        assert_eq!(list.common.description, "List pets");
        assert_eq!(pets.requests[1].1.description(), "");
        assert_eq!(pets.requests[2].1.description(), "Returns *one* pet.");
        assert_eq!(pets.settings.description, "Pet calls");
        assert_eq!(r.root.settings.description, "# Pets");
        assert_eq!(list.params, vec![KeyValue::new("limit", "20").disabled()]);
        assert_eq!(list.headers, vec![KeyValue::new("X-Trace", "")]);
        assert_eq!(list.auth, Auth::Inherit);
        assert_eq!(pets.examples[&0][0].status, 200);
        assert!(String::from_utf8_lossy(&pets.examples[&0][0].body).contains("Rex"));
        let Request::Http(create) = &pets.requests[1].1 else {
            panic!()
        };
        assert_eq!(
            create.body,
            Body::Json {
                text: "{\n  \"name\": \"Rex\",\n  \"tag\": \"\"\n}".into()
            }
        );
        let Request::Http(show) = &pets.requests[2].1 else {
            panic!()
        };
        assert_eq!(show.url, "{{baseUrl}}/pets/{{petId}}");
        assert_eq!(show.auth, Auth::None);
        let Request::Http(delete) = &r.root.folders[1].requests[0].1 else {
            panic!()
        };
        assert_eq!(r.root.folders[1].requests[0].0, "DELETE /pets/{petId}");
        assert!(matches!(&delete.auth, Auth::ApiKey { key, .. } if key == "X-API-Key"));
        let Request::Http(upload) = &r.root.requests[0].1 else {
            panic!()
        };
        assert!(
            matches!(&upload.body, Body::Multipart { parts } if parts.len() == 2
            && matches!(parts[0].kind, PartKind::File { .. }))
        );
        let report: Vec<String> = r.skipped.iter().map(|s| s.reason.clone()).collect();
        assert!(report.iter().any(|s| s.contains("{{token}}")), "{report:?}");
    }

    const SWAGGER: &str = r#"{
      "swagger": "2.0",
      "info": {"title": "Old"},
      "host": "api.example.com",
      "basePath": "/v2/",
      "schemes": ["https"],
      "securityDefinitions": {"basic": {"type": "basic"}, "oauth": {"type": "oauth2"}},
      "paths": {
        "/users": {
          "post": {
            "operationId": "create",
            "security": [{"oauth": []}],
            "parameters": [{"name": "user", "in": "body", "schema": {"properties": {"id": {"type": "integer"}}}}],
            "responses": {"200": {"description": "ok", "examples": {"application/json": {"id": 1}}}}
          }
        },
        "/avatar": {
          "put": {
            "security": [{"basic": []}],
            "parameters": [
              {"name": "file", "in": "formData", "type": "file"},
              {"name": "alt", "in": "formData", "type": "string"}
            ]
          }
        }
      }
    }"#;

    #[test]
    fn imports_swagger_2() {
        let doc: Value = serde_json::from_str(SWAGGER).unwrap();
        let r = import(&doc).unwrap();
        assert_eq!(
            r.variables[0],
            ("baseUrl".into(), "https://api.example.com/v2".into())
        );
        let Request::Http(create) = &r.root.requests[0].1 else {
            panic!()
        };
        assert_eq!(
            create.body,
            Body::Json {
                text: "{\n  \"id\": 0\n}".into()
            }
        );
        assert_eq!(create.auth, Auth::None);
        assert_eq!(r.root.examples[&0][0].body, b"{\n  \"id\": 1\n}");
        let Request::Http(avatar) = &r.root.requests[1].1 else {
            panic!()
        };
        assert!(matches!(&avatar.auth, Auth::Basic { .. }));
        assert!(matches!(&avatar.body, Body::Multipart { parts } if parts.len() == 2));
        assert!(
            r.skipped
                .iter()
                .any(|s| s.reason.contains("oauth2 security"))
        );
    }

    #[test]
    fn errors_and_edge_cases() {
        assert!(import(&serde_json::json!({"openapi": "4.0", "paths": {}})).is_err());
        assert!(import(&serde_json::json!({"openapi": "3.1.0"})).is_err());
        let r = import(&serde_json::json!({"openapi": "3.1.0", "paths": {
            "/a": {"get": {"parameters": [{"$ref": "other.yaml#/p"}]}}
        }}))
        .unwrap();
        assert!(
            r.skipped
                .iter()
                .any(|s| s.reason.contains("external reference"))
        );
        assert!(r.skipped.iter().any(|s| s.reason.contains("no server")));
        assert_eq!(template_path("/a/{b}/c/{d}"), "/a/{{b}}/c/{{d}}");
    }
}
