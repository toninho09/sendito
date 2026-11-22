//! The send pipeline shared by the TUI and `sendito run`:
//! scope → pre-request script → resolve → send → extract/assert → history.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sendito_core::model::*;
use sendito_core::resolve::{
    ResolvedGrpc, ResolvedHttp, mask_request, resolve_grpc, resolve_http, resolve_mcp,
};
use sendito_core::script::{self, AssertResult, ExtractResult, RUNTIME_LAYER, ResponseView};
use sendito_core::vars::{Scope, VarValue};
use sendito_grpc::{Event, Schema};
use sendito_http::{HttpResponse, SendOptions};
use sendito_store::Collection;
use sendito_store::config::Config;
use sendito_store::env::{COLLECTION_ENV, Envs, Keychain, MemoryKeychain, OsKeychain, VarSource};
use sendito_store::history::{self, HistoryEntry};
use tokio::sync::mpsc;

use crate::viewer::Doc;

/// An opened collection with its settings.
pub struct Project {
    pub root: PathBuf,
    pub coll: Collection,
    pub envs: Arc<Envs>,
    pub config: Config,
    pub warnings: Vec<String>,
    pub name: String,
    /// Global config file (settings are written here); `None` disables writing.
    pub global_config: Option<PathBuf>,
    /// Where user themes are looked for, lowest precedence first.
    pub theme_dirs: Vec<PathBuf>,
}

/// The keychain to use: `SENDITO_KEYCHAIN=memory` selects an in-memory one (tests, demos).
pub fn default_keychain() -> Arc<dyn Keychain> {
    if std::env::var("SENDITO_KEYCHAIN").is_ok_and(|v| v == "memory") {
        Arc::new(MemoryKeychain::default())
    } else {
        Arc::new(OsKeychain)
    }
}

impl Project {
    pub fn open(root: &Path, keychain: Arc<dyn Keychain>) -> Self {
        Self::open_with(root, keychain, sendito_store::config::global_path())
    }

    /// Opens a collection with an explicit global config path (tests pass `None`).
    pub fn open_with(root: &Path, keychain: Arc<dyn Keychain>, global: Option<PathBuf>) -> Self {
        let project = sendito_store::config::project_path(root);
        let (config, warnings) = sendito_store::config::load(global.as_deref(), Some(&project));
        let name = config.collection.name.clone().unwrap_or_else(|| {
            root.canonicalize()
                .ok()
                .and_then(|p| p.file_name().map(|s| s.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "collection".into())
        });
        let coll = Collection::new(root, config.collection.ignore.clone());
        let envs = Arc::new(Envs::new(root, name.clone(), keychain));
        Self {
            root: root.to_path_buf(),
            coll,
            envs,
            config,
            warnings,
            name,
            theme_dirs: sendito_store::themes::dirs(global.as_deref(), root),
            global_config: global,
        }
    }

    pub fn send_options(&self) -> SendOptions {
        SendOptions {
            default_timeout: Duration::from_millis(self.config.http.timeout_ms.max(1)),
            max_body: (self.config.http.max_body_mb.max(1) as usize) * 1024 * 1024,
        }
    }
}

/// Variable names (and plain values) known without reading secrets; used for
/// highlighting unresolved variables before sending.
pub fn names_scope(
    envs: &Envs,
    active: Option<&str>,
    runtime: &BTreeMap<String, VarValue>,
) -> Scope {
    let mut scope = Scope::new();
    for name in [Some(COLLECTION_ENV), active].into_iter().flatten() {
        if let Ok(env) = envs.load(name) {
            let vars = env
                .vars
                .iter()
                .map(|v| {
                    let val = match &v.source {
                        VarSource::Plain(s) => VarValue::plain(s.clone()),
                        _ => VarValue::secret(""),
                    };
                    (v.name.clone(), val)
                })
                .collect();
            scope.push_layer(name, vars);
        }
    }
    scope.push_layer(RUNTIME_LAYER, runtime.clone());
    scope
}

/// Builds the full scope with secret values. May block on the keychain.
pub fn build_scope(
    envs: &Envs,
    active: Option<&str>,
    runtime: &BTreeMap<String, VarValue>,
) -> (Scope, Vec<String>) {
    let mut scope = Scope::new();
    let mut errors = Vec::new();
    for name in [Some(COLLECTION_ENV), active].into_iter().flatten() {
        match envs.load(name) {
            Ok(env) => {
                let r = envs.resolve(&env);
                for (var, e) in r.errors {
                    errors.push(format!("{{{{{var}}}}}: {e}"));
                }
                scope.push_layer(name, r.vars);
            }
            Err(e) => errors.push(e.to_string()),
        }
    }
    scope.push_layer(RUNTIME_LAYER, runtime.clone());
    (scope, errors)
}

/// Names a pre-request script will define (they count as resolved).
pub fn script_defined(req: &Request) -> Vec<String> {
    req.scripts()
        .pre
        .iter()
        .map(|s| s.var.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Unresolved variables of a request against a names-only scope.
pub fn unresolved(req: &Request, names: &Scope) -> Vec<String> {
    let mut scope = names.clone();
    for v in script_defined(req) {
        scope.set(RUNTIME_LAYER, v, VarValue::plain("x"));
    }
    sendito_core::resolve::unresolved_vars(req, &scope)
}

#[derive(Debug, Clone, Default)]
pub struct ScriptResults {
    pub pre: Vec<(String, VarValue)>,
    pub extracts: Vec<ExtractResult>,
    pub asserts: Vec<AssertResult>,
}

impl ScriptResults {
    pub fn failed_asserts(&self) -> usize {
        self.asserts.iter().filter(|a| !a.pass).count()
    }

    /// Runtime variable updates (pre-request sets and successful extractions).
    pub fn updates(&self) -> Vec<(String, VarValue)> {
        let mut v = self.pre.clone();
        for e in &self.extracts {
            if let Ok(val) = &e.value {
                v.push((e.var.clone(), val.clone()));
            }
        }
        v
    }
}

/// Everything about one finished HTTP exchange.
#[derive(Debug, Clone)]
pub struct HttpOutcome {
    pub resolved: ResolvedHttp,
    pub response: Result<HttpResponse, String>,
    /// Body prepared for viewing (the response's `body` is moved here).
    pub doc: Doc,
    /// Size of the body as received, in bytes.
    pub body_size: u64,
    pub scripts: ScriptResults,
    pub scope_errors: Vec<String>,
}

pub struct RunCtx {
    pub root: PathBuf,
    pub send: SendOptions,
    pub history: Option<(PathBuf, usize)>,
    pub name: String,
    pub path: Option<String>,
    pub env: Option<String>,
    pub schemas: SchemaCache,
    pub grpc_deadline_ms: u64,
    /// Mask secret values the server echoes back (the TUI); `sendito run`
    /// writes the body unchanged.
    pub mask_echoed_secrets: bool,
}

fn history_entry(
    ctx: &RunCtx,
    template: &Request,
    target: String,
    status: Option<i64>,
    duration_ms: u64,
    size: u64,
    error: Option<String>,
) -> HistoryEntry {
    HistoryEntry {
        ts: history::now_rfc3339(),
        name: ctx.name.clone(),
        path: ctx.path.clone(),
        env: ctx.env.clone(),
        badge: template.badge().to_string(),
        target,
        template: match template {
            Request::Http(h) => h.url.clone(),
            Request::Grpc(g) => format!("{}/{}/{}", g.target, g.service, g.method),
            Request::Mcp(m) => format!("{}/{}", m.server, m.tool),
        },
        request: sendito_store::request_file::to_toml(template),
        status,
        duration_ms,
        size,
        error,
    }
}

fn write_history(ctx: &RunCtx, entry: &HistoryEntry) {
    if let Some((path, max)) = &ctx.history {
        let _ = history::append(path, entry, *max);
    }
}

/// Secret values to mask in what the server sends back: every secret variable
/// and, when a Basic password is secret, the encoded credentials.
fn http_secrets(req: &HttpRequest, scope: &Scope) -> Vec<String> {
    let mut secrets = scope.secret_values();
    if let sendito_core::Auth::Basic { username, password } = &req.auth {
        let (u, p) = (scope.render(username), scope.render(password));
        if u.secret || p.secret {
            use base64::Engine as _;
            secrets.push(
                base64::engine::general_purpose::STANDARD.encode(format!("{}:{}", u.text, p.text)),
            );
        }
    }
    secrets
}

/// Masks secret values in an event shown to the user.
fn scrub_event(ev: Event, secrets: &[String]) -> Event {
    use sendito_core::vars::scrub;
    let kv = |m: Vec<(String, String)>| -> Vec<(String, String)> {
        m.into_iter()
            .map(|(k, v)| (k, scrub(&v, secrets)))
            .collect()
    };
    match ev {
        Event::Sent { json, at, wall } => Event::Sent {
            json: scrub(&json, secrets),
            at,
            wall,
        },
        Event::Headers { metadata, at } => Event::Headers {
            metadata: kv(metadata),
            at,
        },
        Event::Received {
            json,
            full,
            compact,
            size,
            at,
            wall,
        } => Event::Received {
            json: scrub(&json, secrets),
            full: scrub(&full, secrets),
            compact: scrub(&compact, secrets),
            size,
            at,
            wall,
        },
        Event::Finished {
            code,
            message,
            metadata,
            at,
        } => Event::Finished {
            code,
            message: scrub(&message, secrets),
            metadata: kv(metadata),
            at,
        },
        other => other,
    }
}

/// Runs an HTTP request through the full pipeline.
pub async fn exec_http(
    ctx: &RunCtx,
    req: &HttpRequest,
    mut scope: Scope,
    scope_errors: Vec<String>,
) -> HttpOutcome {
    let mut results = ScriptResults {
        pre: script::run_pre(&req.scripts.pre, &mut scope),
        ..Default::default()
    };
    let resolved = resolve_http(req, &scope, &ctx.root);
    let template = Request::Http(req.clone());
    let target = match mask_request(&template, &scope) {
        Request::Http(h) => sendito_core::resolve::normalize_url(&h.url),
        _ => String::new(),
    };
    let start = std::time::Instant::now();
    let sent = sendito_http::send(&resolved, &ctx.send).await;
    let mut body_size = 0;
    let (response, doc) = match sent {
        Ok(mut resp) => {
            let body = std::mem::take(&mut resp.body);
            let view = ResponseView {
                status: resp.status,
                headers: &resp.headers,
                body: &body,
            };
            results.extracts = script::run_extract(&req.scripts.extract, &view, &mut scope);
            results.asserts = script::run_asserts(&req.scripts.asserts, &view, &scope);
            // Secrets echoed back by the server are masked before display.
            let secrets = if ctx.mask_echoed_secrets {
                http_secrets(req, &scope)
            } else {
                Vec::new()
            };
            for (_, v) in resp.headers.iter_mut() {
                *v = sendito_core::vars::scrub(v, &secrets);
            }
            let ct = resp.header("content-type").map(str::to_string);
            let size = body.len() as u64;
            body_size = size;
            let doc = tokio::task::spawn_blocking(move || {
                let body = sendito_core::vars::scrub_bytes(&body, &secrets).unwrap_or(body);
                Doc::from_bytes(body, ct.as_deref())
            })
            .await
            .unwrap_or_default();
            let entry = history_entry(
                ctx,
                &template,
                target,
                Some(resp.status as i64),
                resp.timing.total.as_millis() as u64,
                size,
                None,
            );
            write_history(ctx, &entry);
            (Ok(resp), doc)
        }
        Err(e) => {
            let msg = sendito_core::vars::scrub(&e.to_string(), &http_secrets(req, &scope));
            let entry = history_entry(
                ctx,
                &template,
                target,
                None,
                start.elapsed().as_millis() as u64,
                0,
                Some(msg.clone()),
            );
            write_history(ctx, &entry);
            (Err(msg), Doc::default())
        }
    };
    HttpOutcome {
        resolved,
        response,
        doc,
        body_size,
        scripts: results,
        scope_errors,
    }
}

/// Cache of loaded schemas keyed by target and schema source.
pub type SchemaCache = Arc<Mutex<HashMap<String, Schema>>>;

pub fn schema_key(uri: &str, src: &SchemaSource) -> String {
    match src {
        SchemaSource::Reflection => format!("reflect:{uri}"),
        SchemaSource::Files {
            files,
            import_paths,
        } => format!("files:{}|{}", files.join(","), import_paths.join(",")),
    }
}

/// Loads (or fetches from the cache) the schema for a request.
pub async fn load_schema(
    root: &Path,
    resolved: &ResolvedGrpc,
    src: &SchemaSource,
    cache: &SchemaCache,
    refresh: bool,
) -> Result<Schema, String> {
    let key = schema_key(&resolved.uri, src);
    if !refresh && let Some(s) = cache.lock().ok().and_then(|c| c.get(&key).cloned()) {
        return Ok(s);
    }
    let schema = match src {
        SchemaSource::Files {
            files,
            import_paths,
        } => {
            let (files, imports, root) = (files.clone(), import_paths.clone(), root.to_path_buf());
            tokio::task::spawn_blocking(move || Schema::from_files(&files, &imports, &root))
                .await
                .map_err(|e| e.to_string())?
                .map_err(|e| e.to_string())?
        }
        SchemaSource::Reflection => {
            let ch = sendito_grpc::connect(&conn_settings(resolved))
                .await
                .map_err(|e| e.to_string())?;
            sendito_grpc::schema::from_reflection(ch)
                .await
                .map_err(|e| e.to_string())?
        }
    };
    if let Ok(mut c) = cache.lock() {
        c.insert(key, schema.clone());
    }
    Ok(schema)
}

pub fn conn_settings(r: &ResolvedGrpc) -> sendito_grpc::ConnSettings {
    sendito_grpc::ConnSettings {
        uri: r.uri.clone(),
        verify_tls: r.verify_tls,
        connect_timeout: Duration::from_secs(10),
    }
}

/// Commands from the UI to a running gRPC call.
#[derive(Debug, Clone)]
pub enum GrpcControl {
    /// JSON for one message, or an array of messages.
    Send(String),
    EndStream,
    Cancel,
}

/// Progress of a gRPC call, forwarded to the UI as it happens.
#[derive(Debug, Clone)]
pub enum GrpcUpdate {
    Started {
        resolved: ResolvedGrpc,
        kind: sendito_grpc::CallKind,
    },
    Event(Event),
    Error(String),
    Done {
        scripts: ScriptResults,
        scope_errors: Vec<String>,
    },
}

fn last_message(events: &[Event]) -> (Vec<u8>, Vec<(String, String)>, i32) {
    let mut body = Vec::new();
    let mut headers = Vec::new();
    let mut code = -1;
    for e in events {
        match e {
            Event::Received { full, .. } => body = full.as_bytes().to_vec(),
            Event::Headers { metadata, .. } => headers = metadata.clone(),
            Event::Finished {
                code: c, metadata, ..
            } => {
                code = *c;
                headers.extend(metadata.iter().cloned());
            }
            _ => {}
        }
    }
    (body, headers, code)
}

/// Runs a gRPC call. Updates go to `updates`; `control` drives streaming input.
pub async fn exec_grpc(
    ctx: &RunCtx,
    req: &GrpcRequest,
    mut scope: Scope,
    scope_errors: Vec<String>,
    updates: mpsc::UnboundedSender<GrpcUpdate>,
    mut control: mpsc::UnboundedReceiver<GrpcControl>,
) {
    let mut results = ScriptResults {
        pre: script::run_pre(&req.scripts.pre, &mut scope),
        ..Default::default()
    };
    let mut resolved = resolve_grpc(req, &scope);
    if resolved.deadline_ms.is_none() && ctx.grpc_deadline_ms > 0 {
        resolved.deadline_ms = Some(ctx.grpc_deadline_ms);
    }
    let template = Request::Grpc(req.clone());
    let target = match mask_request(&template, &scope) {
        Request::Grpc(g) => format!(
            "{}/{}/{}",
            sendito_core::resolve::resolve_grpc(&g, &Scope::new()).uri,
            resolved.service,
            resolved.method
        ),
        _ => String::new(),
    };
    let fail = |msg: String, results: ScriptResults| {
        let entry = history_entry(
            ctx,
            &template,
            target.clone(),
            None,
            0,
            0,
            Some(msg.clone()),
        );
        write_history(ctx, &entry);
        let _ = updates.send(GrpcUpdate::Error(msg));
        let _ = updates.send(GrpcUpdate::Done {
            scripts: results,
            scope_errors: scope_errors.clone(),
        });
    };
    if resolved.service.is_empty() || resolved.method.is_empty() {
        return fail("choose a service and method first".into(), results);
    }
    let schema = match load_schema(&ctx.root, &resolved, &req.schema, &ctx.schemas, false).await {
        Ok(s) => s,
        Err(e) => return fail(e, results),
    };
    let method = match schema.method(&resolved.service, &resolved.method) {
        Ok(m) => m,
        Err(e) => return fail(e.to_string(), results),
    };
    let kind = sendito_grpc::CallKind::of(&method);
    let msgs = match sendito_grpc::schema::parse_messages(
        &method.input(),
        &resolved.message,
        kind.client_streams(),
    ) {
        Ok(m) => m,
        Err(e) => return fail(e, results),
    };
    let metadata = match sendito_grpc::metadata_map(&resolved.metadata) {
        Ok(m) => m,
        Err(e) => return fail(e.to_string(), results),
    };
    let channel = match sendito_grpc::connect(&conn_settings(&resolved)).await {
        Ok(c) => c,
        Err(e) => return fail(e.to_string(), results),
    };
    let _ = updates.send(GrpcUpdate::Started {
        resolved: resolved.clone(),
        kind,
    });
    let deadline = resolved.deadline_ms.map(Duration::from_millis);
    let secrets = scope.secret_values();
    let mut call = sendito_grpc::start_call(channel, &method, metadata, deadline, msgs);
    let mut events: Vec<Event> = Vec::new();
    let mut control_open = true;
    loop {
        tokio::select! {
            ev = call.events.recv() => {
                let Some(ev) = ev else { break };
                let done = matches!(ev, Event::Finished { .. });
                events.push(ev.clone());
                let ev = if ctx.mask_echoed_secrets { scrub_event(ev, &secrets) } else { ev };
                let _ = updates.send(GrpcUpdate::Event(ev));
                if done { break; }
            }
            cmd = control.recv(), if control_open => {
                match cmd {
                    Some(GrpcControl::Send(json)) => {
                        let rendered = scope.render(&json).text;
                        match sendito_grpc::schema::parse_messages(&method.input(), &rendered, true) {
                            Ok(m) => {
                                if let Err(e) = call.send(m) {
                                    let _ = updates.send(GrpcUpdate::Error(e.to_string()));
                                }
                            }
                            Err(e) => { let _ = updates.send(GrpcUpdate::Error(e)); }
                        }
                    }
                    Some(GrpcControl::EndStream) => call.close_send(),
                    Some(GrpcControl::Cancel) => call.cancel(),
                    // The controller went away (headless runs): finish the request side.
                    None => { control_open = false; call.close_send(); }
                }
            }
        }
    }
    let (body, headers, code) = last_message(&events);
    let view = ResponseView {
        status: code.max(0) as u16,
        headers: &headers,
        body: &body,
    };
    results.extracts = script::run_extract(&req.scripts.extract, &view, &mut scope);
    results.asserts = script::run_asserts(&req.scripts.asserts, &view, &scope);
    let total = events
        .iter()
        .rev()
        .find_map(|e| match e {
            Event::Finished { at, .. } => Some(at.as_millis() as u64),
            _ => None,
        })
        .unwrap_or(0);
    let size: u64 = events
        .iter()
        .map(|e| match e {
            Event::Received { size, .. } => *size as u64,
            _ => 0,
        })
        .sum();
    let err = if code > 0 {
        Some(sendito_grpc::code_name(code).to_string())
    } else {
        None
    };
    let entry = history_entry(ctx, &template, target, Some(code as i64), total, size, err);
    write_history(ctx, &entry);
    let _ = updates.send(GrpcUpdate::Done {
        scripts: results,
        scope_errors,
    });
}

// ----- MCP -----------------------------------------------------------

/// Re-export so the UI can name the event type without depending on the crate.
pub use sendito_mcp::Event as McpEvent;

/// MCP run settings taken from the project: the `[mcp]` config section, the
/// keychain (OAuth tokens) and the collection name (keychain accounts).
pub struct McpRunInfo {
    pub timeout_ms: u64,
    pub servers: BTreeMap<String, sendito_store::config::McpServer>,
    pub keychain: Arc<dyn Keychain>,
    pub collection: String,
}

impl McpRunInfo {
    pub fn of(p: &Project) -> Self {
        Self {
            timeout_ms: p.config.mcp.timeout_ms,
            servers: p.config.mcp.servers.clone(),
            keychain: p.envs.keychain.clone(),
            collection: p.name.clone(),
        }
    }
}

/// The keychain account an MCP server's tokens live under.
pub fn mcp_token_account(collection: &str, server: &str) -> String {
    format!("{collection}/mcp/{server}")
}

/// Bridges the store's keychain to the MCP crate's token store.
struct KeychainStore(Arc<dyn Keychain>);

impl sendito_mcp::oauth::TokenStore for KeychainStore {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        self.0.get(account)
    }
    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        self.0.set(account, value)
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        self.0.delete(account)
    }
}

/// Builds the transport for a server, rendering `{{variables}}` in its
/// command, arguments, environment, URL and headers.
pub fn mcp_transport(
    info: &McpRunInfo,
    server: &str,
    scope: &Scope,
) -> Result<sendito_mcp::Transport, String> {
    let r = |s: &str| scope.render(s).text;
    let cfg = info.servers.get(server).ok_or_else(|| {
        format!("server {server:?} is not defined in [mcp.servers] of the config")
    })?;
    Ok(match cfg {
        sendito_store::config::McpServer::Stdio(s) => sendito_mcp::Transport::Stdio {
            command: r(&s.command),
            args: s.args.iter().map(|a| r(a)).collect(),
            env: s.env.iter().map(|(k, v)| (k.clone(), r(v))).collect(),
        },
        sendito_store::config::McpServer::Http(h) => {
            let mut headers: Vec<(String, String)> =
                h.headers.iter().map(|(k, v)| (k.clone(), r(v))).collect();
            let auth = match &h.auth {
                Some(sendito_store::config::McpServerAuth::Bearer { token }) => {
                    headers.push(("Authorization".into(), format!("Bearer {}", r(token))));
                    None
                }
                Some(sendito_store::config::McpServerAuth::Oauth {
                    client_id,
                    client_secret,
                    authorize_url,
                    token_url,
                    registration_url,
                    scopes,
                }) => Some(Arc::new(sendito_mcp::RefreshingProvider::new(
                    Arc::new(KeychainStore(info.keychain.clone())),
                    mcp_token_account(&info.collection, server),
                    r(&h.url),
                    sendito_mcp::oauth::OAuthSettings {
                        client_id: client_id.clone(),
                        client_secret: client_secret.clone(),
                        authorize_url: authorize_url.clone(),
                        token_url: token_url.clone(),
                        registration_url: registration_url.clone(),
                        scopes: scopes.clone(),
                    },
                )) as Arc<dyn sendito_mcp::TokenProvider>),
                None => None,
            };
            sendito_mcp::Transport::Http {
                url: r(&h.url),
                headers,
                verify_tls: h.verify_tls,
                auth,
            }
        }
    })
}

/// Progress of an MCP call, forwarded to the UI as it happens.
#[derive(Debug, Clone)]
pub enum McpUpdate {
    Event(sendito_mcp::Event),
    Done {
        error: Option<String>,
        /// The tool answered `isError: true` (a result, not a failure).
        is_error: bool,
        /// Pretty JSON of the whole `tools/call` result, for the body viewer
        /// and for scripts (JSONPath over `$.content`, `$.structuredContent`).
        result: Option<Vec<u8>>,
        scripts: ScriptResults,
        scope_errors: Vec<String>,
    },
}

fn scrub_mcp_event(ev: sendito_mcp::Event, secrets: &[String]) -> sendito_mcp::Event {
    use sendito_core::vars::scrub;
    match ev {
        sendito_mcp::Event::Sent { json, at, wall } => sendito_mcp::Event::Sent {
            json: scrub(&json, secrets),
            at,
            wall,
        },
        sendito_mcp::Event::Received { json, at, wall } => sendito_mcp::Event::Received {
            json: scrub(&json, secrets),
            at,
            wall,
        },
        sendito_mcp::Event::Log { line, at } => sendito_mcp::Event::Log {
            line: scrub(&line, secrets),
            at,
        },
        sendito_mcp::Event::Finished { ok, message, at } => sendito_mcp::Event::Finished {
            ok,
            message: scrub(&message, secrets),
            at,
        },
    }
}

/// Runs one MCP call end to end. `0` is recorded as the status on success,
/// `1` when the tool answers `isError`.
pub async fn exec_mcp(
    ctx: &RunCtx,
    req: &McpRequest,
    mut scope: Scope,
    scope_errors: Vec<String>,
    info: &McpRunInfo,
    updates: mpsc::UnboundedSender<McpUpdate>,
) {
    let mut results = ScriptResults {
        pre: script::run_pre(&req.scripts.pre, &mut scope),
        ..Default::default()
    };
    let resolved = resolve_mcp(req, &scope);
    let template = Request::Mcp(req.clone());
    let target = match mask_request(&template, &scope) {
        Request::Mcp(m) => format!("{}/{}", m.server, m.tool),
        _ => String::new(),
    };
    let secrets = scope.secret_values();
    let started = std::time::Instant::now();
    let fail = |msg: String, results: ScriptResults| {
        let msg = sendito_core::vars::scrub(&msg, &secrets);
        let entry = history_entry(
            ctx,
            &template,
            target.clone(),
            None,
            started.elapsed().as_millis() as u64,
            0,
            Some(msg.clone()),
        );
        write_history(ctx, &entry);
        let _ = updates.send(McpUpdate::Done {
            error: Some(msg),
            is_error: false,
            result: None,
            scripts: results,
            scope_errors: scope_errors.clone(),
        });
    };
    if resolved.server.is_empty() || resolved.tool.is_empty() {
        return fail("choose a server and tool first".into(), results);
    }
    let transport = match mcp_transport(info, &resolved.server, &scope) {
        Ok(t) => t,
        Err(e) => return fail(e, results),
    };
    let timeout = resolved.timeout_ms.unwrap_or(info.timeout_ms);
    let opts = sendito_mcp::Options {
        timeout: Duration::from_millis(timeout.max(1)),
    };
    let (etx, mut erx) = mpsc::unbounded_channel();
    let call = sendito_mcp::call_tool(&transport, &resolved.tool, &resolved.arguments, &opts, etx);
    let forward = async {
        while let Some(ev) = erx.recv().await {
            let scr = if ctx.mask_echoed_secrets {
                scrub_mcp_event(ev, &secrets)
            } else {
                ev
            };
            let _ = updates.send(McpUpdate::Event(scr));
        }
    };
    let (outcome, _) = tokio::join!(call, forward);
    match outcome {
        Ok(r) => {
            let pretty = serde_json::to_string_pretty(&r.raw).unwrap_or_else(|_| "{}".into());
            let body = pretty.into_bytes();
            let view = ResponseView {
                status: u16::from(r.is_error),
                headers: &[],
                body: &body,
            };
            results.extracts = script::run_extract(&req.scripts.extract, &view, &mut scope);
            results.asserts = script::run_asserts(&req.scripts.asserts, &view, &scope);
            let size = body.len() as u64;
            let entry = history_entry(
                ctx,
                &template,
                target,
                Some(i64::from(r.is_error)),
                started.elapsed().as_millis() as u64,
                size,
                None,
            );
            write_history(ctx, &entry);
            let body = if ctx.mask_echoed_secrets {
                sendito_core::vars::scrub_bytes(&body, &secrets).unwrap_or(body)
            } else {
                body
            };
            let _ = updates.send(McpUpdate::Done {
                error: None,
                is_error: r.is_error,
                result: Some(body),
                scripts: results,
                scope_errors,
            });
        }
        Err(e) => fail(e.to_string(), results),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_results() {
        let r = ScriptResults {
            pre: vec![("a".into(), VarValue::plain("1"))],
            extracts: vec![
                ExtractResult {
                    var: "b".into(),
                    value: Ok(VarValue::plain("2")),
                },
                ExtractResult {
                    var: "c".into(),
                    value: Err("x".into()),
                },
            ],
            asserts: vec![AssertResult {
                label: "x".into(),
                pass: false,
                actual: "y".into(),
            }],
        };
        assert_eq!(r.updates().len(), 2);
        assert_eq!(r.failed_asserts(), 1);
        assert_eq!(schema_key("u", &SchemaSource::Reflection), "reflect:u");
        let (b, h, c) = last_message(&[
            Event::Headers {
                metadata: vec![("a".into(), "1".into())],
                at: Duration::ZERO,
            },
            Event::Received {
                json: "{}".into(),
                full: "{}".into(),
                compact: "{}".into(),
                size: 0,
                at: Duration::ZERO,
                wall: std::time::SystemTime::now(),
            },
            Event::Finished {
                code: 0,
                message: String::new(),
                metadata: vec![],
                at: Duration::ZERO,
            },
        ]);
        assert_eq!(b, b"{}");
        assert_eq!(h.len(), 1);
        assert_eq!(c, 0);
    }
}
