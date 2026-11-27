//! The tools of `sendito serve`: read the collection, send requests, and
//! write request files. A failed tool answers `isError: true` with the reason,
//! so the agent can read it and try again; only an unknown tool is a
//! JSON-RPC error.
//!
//! A tool sees only the collection as the tree scan sees it: a file that is
//! not a request (`Cargo.toml`, `folder.toml`), a directory with no requests
//! (`src/` of a code repository) and hidden files are not part of it. A sent
//! request reads only the files of the collection (`confine_files`).

use std::time::Instant;

use base64::Engine;
use sendito_core::vars::Scope;
use sendito_pipeline::suite::{self, Cancel, Event, Sent};
use sendito_store::collection::{name_of, parent_rel, validate_rel};
use sendito_store::{Folder, request_file};
use serde_json::{Value, json};

use super::{Progress, ServeFlags, Server};
use crate::cli::ask::{Answers, answer_prompts};
use crate::cli::report::SuiteReport;
use crate::*;

/// What a tool reads: the collection as of this call, the server flags and
/// where to report progress.
struct Ctx<'a> {
    project: &'a Project,
    flags: &'a ServeFlags,
    progress: Option<&'a Progress>,
}

/// The body and message bytes a send returns when the call sets no `max_bytes`.
const MAX_BYTES: usize = 64 * 1024;

/// What a tool returns: its structured result, or the reason it failed.
type Outcome = Result<Value, String>;

/// What a tool does: `--read-only` leaves out `Write`, `--no-send` `Send`.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Read,
    Send,
    Write,
}

/// One tool: its definition and what it does.
struct Def {
    name: &'static str,
    title: &'static str,
    kind: Kind,
    description: &'static str,
    schema: Value,
    /// The schema of `structuredContent`.
    output: Value,
    annotations: Value,
}

/// An object schema with these properties and required keys.
fn object(properties: Value, required: &[&str]) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

fn defs() -> Vec<Def> {
    let read = json!({ "readOnlyHint": true, "openWorldHint": false });
    let send = json!({ "readOnlyHint": false, "openWorldHint": true });
    let path = json!({ "type": "string", "description": "Request path relative to the collection root, for example Users/list.toml (.toml can be left out)." });
    let node = |what: &str| json!({ "type": "string", "description": what });
    let folder =
        json!({ "type": "string", "description": "Folder path; default: the whole collection." });
    let env = json!({ "type": "string", "description": "Environment name; default: the one sendito serve started with." });
    let prompts = json!({ "type": "object", "additionalProperties": { "type": "string" }, "description": "Values for the $prompt and $select labels of the request, label → value." });
    let retry = json!({ "type": "integer", "minimum": 0, "description": "Retries after an error or a failed assertion; default: the request's [options] retry." });
    let max_bytes = json!({ "type": "integer", "minimum": 1, "description": "The most body (or message) bytes to return; default 65536." });
    let toml =
        json!({ "type": "string", "description": "The request as TOML, as in a request file." });
    let sent = object(
        json!({
            "path": { "type": "string" },
            "kind": { "type": "string" },
            "outcome": { "type": "string", "enum": ["pass", "fail", "error"] },
            "duration_ms": { "type": "integer" },
            "assertions": { "type": "array", "items": { "type": "object" } },
            "http": { "type": "object" },
            "body": { "type": "string" },
            "truncated": { "type": "boolean" },
            "hint": { "type": "string", "description": "The setting to change, when a configuration problem caused the failure." },
        }),
        &["path", "outcome"],
    );
    let moved = object(json!({ "path": { "type": "string" } }), &["path"]);
    vec![
        Def {
            name: "list_requests",
            title: "List requests",
            kind: Kind::Read,
            description: "List the requests and folders of the collection (or of one folder), in tree order, with the method or kind and the description of each request.",
            schema: json!({ "type": "object", "properties": {
                "folder": folder,
                "depth": { "type": "integer", "minimum": 1, "description": "How many folder levels to list; 1 lists only the folder's own requests and subfolders. Default: every level." },
            }}),
            output: object(
                json!({
                    "folder": { "type": "string" },
                    "folders": { "type": "array", "items": { "type": "string" } },
                    "requests": { "type": "array", "items": object(json!({
                        "path": { "type": "string" },
                        "badge": { "type": "string" },
                        "description": { "type": "string" },
                    }), &["path", "badge"]) },
                }),
                &["folder", "folders", "requests"],
            ),
            annotations: read.clone(),
        },
        Def {
            name: "read_request",
            title: "Read a request",
            kind: Kind::Read,
            description: "Read the TOML of one request file and of the folder settings (folder.toml) that apply to it.",
            schema: json!({ "type": "object", "properties": { "path": path }, "required": ["path"] }),
            output: object(
                json!({
                    "path": { "type": "string" },
                    "toml": { "type": "string" },
                    "folders": { "type": "array", "items": object(json!({
                        "folder": { "type": "string" },
                        "toml": { "type": "string" },
                    }), &["folder", "toml"]) },
                }),
                &["path", "toml", "folders"],
            ),
            annotations: read.clone(),
        },
        Def {
            name: "list_environments",
            title: "List environments",
            kind: Kind::Read,
            description: "List the environments and their variables. A secret variable shows its name and where its value is kept, never the value.",
            schema: json!({ "type": "object", "properties": {} }),
            output: object(
                json!({
                    "default": { "type": ["string", "null"] },
                    "environments": { "type": "array", "items": object(json!({
                        "name": { "type": "string" },
                        "variables": { "type": "array", "items": { "type": "object" } },
                    }), &["name", "variables"]) },
                }),
                &["default", "environments"],
            ),
            annotations: read.clone(),
        },
        Def {
            name: "preview_request",
            title: "Preview a request",
            kind: Kind::Read,
            description: "Show an HTTP or SSE request as a curl command, with its folder settings and the environment applied, without sending it. Secret variables stay {{placeholders}}; scripts and $prompt values do not run. Give a saved request (path) or TOML (toml).",
            schema: json!({ "type": "object", "properties": {
                "path": path.clone(), "toml": toml.clone(), "env": env.clone(),
            }}),
            output: object(json!({ "curl": { "type": "string" } }), &["curl"]),
            annotations: read.clone(),
        },
        Def {
            name: "import_curl",
            title: "Import a curl command",
            kind: Kind::Read,
            description: "Convert a curl command into Sendito request TOML. Nothing is sent or saved: give the TOML to write_request or send_request. The notes name the curl options that were ignored or approximated.",
            schema: json!({ "type": "object", "properties": {
                "command": { "type": "string", "description": "The curl command line." },
            }, "required": ["command"] }),
            output: object(
                json!({
                    "toml": { "type": "string" },
                    "notes": { "type": "array", "items": { "type": "string" } },
                }),
                &["toml", "notes"],
            ),
            annotations: read,
        },
        Def {
            name: "run_request",
            title: "Run a saved request",
            kind: Kind::Send,
            description: "Send one saved request with its folder settings, scripts and assertions. Returns the outcome (pass, fail or error), the status, the assertions and the response: status, headers and body for HTTP, the messages for gRPC, WebSocket, SSE, MQTT and MCP. Secrets the server echoes back are masked.",
            schema: json!({ "type": "object", "properties": {
                "path": path.clone(), "env": env.clone(), "prompts": prompts.clone(), "retry": retry.clone(), "max_bytes": max_bytes.clone(),
            }, "required": ["path"] }),
            output: sent.clone(),
            annotations: send.clone(),
        },
        Def {
            name: "run_folder",
            title: "Run a folder",
            kind: Kind::Send,
            description: "Send every request under a folder in tree order; a value one request extracts is available to the next. Returns one line per request (outcome, status, assertions), without bodies; with failures, also the response of each request that does not pass. Reports progress when the call gives a progressToken.",
            schema: json!({ "type": "object", "properties": {
                "folder": folder,
                "env": env.clone(), "prompts": prompts.clone(), "retry": retry,
                "fail_fast": { "type": "boolean", "description": "Stop at the first request that does not pass." },
                "data": { "type": "string", "description": "A CSV or JSON data file in the collection: the folder runs once per row." },
                "failures": { "type": "boolean", "description": "Also return the response of each request that does not pass, as run_request does." },
                "max_bytes": max_bytes.clone(),
            }}),
            output: object(
                json!({
                    "folder": { "type": "string" },
                    "total": { "type": "integer" },
                    "passed": { "type": "integer" },
                    "failed": { "type": "integer" },
                    "errors": { "type": "integer" },
                    "requests": { "type": "array", "items": { "type": "object" } },
                    "failures": { "type": "array", "items": sent.clone() },
                }),
                &["folder", "total", "requests"],
            ),
            annotations: send.clone(),
        },
        Def {
            name: "send_request",
            title: "Send a request given as TOML",
            kind: Kind::Send,
            description: "Send a request that no file holds, written as Sendito TOML (read the resource sendito://format first). Nothing is saved. Returns the same as run_request.",
            schema: json!({ "type": "object", "properties": {
                "toml": toml, "env": env, "prompts": prompts, "max_bytes": max_bytes,
            }, "required": ["toml"] }),
            output: sent,
            annotations: send,
        },
        Def {
            name: "write_request",
            title: "Write a request",
            kind: Kind::Write,
            description: "Create or replace a request file from Sendito TOML (read the resource sendito://format first). Missing folders are created. The file is written in the canonical format.",
            schema: json!({ "type": "object", "properties": {
                "path": path,
                "toml": { "type": "string", "description": "The request as TOML." },
            }, "required": ["path", "toml"] }),
            output: object(
                json!({ "path": { "type": "string" }, "created": { "type": "boolean" } }),
                &["path", "created"],
            ),
            annotations: json!({ "readOnlyHint": false, "destructiveHint": true, "idempotentHint": true, "openWorldHint": false }),
        },
        Def {
            name: "create_folder",
            title: "Create a folder",
            kind: Kind::Write,
            description: "Create a folder (and the folders above it that are missing).",
            schema: json!({ "type": "object", "properties": {
                "path": node("Folder path relative to the collection root."),
            }, "required": ["path"] }),
            output: moved.clone(),
            annotations: json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false }),
        },
        Def {
            name: "move",
            title: "Move or rename",
            kind: Kind::Write,
            description: "Move or rename a request or a folder in one step: when the new path is taken, nothing changes. A request keeps its saved examples.",
            schema: json!({ "type": "object", "properties": {
                "path": node("The request or folder to move."),
                "to": node("Its new path, for example Admin/users.toml."),
            }, "required": ["path", "to"] }),
            output: moved,
            annotations: json!({ "readOnlyHint": false, "destructiveHint": false, "openWorldHint": false }),
        },
        Def {
            name: "delete",
            title: "Delete",
            kind: Kind::Write,
            description: "Delete a request file, or a folder with everything in it.",
            schema: json!({ "type": "object", "properties": {
                "path": node("The request or folder to delete."),
            }, "required": ["path"] }),
            output: object(json!({ "deleted": { "type": "string" } }), &["deleted"]),
            annotations: json!({ "readOnlyHint": false, "destructiveHint": true, "openWorldHint": false }),
        },
    ]
}

/// The flags offer the tools of this kind.
fn offered(flags: &ServeFlags, kind: Kind) -> bool {
    !(flags.read_only && kind == Kind::Write) && !(flags.no_send && kind == Kind::Send)
}

/// The `tools/list` result: the tools that the flags offer.
pub(super) fn list(flags: &ServeFlags) -> Value {
    let tools: Vec<Value> = defs()
        .into_iter()
        .filter(|d| offered(flags, d.kind))
        .map(|d| {
            json!({
                "name": d.name,
                "title": d.title,
                "description": d.description,
                "inputSchema": d.schema,
                "outputSchema": d.output,
                "annotations": d.annotations,
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// Runs the tool `name`. `Err` is a protocol error: the tool is not offered.
pub(super) async fn call(
    server: &Server,
    name: &str,
    args: &Value,
    cancel: &Cancel,
    progress: Option<&Progress>,
) -> Result<Value, String> {
    if !defs()
        .iter()
        .any(|d| d.name == name && offered(&server.flags, d.kind))
    {
        return Err(format!("unknown tool {name:?}"));
    }
    let project = server.project();
    let cx = &Ctx {
        project: &project,
        flags: &server.flags,
        progress,
    };
    let outcome = match name {
        "list_requests" => list_requests(cx, args),
        "read_request" => read_request(cx, args),
        "list_environments" => list_environments(cx),
        "preview_request" => preview_request(cx, args),
        "import_curl" => import_curl(args),
        "run_request" => run_request(cx, args, cancel).await,
        "run_folder" => run_folder(cx, args, cancel).await,
        "send_request" => send_request(cx, args, cancel).await,
        "write_request" => write_request(cx, args),
        "create_folder" => create_folder(cx, args),
        "move" => move_node(cx, args),
        "delete" => delete(cx, args),
        _ => Err(format!("unknown tool {name:?}")),
    };
    Ok(match outcome {
        Ok(v) => json!({
            "content": [{ "type": "text", "text": serde_json::to_string_pretty(&v).unwrap_or_default() }],
            "structuredContent": v,
        }),
        Err(e) => json!({
            "content": [{ "type": "text", "text": e }],
            "isError": true,
        }),
    })
}

// ---- arguments ----

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("the argument {key:?} (a string) is missing"))
}

fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// A positive integer argument; `None` when the call does not give it.
fn positive_arg(args: &Value, key: &str) -> Result<Option<usize>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .filter(|n| *n > 0)
            .map(|n| Some(usize::try_from(n).unwrap_or(usize::MAX)))
            .ok_or_else(|| format!("the argument {key:?} must be an integer of 1 or more")),
    }
}

fn max_bytes_arg(args: &Value) -> Result<usize, String> {
    Ok(positive_arg(args, "max_bytes")?.unwrap_or(MAX_BYTES))
}

/// The `prompts` argument as `--prompt` pairs.
fn prompts_arg(args: &Value) -> Result<Vec<(String, String)>, String> {
    let Some(v) = args.get("prompts") else {
        return Ok(Vec::new());
    };
    let map = v
        .as_object()
        .ok_or("the argument \"prompts\" must be an object of strings")?;
    map.iter()
        .map(|(k, v)| match v.as_str() {
            Some(s) => Ok((k.clone(), s.to_string())),
            None => Err(format!("the prompt {k:?} must be a string")),
        })
        .collect()
}

// ---- paths ----

/// A path the agent gives, relative to the collection root, `/`-separated
/// and without empty parts. No part may leave the collection or name a
/// hidden file (`.sendito`, `.env`): those hold secrets and settings.
fn clean_path(path: &str) -> Result<String, String> {
    let rel = path
        .split('/')
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join("/");
    validate_rel(&rel).map_err(|e| e.to_string())?;
    if rel.split('/').any(|p| p.starts_with('.')) {
        return Err(format!(
            "{path:?}: a path part cannot start with '.' (hidden files are not part of the collection)"
        ));
    }
    Ok(rel)
}

/// A request path: [`clean_path`] with the `.toml` extension.
fn request_path(path: &str) -> Result<String, String> {
    let rel = clean_path(path)?;
    if rel.is_empty() {
        return Err("the request path is empty".into());
    }
    Ok(if rel.ends_with(".toml") {
        rel
    } else {
        format!("{rel}.toml")
    })
}

/// The tree of the collection, as the scan sees it.
fn tree(cx: &Ctx<'_>) -> Result<Folder, String> {
    cx.project.coll.scan().map_err(|e| e.to_string())
}

/// `rel` in the collection: a symbolic link cannot lead out of it.
fn confined(cx: &Ctx<'_>, rel: &str) -> Result<(), String> {
    let coll = &cx.project.coll;
    runner::confined(&coll.root, &coll.abs(rel))
}

/// A node of the collection tree.
enum Node {
    Request(String),
    Folder(String),
}

impl Node {
    fn rel(&self) -> &str {
        match self {
            Node::Request(rel) | Node::Folder(rel) => rel,
        }
    }
}

/// The request or folder of `tree` at `path`: a folder when the tree has
/// one there, else a request.
fn node(cx: &Ctx<'_>, tree: &Folder, path: &str) -> Result<Node, String> {
    let rel = clean_path(path)?;
    if rel.is_empty() {
        return Err("the path names the collection root".into());
    }
    let node = if tree.find_folder(&rel).is_some() {
        Node::Folder(rel)
    } else {
        let rel = request_path(&rel)?;
        if tree.find_request(&rel).is_none() {
            return Err(format!(
                "{rel} is not a request or a folder of the collection"
            ));
        }
        Node::Request(rel)
    };
    confined(cx, node.rel())?;
    Ok(node)
}

/// The request of `tree` at `path`.
fn existing_request(cx: &Ctx<'_>, tree: &Folder, path: &str) -> Result<String, String> {
    match node(cx, tree, path) {
        Ok(Node::Request(rel)) => Ok(rel),
        Ok(Node::Folder(rel)) => Err(format!("{rel} is a folder, not a request")),
        Err(_) => Err(format!("request {} does not exist", request_path(path)?)),
    }
}

/// The folder of `tree` at `rel` (`""` is the root).
fn existing_folder<'t>(tree: &'t Folder, rel: &str) -> Result<&'t Folder, String> {
    tree.find_folder(rel)
        .ok_or_else(|| format!("folder {rel:?} does not exist"))
}

/// A new node can go at `rel`: the nearest directory above it that exists
/// is a folder of the collection, so a tool cannot write into `src/` or
/// over a file that is not a request.
fn check_new(cx: &Ctx<'_>, tree: &Folder, rel: &str) -> Result<(), String> {
    let coll = &cx.project.coll;
    if coll.abs(rel).exists() {
        return Err(format!("{rel} already exists"));
    }
    let mut dir = parent_rel(rel);
    while !dir.is_empty() && !coll.abs(dir).exists() {
        dir = parent_rel(dir);
    }
    if dir.is_empty() {
        return Ok(());
    }
    if tree.find_folder(dir).is_none() {
        return Err(format!("{dir} is not a folder of the collection"));
    }
    confined(cx, dir)
}

// ---- reading ----

fn list_requests(cx: &Ctx<'_>, args: &Value) -> Outcome {
    let folder = clean_path(opt_str(args, "folder").unwrap_or(""))?;
    let depth = positive_arg(args, "depth")?;
    let tree = tree(cx)?;
    let start = existing_folder(&tree, &folder)?;
    let (mut requests, mut folders) = (Vec::new(), Vec::new());
    walk(start, depth, &mut requests, &mut folders);
    Ok(json!({ "folder": folder, "folders": folders, "requests": requests }))
}

/// The requests and the subfolders of `f`, in tree order, down to `depth`
/// levels (`None`: every level).
fn walk(f: &Folder, depth: Option<usize>, requests: &mut Vec<Value>, folders: &mut Vec<String>) {
    for r in &f.requests {
        let mut v = json!({ "path": r.rel, "badge": r.badge });
        if !r.description.is_empty() {
            v["description"] = r.description.clone().into();
        }
        requests.push(v);
    }
    for sub in &f.folders {
        folders.push(sub.rel.clone());
        if depth != Some(1) {
            walk(sub, depth.map(|d| d - 1), requests, folders);
        }
    }
}

fn read_request(cx: &Ctx<'_>, args: &Value) -> Outcome {
    let rel = existing_request(cx, &tree(cx)?, str_arg(args, "path")?)?;
    let coll = &cx.project.coll;
    let toml = std::fs::read_to_string(coll.abs(&rel)).map_err(|e| format!("{rel}: {e}"))?;
    let folders: Vec<Value> = coll
        .chain(&rel)
        .map_err(|e| e.to_string())?
        .iter()
        .map(|(rel, f)| json!({ "folder": rel, "toml": sendito_store::folder::to_toml(f) }))
        .collect();
    Ok(json!({ "path": rel, "toml": toml, "folders": folders }))
}

fn list_environments(cx: &Ctx<'_>) -> Outcome {
    let envs = &cx.project.envs;
    let list: Result<Vec<Value>, String> = envs
        .list()
        .iter()
        .filter(|name| cx.flags.allows(name))
        .map(|name| {
            let file = envs.load(name).map_err(|e| e.to_string())?;
            let vars: Vec<Value> = file
                .vars
                .iter()
                .map(|v| match &v.source {
                    sendito_store::env::VarSource::Plain(value) => {
                        json!({ "name": v.name, "secret": false, "value": value })
                    }
                    source => {
                        json!({ "name": v.name, "secret": true, "source": source.label() })
                    }
                })
                .collect();
            Ok(json!({ "name": name, "variables": vars }))
        })
        .collect();
    Ok(json!({ "default": cx.flags.env, "environments": list? }))
}

/// A request given as TOML.
fn parse_toml(args: &Value) -> Result<Request, String> {
    request_file::parse(str_arg(args, "toml")?)
        .map_err(|e| format!("the TOML is not a valid request: {e}"))
}

fn preview_request(cx: &Ctx<'_>, args: &Value) -> Outcome {
    let env = env_arg(cx, args)?;
    let coll = &cx.project.coll;
    let mut scope = runner::names_scope(&cx.project.envs, env.as_deref(), &Default::default());
    let req = match (opt_str(args, "path"), args.get("toml")) {
        (Some(path), None) => {
            let rel = existing_request(cx, &tree(cx)?, path)?;
            let req = coll.load(&rel).map_err(|e| e.to_string())?;
            let chain = coll.chain(&rel).map_err(|e| e.to_string())?;
            runner::with_folders(&req, &chain, &mut scope)
        }
        (None, Some(_)) => parse_toml(args)?,
        _ => return Err("give the argument \"path\" or \"toml\", not both".into()),
    };
    let http = sendito_import::http_export_request(&req, &scope).ok_or_else(|| {
        format!(
            "preview_request shows HTTP and SSE requests only, not {}",
            req.badge()
        )
    })?;
    let curl = sendito_import::curl::export(&http, &scope, false, &cx.project.root);
    Ok(json!({ "curl": curl }))
}

fn import_curl(args: &Value) -> Outcome {
    let imported =
        sendito_import::curl::import(str_arg(args, "command")?).map_err(|e| e.to_string())?;
    let toml = request_file::to_toml(&Request::Http(imported.request));
    Ok(json!({ "toml": toml, "notes": imported.notes }))
}

// ---- sending ----

/// The environment of a call: its `env`, else the server's. An environment
/// that `--allow-env` leaves out is refused.
fn env_arg(cx: &Ctx<'_>, args: &Value) -> Result<Option<String>, String> {
    let Some(env) = opt_str(args, "env") else {
        return Ok(cx.flags.env.clone());
    };
    let mut have = cx.project.envs.list();
    have.retain(|e| cx.flags.allows(e));
    if have.iter().any(|e| e == env) {
        Ok(Some(env.to_string()))
    } else if !cx.flags.allows(env) {
        Err(format!(
            "environment {env:?} is not allowed: sendito serve --allow-env offers only {}",
            have.join(", ")
        ))
    } else {
        Err(format!(
            "environment {env:?} does not exist (have: {})",
            have.join(", ")
        ))
    }
}

/// The run options of a call: secrets masked, the output kept, only the
/// files of the collection read.
fn options(cx: &Ctx<'_>, args: &Value, env: Option<String>) -> suite::Options {
    suite::Options {
        env,
        no_history: cx.flags.no_history,
        no_cookies: cx.flags.no_cookies,
        fail_fast: args
            .get("fail_fast")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        retry: args
            .get("retry")
            .and_then(Value::as_u64)
            .map(|n| u32::try_from(n).unwrap_or(u32::MAX)),
        mask_echoed_secrets: true,
        keep_output: true,
        confine_files: true,
        ..Default::default()
    }
}

/// Gives the `$prompt` and `$select` values from the `prompts` argument; a
/// run asks each label once. It borrows only `prompts`: [`Scope`] is not
/// `Sync`, so a borrow of [`Start`] cannot live across an `await`.
fn ask(prompts: &[(String, String)]) -> impl FnMut(&Request, &Scope) -> Result<(), String> + '_ {
    let mut answers = Answers::new();
    move |req: &Request, scope: &Scope| answer_prompts(req, scope, prompts, &mut answers, false)
}

/// What one call sends from: the loaded scope and the run settings.
struct Start {
    scope: Scope,
    errors: Vec<String>,
    failed: Vec<String>,
    opts: suite::Options,
    prompts: Vec<(String, String)>,
    max_bytes: usize,
}

async fn start(cx: &Ctx<'_>, args: &Value) -> Result<Start, String> {
    let env = env_arg(cx, args)?;
    let prompts = prompts_arg(args)?;
    let max_bytes = max_bytes_arg(args)?;
    let (scope, errors, failed) = scope_of(cx.project, &env).await;
    Ok(Start {
        scope,
        errors,
        failed,
        opts: options(cx, args, env),
        prompts,
        max_bytes,
    })
}

impl Start {
    fn plan<'a>(&'a self, cx: &Ctx<'a>) -> suite::Plan<'a> {
        suite::Plan {
            project: cx.project,
            scope: self.scope.clone(),
            scope_errors: &self.errors,
            failed: &self.failed,
        }
    }

    /// The result of one send for the agent.
    fn value(&self, sent: &Sent, notes: &[String]) -> Value {
        sent_value(sent, &self.errors, notes, self.max_bytes)
    }
}

async fn run_request(cx: &Ctx<'_>, args: &Value, cancel: &Cancel) -> Outcome {
    let rel = existing_request(cx, &tree(cx)?, str_arg(args, "path")?)?;
    let s = start(cx, args).await?;
    let (mut sent, mut notes) = (None, Vec::new());
    let on = |ev: Event| match ev {
        Event::Finished(done) => sent = Some(*done),
        Event::Note(n) => notes.push(n),
        Event::Log(_) | Event::Retry { .. } => {}
    };
    suite::run(s.plan(cx), &[rel], &s.opts, ask(&s.prompts), on, cancel).await;
    let sent = sent.ok_or("the request was cancelled")?;
    Ok(s.value(&sent, &notes))
}

async fn send_request(cx: &Ctx<'_>, args: &Value, cancel: &Cancel) -> Outcome {
    let req = parse_toml(args)?;
    let s = start(cx, args).await?;
    let mut notes = Vec::new();
    let on = |ev: Event| {
        if let Event::Note(n) = ev {
            notes.push(n);
        }
    };
    let sent = suite::run_request(
        s.plan(cx),
        &req,
        "ad hoc",
        &s.opts,
        ask(&s.prompts),
        on,
        cancel,
    )
    .await;
    Ok(s.value(&sent, &notes))
}

async fn run_folder(cx: &Ctx<'_>, args: &Value, cancel: &Cancel) -> Outcome {
    let folder = clean_path(opt_str(args, "folder").unwrap_or(""))?;
    let coll = &cx.project.coll;
    let rels = coll.requests_under(&folder).map_err(|e| e.to_string())?;
    if rels.is_empty() {
        return Err(format!("no requests under {folder:?}"));
    }
    let mut s = start(cx, args).await?;
    if let Some(data) = opt_str(args, "data") {
        let rel = clean_path(data)?;
        confined(cx, &rel)?;
        s.opts.data = suite::load_data(&coll.abs(&rel))?;
    }
    let with_failures = args
        .get("failures")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let total = rels.len() * s.opts.data.len().max(1);
    let started = Instant::now();
    let mut report = SuiteReport {
        environment: s.opts.env.clone(),
        folder: if folder.is_empty() {
            ".".into()
        } else {
            folder
        },
        ..Default::default()
    };
    let mut failures = Vec::new();
    let on = |ev: Event| {
        if let Event::Finished(done) = ev {
            if with_failures && done.entry.outcome != "pass" {
                failures.push(sent_value(&done, &[], &[], s.max_bytes));
            }
            if let Some(p) = cx.progress {
                p.send(report.entries.len() + 1, total, &done.entry.path);
            }
            report.entries.push(done.entry);
        }
    };
    suite::run(s.plan(cx), &rels, &s.opts, ask(&s.prompts), on, cancel).await;
    report.duration_ms = started.elapsed().as_millis();
    let mut v = report.value();
    if with_failures {
        v["failures"] = failures.into();
    }
    if !s.errors.is_empty() {
        v["warnings"] = json!(s.errors);
    }
    Ok(v)
}

/// The result of one send for the agent.
fn sent_value(sent: &Sent, warnings: &[String], notes: &[String], max: usize) -> Value {
    let e = &sent.entry;
    let mut v = e.json();
    v["assertions"] = e
        .asserts
        .iter()
        .map(|a| json!({ "label": a.label, "pass": a.pass, "actual": a.actual }))
        .collect();
    if let Some(o) = &sent.response {
        if let Ok(r) = &o.response {
            let headers: Vec<Value> = r.headers.iter().map(|(k, v)| json!([k, v])).collect();
            v["http"] = json!({
                "version": r.version,
                "status": r.status,
                "reason": r.reason,
                "url": r.url,
                "headers": headers,
            });
            if r.truncated {
                v["http"]["cut_by_max_body_mb"] = true.into();
            }
        }
        put_body(&mut v, o.doc.bytes(), max);
    } else if !sent.output.is_empty() {
        put_messages(&mut v, &sent.output, max);
    }
    if !warnings.is_empty() {
        v["warnings"] = json!(warnings);
    }
    if !notes.is_empty() {
        v["notes"] = json!(notes);
    }
    v
}

/// The longest start of `s` that fits in `max` bytes and ends on a char.
fn cut_str(s: &str, max: usize) -> &str {
    let mut end = max.min(s.len());
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The body, cut at `max` bytes: text when it is UTF-8, else base64.
fn put_body(v: &mut Value, bytes: &[u8], max: usize) {
    match std::str::from_utf8(bytes) {
        Ok(text) => v["body"] = cut_str(text, max).into(),
        Err(_) => {
            let cut = &bytes[..bytes.len().min(max)];
            v["body"] = base64::engine::general_purpose::STANDARD.encode(cut).into();
            v["body_encoding"] = "base64".into();
        }
    }
    v["body_bytes"] = bytes.len().into();
    v["truncated"] = (bytes.len() > max).into();
}

/// The messages, in order, until they fill `max` bytes.
fn put_messages(v: &mut Value, output: &[String], max: usize) {
    let mut kept = Vec::new();
    let mut left = max;
    for m in output {
        if m.len() <= left {
            kept.push(m.as_str());
            left -= m.len();
        } else {
            if left > 0 {
                kept.push(cut_str(m, left));
            }
            break;
        }
    }
    let truncated = output.iter().map(String::len).sum::<usize>() > max;
    v["messages"] = json!(kept);
    v["message_count"] = output.len().into();
    v["truncated"] = truncated.into();
}

// ---- writing ----

fn write_request(cx: &Ctx<'_>, args: &Value) -> Outcome {
    let rel = request_path(str_arg(args, "path")?)?;
    let req = parse_toml(args)?;
    let coll = &cx.project.coll;
    let tree = tree(cx)?;
    let created = tree.find_request(&rel).is_none();
    if created {
        check_new(cx, &tree, &rel)?;
        coll.create_request(parent_rel(&rel), name_of(&rel), &req)
    } else {
        confined(cx, &rel)?;
        coll.save(&rel, &req).map(|_| rel.clone())
    }
    .map_err(|e| e.to_string())?;
    Ok(json!({ "path": rel, "created": created }))
}

fn create_folder(cx: &Ctx<'_>, args: &Value) -> Outcome {
    let rel = clean_path(str_arg(args, "path")?)?;
    if rel.is_empty() {
        return Err("the folder path is empty".into());
    }
    check_new(cx, &tree(cx)?, &rel)?;
    let rel = cx
        .project
        .coll
        .create_folder(parent_rel(&rel), name_of(&rel))
        .map_err(|e| e.to_string())?;
    Ok(json!({ "path": rel }))
}

fn move_node(cx: &Ctx<'_>, args: &Value) -> Outcome {
    let tree = tree(cx)?;
    let from = node(cx, &tree, str_arg(args, "path")?)?;
    let to = str_arg(args, "to")?;
    let to = match from {
        Node::Request(_) => request_path(to)?,
        Node::Folder(_) => clean_path(to)?,
    };
    if to.is_empty() {
        return Err("the new path is empty".into());
    }
    let from = from.rel();
    // A rename that changes only the case keeps the same file.
    if !to.eq_ignore_ascii_case(from) {
        check_new(cx, &tree, &to)?;
    }
    let rel = cx
        .project
        .coll
        .relocate(from, parent_rel(&to), name_of(&to))
        .map_err(|e| e.to_string())?;
    Ok(json!({ "path": rel }))
}

fn delete(cx: &Ctx<'_>, args: &Value) -> Outcome {
    let node = node(cx, &tree(cx)?, str_arg(args, "path")?)?;
    let rel = node.rel();
    cx.project.coll.delete(rel).map_err(|e| e.to_string())?;
    Ok(json!({ "deleted": rel }))
}

#[cfg(test)]
mod tests {
    use super::super::tests::server;
    use super::*;

    fn rt() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    fn run(s: &Server, name: &str, args: Value) -> Value {
        rt().block_on(call(s, name, &args, &Cancel::default(), None))
            .unwrap()
    }

    fn error_text(v: &Value) -> &str {
        assert_eq!(v["isError"], true, "{v}");
        v["content"][0]["text"].as_str().unwrap()
    }

    fn names(v: Value) -> Vec<String> {
        v["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap().to_string())
            .collect()
    }

    const GET: &str = "kind = \"http\"\nmethod = \"GET\"\nurl = \"http://example.test/\"\n";

    #[test]
    fn read_only_leaves_out_the_tools_that_write() {
        let (_d, mut s) = server(true);
        let ro = names(list(&s.flags));
        assert!(!ro.contains(&"delete".to_string()));
        assert!(!ro.contains(&"write_request".to_string()));
        assert!(ro.contains(&"run_request".to_string()));
        assert!(ro.contains(&"preview_request".to_string()));
        let refused = rt().block_on(call(
            &s,
            "delete",
            &json!({"path":"a"}),
            &Cancel::default(),
            None,
        ));
        assert!(refused.unwrap_err().contains("unknown tool"));
        s.flags.read_only = false;
        assert!(names(list(&s.flags)).contains(&"delete".to_string()));
    }

    #[test]
    fn no_send_leaves_out_the_tools_that_send() {
        let (_d, mut s) = server(false);
        s.flags.no_send = true;
        let tools = names(list(&s.flags));
        for name in ["run_request", "run_folder", "send_request"] {
            assert!(!tools.contains(&name.to_string()), "{name}");
        }
        assert!(tools.contains(&"write_request".to_string()));
        assert!(tools.contains(&"preview_request".to_string()));
        let refused = rt().block_on(call(
            &s,
            "send_request",
            &json!({"toml": GET}),
            &Cancel::default(),
            None,
        ));
        assert!(refused.unwrap_err().contains("unknown tool"));
    }

    #[test]
    fn every_tool_has_a_title_and_an_output_schema() {
        let (_d, s) = server(false);
        for t in list(&s.flags)["tools"].as_array().unwrap() {
            assert!(t["title"].is_string(), "{t}");
            assert_eq!(t["outputSchema"]["type"], "object", "{t}");
        }
    }

    #[test]
    fn paths_stay_in_the_collection_and_out_of_hidden_files() {
        assert_eq!(clean_path("/a//b/").unwrap(), "a/b");
        assert!(clean_path("../x").is_err());
        assert!(clean_path("a/../../x").is_err());
        assert!(clean_path(".sendito/secrets.toml").is_err());
        assert!(clean_path(".env").is_err());
        assert_eq!(request_path("Users/list").unwrap(), "Users/list.toml");
        assert_eq!(request_path("Users/list.toml").unwrap(), "Users/list.toml");
    }

    #[test]
    fn write_read_move_and_delete_a_request() {
        let (dir, s) = server(false);
        let v = run(&s, "write_request", json!({"path": "A/one", "toml": GET}));
        assert_eq!(v["structuredContent"]["created"], true, "{v}");
        assert!(dir.path().join("A/one.toml").is_file());
        let v = run(&s, "write_request", json!({"path": "A/one", "toml": GET}));
        assert_eq!(v["structuredContent"]["created"], false, "{v}");
        let v = run(&s, "read_request", json!({"path": "A/one.toml"}));
        assert!(
            v["structuredContent"]["toml"]
                .as_str()
                .unwrap()
                .contains("example.test")
        );
        let v = run(&s, "list_requests", json!({}));
        assert_eq!(v["structuredContent"]["requests"][0]["path"], "A/one.toml");
        assert_eq!(v["structuredContent"]["requests"][0]["badge"], "GET");
        let v = run(&s, "move", json!({"path": "A/one", "to": "B/two"}));
        assert_eq!(v["structuredContent"]["path"], "B/two.toml", "{v}");
        let v = run(&s, "delete", json!({"path": "B/two"}));
        assert_eq!(v["structuredContent"]["deleted"], "B/two.toml");
        assert!(!dir.path().join("B/two.toml").exists());
        let v = run(&s, "read_request", json!({"path": "B/two"}));
        assert_eq!(v["isError"], true);
    }

    #[test]
    fn files_that_are_not_part_of_the_collection_are_refused() {
        let (dir, s) = server(false);
        let root = dir.path();
        std::fs::create_dir(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        run(&s, "write_request", json!({"path": "A/one", "toml": GET}));
        std::fs::write(root.join("A/folder.toml"), "[headers]\n").unwrap();
        for path in ["src", "Cargo", "A/folder"] {
            let v = run(&s, "delete", json!({"path": path}));
            assert!(error_text(&v).contains("not a request or a folder"), "{v}");
            let v = run(&s, "read_request", json!({"path": path}));
            assert_eq!(v["isError"], true, "{v}");
        }
        for path in ["Cargo", "A/folder", "src/new", "src/deep/new"] {
            let v = run(&s, "write_request", json!({"path": path, "toml": GET}));
            assert_eq!(v["isError"], true, "{path}: {v}");
        }
        let v = run(&s, "create_folder", json!({"path": "src/x"}));
        assert!(error_text(&v).contains("src is not a folder"), "{v}");
        let v = run(&s, "move", json!({"path": "A/one", "to": "src/one"}));
        assert_eq!(v["isError"], true, "{v}");
        assert!(root.join("src/main.rs").is_file());
        assert!(root.join("Cargo.toml").is_file());
        assert!(root.join("A/folder.toml").is_file());
        assert!(root.join("A/one.toml").is_file());
    }

    #[test]
    fn a_move_to_a_taken_path_changes_nothing() {
        let (dir, s) = server(false);
        run(&s, "write_request", json!({"path": "A/one", "toml": GET}));
        run(&s, "write_request", json!({"path": "B/two", "toml": GET}));
        let v = run(&s, "move", json!({"path": "A/one", "to": "B/two"}));
        assert!(error_text(&v).contains("already exists"), "{v}");
        assert!(dir.path().join("A/one.toml").is_file());
        let v = run(&s, "move", json!({"path": "A", "to": "C/D"}));
        assert_eq!(v["structuredContent"]["path"], "C/D", "{v}");
        assert!(dir.path().join("C/D/one.toml").is_file());
    }

    #[test]
    fn a_sent_request_reads_only_the_files_of_the_collection() {
        let (dir, s) = server(false);
        let root = dir.path();
        std::fs::write(root.join(".sendito/secrets.toml"), "[dev]\nt = \"x\"\n").unwrap();
        let outside = tempfile::NamedTempFile::new().unwrap();
        for path in [
            outside.path().to_string_lossy().into_owned(),
            ".sendito/secrets.toml".into(),
            "../x".into(),
        ] {
            let toml = format!(
                "kind = \"http\"\nmethod = \"POST\"\nurl = \"http://127.0.0.1:9/\"\n[body]\ntype = \"file\"\npath = {path:?}\n"
            );
            let v = run(&s, "send_request", json!({"toml": toml}));
            let c = &v["structuredContent"];
            assert_eq!(c["outcome"], "error", "{v}");
            let text = c["status"].as_str().unwrap_or_default();
            assert!(
                text.contains("reads only the files of the collection"),
                "{path}: {v}"
            );
        }
    }

    #[test]
    fn list_requests_shows_descriptions_and_stops_at_depth() {
        let (_d, s) = server(false);
        let toml = format!("{GET}description = \"Lists users.\"\n");
        run(&s, "write_request", json!({"path": "top", "toml": toml}));
        run(
            &s,
            "write_request",
            json!({"path": "A/B/deep", "toml": GET}),
        );
        let v = run(&s, "list_requests", json!({"depth": 1}));
        let c = &v["structuredContent"];
        assert_eq!(c["folders"], json!(["A"]), "{v}");
        assert_eq!(c["requests"].as_array().unwrap().len(), 1, "{v}");
        assert_eq!(c["requests"][0]["description"], "Lists users.", "{v}");
        let v = run(&s, "list_requests", json!({}));
        assert_eq!(
            v["structuredContent"]["folders"],
            json!(["A", "A/B"]),
            "{v}"
        );
        let v = run(&s, "list_requests", json!({"depth": 0}));
        assert_eq!(v["isError"], true, "{v}");
    }

    #[test]
    fn preview_and_import_curl_send_nothing() {
        let (_d, s) = server(true);
        let v = run(
            &s,
            "import_curl",
            json!({"command": "curl -X POST https://example.test/u -H 'X-A: 1' -d 'n=1'"}),
        );
        let toml = v["structuredContent"]["toml"].as_str().unwrap().to_string();
        assert!(toml.contains("example.test/u"), "{v}");
        let v = run(&s, "preview_request", json!({"toml": toml}));
        let curl = v["structuredContent"]["curl"].as_str().unwrap();
        assert!(curl.starts_with("curl") && curl.contains("X-A: 1"), "{v}");
        let v = run(&s, "preview_request", json!({}));
        assert_eq!(v["isError"], true, "{v}");
    }

    #[test]
    fn invalid_toml_is_a_tool_error() {
        let (_d, s) = server(false);
        let v = run(&s, "write_request", json!({"path": "x", "toml": "[http"}));
        assert!(error_text(&v).contains("not a valid request"));
    }

    #[test]
    fn max_bytes_must_be_positive() {
        assert_eq!(max_bytes_arg(&json!({})).unwrap(), MAX_BYTES);
        assert_eq!(max_bytes_arg(&json!({"max_bytes": 10})).unwrap(), 10);
        assert!(max_bytes_arg(&json!({"max_bytes": 0})).is_err());
        assert!(max_bytes_arg(&json!({"max_bytes": "x"})).is_err());
    }

    #[test]
    fn allow_env_hides_and_refuses_the_other_environments() {
        let (dir, mut s) = server(false);
        s.flags.allow_env = vec!["dev".into()];
        let envs = dir.path().join(".sendito/environments");
        std::fs::create_dir_all(&envs).unwrap();
        for name in ["dev", "prod"] {
            std::fs::write(envs.join(format!("{name}.toml")), "[variables]\n").unwrap();
        }
        let v = run(&s, "list_environments", json!({}));
        let names: Vec<&str> = v["structuredContent"]["environments"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["dev"], "{v}");
        let toml = "kind = \"http\"\nmethod = \"GET\"\nurl = \"http://127.0.0.1:9/\"\n";
        let v = run(&s, "send_request", json!({"toml": toml, "env": "prod"}));
        let text = error_text(&v);
        assert!(
            text.contains("\"prod\" is not allowed") && text.contains("only dev"),
            "{text}"
        );
    }

    #[test]
    fn environments_show_secret_names_but_not_values() {
        let (dir, s) = server(false);
        let envs = dir.path().join(".sendito/environments");
        std::fs::create_dir_all(&envs).unwrap();
        std::fs::write(
            envs.join("dev.toml"),
            "[variables]\nbase = \"http://localhost\"\ntoken = { source = \"local\" }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join(".sendito/secrets.toml"),
            "[dev]\ntoken = \"s3cret\"\n",
        )
        .unwrap();
        let v = run(&s, "list_environments", json!({}));
        let text = v.to_string();
        let vars = &v["structuredContent"]["environments"][0]["variables"];
        assert_eq!(vars[0]["value"], "http://localhost", "{v}");
        assert_eq!(vars[1]["name"], "token");
        assert_eq!(vars[1]["secret"], true);
        assert!(vars[1].get("value").is_none());
        assert!(!text.contains("s3cret"));
    }

    #[test]
    fn bodies_and_messages_are_cut_at_max_bytes() {
        let mut v = json!({});
        put_body(&mut v, "héllo".as_bytes(), 2);
        assert_eq!(v["body"], "h");
        assert_eq!(v["truncated"], true);
        assert_eq!(v["body_bytes"], 6);
        let mut v = json!({});
        put_body(&mut v, &[0xff, 0x00, 0x01], 10);
        assert_eq!(v["body_encoding"], "base64");
        assert_eq!(v["body"], "/wAB");
        assert_eq!(v["truncated"], false);
        let mut v = json!({});
        put_messages(&mut v, &["abc".into(), "defg".into(), "h".into()], 5);
        assert_eq!(v["messages"], json!(["abc", "de"]));
        assert_eq!(v["message_count"], 3);
        assert_eq!(v["truncated"], true);
    }
}
