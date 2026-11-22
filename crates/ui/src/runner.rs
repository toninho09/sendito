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
    /// `[http] tls` with paths resolved against the collection root.
    pub tls: sendito_http::TlsMaterial,
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
        let (config, mut warnings) = sendito_store::config::load(global.as_deref(), Some(&project));
        if let Some(p) = &config.http.proxy
            && let Err(e) = sendito_http::ProxySpec::parse(p)
        {
            warnings.push(e);
        }
        let tls = {
            let t = &config.http.tls;
            let m = sendito_http::TlsMaterial {
                ca: t.ca.as_deref().map(|p| sendito_store::env::expand(root, p)),
                client_cert: t
                    .client_cert
                    .as_deref()
                    .map(|p| sendito_store::env::expand(root, p)),
                client_key: t
                    .client_key
                    .as_deref()
                    .map(|p| sendito_store::env::expand(root, p)),
            };
            if let Some(e) = m.pair_error() {
                warnings.push(e);
            }
            m
        };
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
            tls,
        }
    }

    pub fn send_options(&self) -> SendOptions {
        SendOptions {
            default_timeout: Duration::from_millis(self.config.http.timeout_ms.max(1)),
            max_body: (self.config.http.max_body_mb.max(1) as usize) * 1024 * 1024,
            proxy: self
                .config
                .http
                .proxy
                .as_deref()
                .and_then(|p| sendito_http::ProxySpec::parse(p).ok()),
            tls: self.tls.clone(),
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
    /// `[websocket] timeout_ms`: the default handshake timeout (0 = none).
    pub ws_timeout_ms: u64,
    /// Mask secret values the server echoes back (the TUI); `sendito run`
    /// writes the body unchanged.
    pub mask_echoed_secrets: bool,
    /// `[http] cookie_jar` config default; the request's `[options] cookies`
    /// overrides it.
    pub cookie_jar: bool,
    pub oauth: Option<OAuthRunInfo>,
}

#[derive(Clone)]
pub struct OAuthRunInfo {
    pub identities: BTreeMap<String, sendito_store::config::OAuthIdentity>,
    pub keychain: Arc<dyn Keychain>,
    pub collection: String,
}

impl OAuthRunInfo {
    pub fn of(project: &Project) -> Self {
        Self {
            identities: project.config.oauth.clone(),
            keychain: project.envs.keychain.clone(),
            collection: project.name.clone(),
        }
    }
}

/// Separate tokens per collection, active environment and identity.
pub fn oauth_token_account(collection: &str, env: Option<&str>, identity: &str) -> String {
    let env = env.unwrap_or(COLLECTION_ENV);
    format!(
        "http-oauth/{}/{collection}/{}/{env}/{}/{identity}",
        collection.len(),
        env.len(),
        identity.len()
    )
}

/// Resolve configured OAuth fields without allowing secrets in endpoint URLs.
pub fn oauth_settings(
    identity: &sendito_store::config::OAuthIdentity,
    scope: &Scope,
) -> Result<sendito_mcp::oauth::OAuthSettings, String> {
    let render = |text: &str, field: &str, allow_secret: bool| {
        let r = scope.render(text);
        if !r.unresolved.is_empty() || r.text.contains("{{") {
            return Err(format!("OAuth {field} contains unresolved variables"));
        }
        if r.secret && !allow_secret {
            return Err(format!("OAuth {field} must not contain secret variables"));
        }
        Ok(r.text)
    };
    Ok(sendito_mcp::oauth::OAuthSettings {
        client_id: Some(render(&identity.client_id, "client_id", false)?),
        client_secret: identity
            .client_secret
            .as_deref()
            .map(|s| render(s, "client_secret", true))
            .transpose()?,
        authorize_url: Some(render(&identity.authorize_url, "authorize_url", false)?),
        token_url: Some(render(&identity.token_url, "token_url", false)?),
        scopes: identity
            .scopes
            .iter()
            .map(|s| render(s, "scope", false))
            .collect::<Result<_, _>>()?,
        registration_url: None,
    })
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
            Request::Ws(w) => w.url.clone(),
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

fn http_oauth_provider(
    ctx: &RunCtx,
    name: &str,
    scope: &Scope,
) -> Result<sendito_mcp::oauth::RefreshingProvider, String> {
    let rendered = scope.render(name);
    if rendered.secret
        || !rendered.unresolved.is_empty()
        || rendered.text.contains("{{")
        || rendered.text.trim().is_empty()
    {
        return Err("OAuth identity must be a non-secret, resolved name".into());
    }
    let info = ctx.oauth.as_ref().ok_or("OAuth is not configured")?;
    let identity = info
        .identities
        .get(&rendered.text)
        .ok_or("OAuth identity is not defined in [oauth]")?;
    let settings = oauth_settings(identity, scope)?;
    Ok(sendito_mcp::oauth::RefreshingProvider::new_http(
        Arc::new(KeychainStore(info.keychain.clone())),
        oauth_token_account(&info.collection, ctx.env.as_deref(), &rendered.text),
        settings,
    ))
}

/// Reads the jar's matching `Cookie` header off the UI thread.
async fn jar_request(
    root: &Path,
    env: &str,
    url: &str,
) -> Result<(Option<String>, Vec<String>), String> {
    let (root, env, url) = (root.to_path_buf(), env.to_string(), url.to_string());
    match tokio::task::spawn_blocking(move || sendito_store::cookies::request(&root, &env, &url))
        .await
    {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => Err(format!("cookie jar: {e}")),
        Err(_) => Err("cookie jar task failed".into()),
    }
}

/// Stores a response's Set-Cookie headers off the UI thread.
async fn jar_store(root: &Path, env: &str, url: &str, sets: &[String]) -> Result<(), String> {
    let (root, env, url, sets) = (
        root.to_path_buf(),
        env.to_string(),
        url.to_string(),
        sets.to_vec(),
    );
    match tokio::task::spawn_blocking(move || {
        sendito_store::cookies::store_response(&root, &env, &url, &sets)
    })
    .await
    {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(format!("cookie jar: {e}")),
        Err(_) => Err("cookie jar task failed".into()),
    }
}

fn set_cookie_headers(headers: &[(String, String)]) -> Vec<String> {
    headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, v)| v.clone())
        .collect()
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
    let mut resolved = resolve_http(req, &scope, &ctx.root);
    let cookies_on = req.options.cookies.unwrap_or(ctx.cookie_jar);
    let jar_env = ctx.env.as_deref().unwrap_or(COLLECTION_ENV);
    let mut cookie_secrets = Vec::new();
    // Index of the jar's Cookie header in `resolved.headers`, kept so the
    // OAuth retry can replay an updated jar.
    let mut jar_cookie: Option<usize> = None;
    let cookie_setup = if cookies_on {
        match jar_request(&ctx.root, jar_env, &resolved.url).await {
            Ok((header, secrets)) => {
                cookie_secrets = secrets;
                if let Some(header) = header {
                    resolved.headers.push(("Cookie".into(), header));
                    resolved
                        .header_origins
                        .push(sendito_core::resolve::HeaderOrigin::User);
                    jar_cookie = Some(resolved.headers.len() - 1);
                }
                // Reqwest would forward the initial Cookie header unchanged on
                // same-host redirects, even when the cookie path no longer matches.
                if resolved.follow_redirects {
                    resolved.follow_redirects = false;
                    resolved
                        .notes
                        .push("redirects are not followed when the cookie jar is enabled".into());
                }
                Ok(())
            }
            Err(e) => Err(sendito_http::HttpError::Send(e)),
        }
    } else {
        Ok(())
    };
    let template = Request::Http(req.clone());
    let target = match mask_request(&template, &scope) {
        Request::Http(h) => sendito_core::resolve::normalize_url(&h.url),
        _ => String::new(),
    };
    let start = std::time::Instant::now();
    let mut provider = None;
    let auth_setup = match (cookie_setup.is_ok(), &req.auth) {
        (true, Auth::OAuth { .. })
            if resolved
                .headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("authorization")) =>
        {
            resolved
                .notes
                .push("OAuth not added: the request explicitly defines Authorization".into());
            Ok(())
        }
        (true, Auth::OAuth { name }) => {
            use sendito_mcp::TokenProvider;
            match sendito_mcp::oauth::validate_oauth_url(&resolved.url)
                .map_err(|_| "OAuth requests require HTTPS except on loopback".to_string())
                .and_then(|_| http_oauth_provider(ctx, name, &scope))
            {
                Ok(p) => match p.token(false).await {
                    Ok(token) => {
                        cookie_secrets.push(token.clone());
                        resolved.headers.push(("Authorization".into(), format!("Bearer {token}")));
                        resolved.header_origins.push(sendito_core::resolve::HeaderOrigin::Auth);
                        provider = Some(p);
                        Ok(())
                    }
                    Err(_) => Err(sendito_http::HttpError::Send("OAuth token unavailable; run sendito auth login for this identity and environment".into())),
                },
                Err(e) => Err(sendito_http::HttpError::Send(e)),
            }
        }
        _ => Ok(()),
    };
    let mut sent = match cookie_setup.and(auth_setup) {
        Ok(()) => sendito_http::send(&resolved, &ctx.send).await,
        Err(e) => Err(e),
    };
    if sent.as_ref().is_ok_and(|r| {
        r.status == 401
            && url::Url::parse(&resolved.url)
                .ok()
                .zip(url::Url::parse(&r.url).ok())
                .is_some_and(|(original, final_url)| original.origin() == final_url.origin())
    }) && let Some(provider) = provider
    {
        use sendito_mcp::TokenProvider;
        match provider.token(true).await {
            Ok(token) => {
                cookie_secrets.push(token.clone());
                if let Some((_, value)) = resolved
                    .headers
                    .iter_mut()
                    .find(|(n, _)| n.eq_ignore_ascii_case("authorization"))
                {
                    *value = format!("Bearer {token}");
                }
                // The 401 may have set cookies: store them and replay the
                // updated jar on the retry instead of the first send's header.
                if cookies_on && let Ok(first) = &sent {
                    let sets = set_cookie_headers(&first.headers);
                    if !sets.is_empty() {
                        cookie_secrets.extend(sendito_store::cookies::response_secrets(&sets));
                        if let Err(e) = jar_store(&ctx.root, jar_env, &first.url, &sets).await {
                            resolved.notes.push(format!("cookies not stored: {e}"));
                        }
                    }
                    match jar_request(&ctx.root, jar_env, &resolved.url).await {
                        Ok((header, secrets)) => {
                            cookie_secrets.extend(secrets);
                            match (jar_cookie, header) {
                                (Some(i), Some(h)) => resolved.headers[i].1 = h,
                                (Some(i), None) => {
                                    resolved.headers.remove(i);
                                    resolved.header_origins.remove(i);
                                }
                                (None, Some(h)) => {
                                    resolved.headers.push(("Cookie".into(), h));
                                    resolved
                                        .header_origins
                                        .push(sendito_core::resolve::HeaderOrigin::User);
                                }
                                (None, None) => {}
                            }
                        }
                        Err(e) => resolved.notes.push(format!("cookie jar: {e}")),
                    }
                }
                sent = sendito_http::send(&resolved, &ctx.send).await;
            }
            Err(_) => {
                sent = Err(sendito_http::HttpError::Send(
                    "OAuth refresh failed; sign in again".into(),
                ))
            }
        }
    };
    if cookies_on && let Ok(resp) = &sent {
        let sets = set_cookie_headers(&resp.headers);
        cookie_secrets.extend(sendito_store::cookies::response_secrets(&sets));
        if !sets.is_empty()
            && let Err(e) = jar_store(&ctx.root, jar_env, &resp.url, &sets).await
        {
            resolved.notes.push(format!("cookies not stored: {e}"));
        }
    }
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
            // Cookie values and OAuth access tokens remain secret even if an
            // extraction rule did not mark them; runtime variables may persist.
            for extract in &mut results.extracts {
                if let Ok(value) = &mut extract.value
                    && cookie_secrets
                        .iter()
                        .any(|s| !s.is_empty() && value.value.contains(s))
                {
                    value.secret = true;
                    scope.set(RUNTIME_LAYER, &extract.var, value.clone());
                }
            }
            results.asserts = script::run_asserts(&req.scripts.asserts, &view, &scope);
            for assertion in &mut results.asserts {
                assertion.actual = sendito_core::vars::scrub(&assertion.actual, &cookie_secrets);
                assertion.label = sendito_core::vars::scrub(&assertion.label, &cookie_secrets);
            }
            // Secrets echoed back by the server are masked before display.
            let secrets = if ctx.mask_echoed_secrets {
                let mut secrets = http_secrets(req, &scope);
                secrets.extend(cookie_secrets);
                secrets
            } else {
                Vec::new()
            };
            for (name, value) in resp.headers.iter_mut() {
                if ctx.mask_echoed_secrets && name.eq_ignore_ascii_case("set-cookie") {
                    *value = "••••••".into();
                } else {
                    *value = sendito_core::vars::scrub(value, &secrets);
                }
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
            let mut secrets = http_secrets(req, &scope);
            secrets.extend(cookie_secrets);
            let msg = sendito_core::vars::scrub(&e.to_string(), &secrets);
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
    tls: &sendito_http::TlsMaterial,
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
            let ch = sendito_grpc::connect(&conn_settings(resolved, tls))
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

pub fn conn_settings(
    r: &ResolvedGrpc,
    tls: &sendito_http::TlsMaterial,
) -> sendito_grpc::ConnSettings {
    sendito_grpc::ConnSettings {
        uri: r.uri.clone(),
        verify_tls: r.verify_tls,
        connect_timeout: Duration::from_secs(10),
        tls: tls.clone(),
    }
}

/// Commands to a running request: gRPC calls and WebSocket sessions share
/// the channel type so the UI can hold one sender per tab.
#[derive(Debug, Clone)]
pub enum Control {
    Grpc(GrpcControl),
    Ws(WsControl),
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
    mut control: mpsc::UnboundedReceiver<Control>,
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
    if matches!(req.auth, Auth::OAuth { .. }) {
        return fail(
            "OAuth 2 authentication is only supported for HTTP requests".into(),
            results,
        );
    }
    if resolved.service.is_empty() || resolved.method.is_empty() {
        return fail("choose a service and method first".into(), results);
    }
    let schema = match load_schema(
        &ctx.root,
        &resolved,
        &req.schema,
        &ctx.schemas,
        false,
        &ctx.send.tls,
    )
    .await
    {
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
    let channel = match sendito_grpc::connect(&conn_settings(&resolved, &ctx.send.tls)).await {
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
                let cmd = cmd.and_then(|c| match c {
                    Control::Grpc(g) => Some(g),
                    Control::Ws(_) => None,
                });
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

// ----- WebSocket -----------------------------------------------------

/// Commands from the UI to a running WebSocket session.
#[derive(Debug, Clone)]
pub enum WsControl {
    /// One text frame.
    Text(String),
    /// One binary frame (raw bytes).
    Binary(Vec<u8>),
    /// Start the close handshake (1000).
    EndStream,
    /// Abort without a close handshake.
    Cancel,
}

/// Progress of a WebSocket session, forwarded to the UI as it happens.
#[derive(Debug, Clone)]
pub enum WsUpdate {
    Started {
        resolved: sendito_core::resolve::ResolvedWs,
    },
    Event(sendito_ws::Event),
    Error(String),
    Done {
        scripts: ScriptResults,
        scope_errors: Vec<String>,
    },
}

fn scrub_ws_event(ev: sendito_ws::Event, secrets: &[String]) -> sendito_ws::Event {
    use sendito_core::vars::scrub;
    let kv = |m: Vec<(String, String)>| -> Vec<(String, String)> {
        m.into_iter()
            .map(|(k, v)| (k, scrub(&v, secrets)))
            .collect()
    };
    match ev {
        sendito_ws::Event::Handshake {
            status,
            headers,
            subprotocol,
            at,
            wall,
        } => sendito_ws::Event::Handshake {
            status,
            headers: kv(headers),
            subprotocol,
            at,
            wall,
        },
        sendito_ws::Event::Sent {
            kind,
            preview,
            size,
            at,
            wall,
        } => sendito_ws::Event::Sent {
            kind,
            preview: scrub(&preview, secrets),
            size,
            at,
            wall,
        },
        sendito_ws::Event::Received {
            kind,
            preview,
            size,
            at,
            wall,
        } => sendito_ws::Event::Received {
            kind,
            preview: scrub(&preview, secrets),
            size,
            at,
            wall,
        },
        sendito_ws::Event::Finished { code, reason, at } => sendito_ws::Event::Finished {
            code,
            reason: scrub(&reason, secrets),
            at,
        },
    }
}

/// The close code the session must end with to count as a success.
const WS_CLEAN_CLOSE: u16 = 1000;

/// The HTTP equivalent of a resolved WebSocket URL: the cookie jar only speaks
/// HTTP, and `ws`/`wss` mean the same origin and secure flag as `http`/`https`.
fn http_equivalent(url: &str) -> String {
    match url.strip_prefix("wss://") {
        Some(rest) => format!("https://{rest}"),
        None => match url.strip_prefix("ws://") {
            Some(rest) => format!("http://{rest}"),
            None => url.to_string(),
        },
    }
}

/// Runs a WebSocket session. Updates go to `updates`; `control` drives
/// interactive input (like the gRPC streaming path).
pub async fn exec_ws(
    ctx: &RunCtx,
    req: &WebSocketRequest,
    mut scope: Scope,
    scope_errors: Vec<String>,
    updates: mpsc::UnboundedSender<WsUpdate>,
    mut control: mpsc::UnboundedReceiver<Control>,
) {
    let mut results = ScriptResults {
        pre: script::run_pre(&req.scripts.pre, &mut scope),
        ..Default::default()
    };
    let mut resolved = sendito_core::resolve::resolve_ws(req, &scope);
    // `timeout_ms` unset falls back to `[websocket] timeout_ms`; 0 means no
    // limit in either place.
    if resolved.timeout_ms.is_none() {
        resolved.timeout_ms = Some(ctx.ws_timeout_ms);
    }
    if resolved.timeout_ms == Some(0) {
        resolved.timeout_ms = None;
    }
    let template = Request::Ws(req.clone());
    let target = match mask_request(&template, &scope) {
        Request::Ws(w) => sendito_core::resolve::resolve_ws(&w, &Scope::new()).url,
        _ => String::new(),
    };
    let started = std::time::Instant::now();
    let fail_secrets = scope.secret_values();
    let fail = |msg: String, results: ScriptResults| {
        let msg = sendito_core::vars::scrub(&msg, &fail_secrets);
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
        let _ = updates.send(WsUpdate::Error(msg));
        let _ = updates.send(WsUpdate::Done {
            scripts: results,
            scope_errors: scope_errors.clone(),
        });
    };
    if matches!(req.auth, Auth::OAuth { .. }) {
        return fail(
            "OAuth 2 authentication is only supported for HTTP requests".into(),
            results,
        );
    }
    if resolved.url.is_empty() {
        return fail("choose a WebSocket URL first".into(), results);
    }
    let proxy = match sendito_ws::WsProxy::of(ctx.send.proxy.as_ref()) {
        Ok(p) => p,
        Err(e) => return fail(e, results),
    };
    let mut settings = sendito_ws::ConnSettings {
        url: resolved.url.clone(),
        headers: resolved.headers.clone(),
        subprotocols: resolved.subprotocols.clone(),
        verify_tls: resolved.verify_tls,
        handshake_timeout: resolved.timeout_ms.map(Duration::from_millis),
        deadline: resolved.deadline_ms.map(Duration::from_millis),
        close_timeout: sendito_ws::CLOSE_TIMEOUT,
        tls: ctx.send.tls.clone(),
        proxy,
    };
    // The collection cookie jar can add a Cookie header to the handshake. The
    // jar speaks HTTP, so a ws:// URL is presented to it as http:// (and wss as
    // https): the same host, path and secure flag the cookie rules depend on.
    let cookie_url = http_equivalent(&resolved.url);
    let cookies_on = req.options.cookies.unwrap_or(ctx.cookie_jar);
    let jar_env = ctx.env.as_deref().unwrap_or(COLLECTION_ENV);
    let mut cookie_secrets = Vec::new();
    if cookies_on {
        match jar_request(&ctx.root, jar_env, &cookie_url).await {
            Ok((header, secrets)) => {
                cookie_secrets = secrets;
                if let Some(header) = header {
                    settings.headers.push(("Cookie".into(), header));
                }
            }
            Err(e) => return fail(e, results),
        }
    }
    let mut session = match sendito_ws::open(&settings).await {
        Ok(s) => s,
        Err(e) => return fail(e.to_string(), results),
    };
    // The opening messages go out right after the handshake, in order.
    let frames = match sendito_ws::parse_messages(&resolved.messages) {
        Ok(f) => f,
        Err(e) => {
            session.cancel();
            return fail(e, results);
        }
    };
    let mut initial_error = None;
    for f in frames {
        let r = match f {
            sendito_ws::Frame::Text(t) => session.send_text(t),
            sendito_ws::Frame::Binary(b) => session.send_binary(b),
        };
        if let Err(e) = r {
            initial_error = Some(e.to_string());
            break;
        }
    }
    let _ = updates.send(WsUpdate::Started {
        resolved: resolved.clone(),
    });
    let secrets = scope.secret_values();
    let mut events: Vec<sendito_ws::Event> = Vec::new();
    let mut control_open = true;
    if let Some(e) = initial_error {
        let _ = updates.send(WsUpdate::Error(e.clone()));
    }
    loop {
        tokio::select! {
            ev = session.events.recv() => {
                let Some(ev) = ev else { break };
                if cookies_on
                    && let sendito_ws::Event::Handshake { headers, .. } = &ev
                {
                    // Store cookies the handshake response set.
                    let sets = set_cookie_headers(headers);
                    if !sets.is_empty()
                        && let Err(e) = jar_store(&ctx.root, jar_env, &cookie_url, &sets).await
                    {
                        let _ = updates.send(WsUpdate::Error(format!(
                            "cookies not stored: {e}"
                        )));
                    }
                }
                let done = matches!(ev, sendito_ws::Event::Finished { .. });
                events.push(ev.clone());
                let ev = if ctx.mask_echoed_secrets { scrub_ws_event(ev, &secrets) } else { ev };
                let _ = updates.send(WsUpdate::Event(ev));
                if done { break; }
            }
            cmd = control.recv(), if control_open => {
                let cmd = cmd.and_then(|c| match c {
                    Control::Ws(w) => Some(w),
                    Control::Grpc(_) => None,
                });
                match cmd {
                    Some(WsControl::Text(text)) => {
                        let rendered = scope.render(&text).text;
                        if let Err(e) = session.send_text(rendered) {
                            let _ = updates.send(WsUpdate::Error(e.to_string()));
                        }
                    }
                    Some(WsControl::Binary(bytes)) => {
                        if let Err(e) = session.send_binary(bytes) {
                            let _ = updates.send(WsUpdate::Error(e.to_string()));
                        }
                    }
                    Some(WsControl::EndStream) => session.close(),
                    Some(WsControl::Cancel) => session.cancel(),
                    // The controller went away (headless runs). With a deadline
                    // the session runs until it expires, so a scripted run sees
                    // every frame the server sends; without one there is nothing
                    // to wait for, and the close handshake ends it. `close()`
                    // is also bounded by `close_timeout`, so this never hangs.
                    None => {
                        control_open = false;
                        if resolved.deadline_ms.is_none() {
                            session.close();
                        }
                    }
                }
            }
        }
    }
    // Scripts see the handshake headers, the close code as status and the
    // last text frame as body.
    let (code, reason, headers, body) = ws_summary(&events);
    let view = ResponseView {
        status: code,
        headers: &headers,
        body: &body,
    };
    results.extracts = script::run_extract(&req.scripts.extract, &view, &mut scope);
    results.asserts = script::run_asserts(&req.scripts.asserts, &view, &scope);
    for assertion in &mut results.asserts {
        let all = [&secrets[..], &cookie_secrets[..]].concat();
        assertion.actual = sendito_core::vars::scrub(&assertion.actual, &all);
        assertion.label = sendito_core::vars::scrub(&assertion.label, &all);
    }
    let total = events
        .iter()
        .rev()
        .find_map(|e| match e {
            sendito_ws::Event::Finished { at, .. } => Some(at.as_millis() as u64),
            _ => None,
        })
        .unwrap_or_else(|| started.elapsed().as_millis() as u64);
    let size: u64 = events
        .iter()
        .map(|e| match e {
            sendito_ws::Event::Received { size, .. } => *size as u64,
            _ => 0,
        })
        .sum();
    let err = if code == WS_CLEAN_CLOSE {
        None
    } else if reason.is_empty() {
        Some(format!("close {code}"))
    } else {
        Some(format!("close {code}: {reason}"))
    };
    let entry = history_entry(ctx, &template, target, Some(code as i64), total, size, err);
    write_history(ctx, &entry);
    let _ = updates.send(WsUpdate::Done {
        scripts: results,
        scope_errors,
    });
}

/// Close code, reason, handshake headers and last text frame payload.
fn ws_summary(events: &[sendito_ws::Event]) -> (u16, String, Vec<(String, String)>, Vec<u8>) {
    let mut code = 1006;
    let mut reason = String::new();
    let mut headers: Vec<(String, String)> = Vec::new();
    let mut body: Vec<u8> = Vec::new();
    for e in events {
        match e {
            sendito_ws::Event::Handshake { headers: h, .. } => {
                headers = h.clone();
            }
            sendito_ws::Event::Received { kind, preview, .. } => {
                if *kind == sendito_ws::FrameKind::Text {
                    body = preview.as_bytes().to_vec();
                }
            }
            sendito_ws::Event::Finished {
                code: c, reason: r, ..
            } => {
                code = *c;
                reason = r.clone();
            }
            _ => {}
        }
    }
    (code, reason, headers, body)
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
    /// `[http] proxy`, also used by MCP servers over Streamable HTTP.
    pub proxy: Option<sendito_http::ProxySpec>,
    /// `[http] tls` (custom CA, client certificate).
    pub tls: sendito_http::TlsMaterial,
}

impl McpRunInfo {
    pub fn of(p: &Project) -> Self {
        Self {
            timeout_ms: p.config.mcp.timeout_ms,
            servers: p.config.mcp.servers.clone(),
            keychain: p.envs.keychain.clone(),
            collection: p.name.clone(),
            proxy: p.send_options().proxy,
            tls: p.tls.clone(),
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
                proxy: info.proxy.clone(),
                tls: info.tls.clone(),
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
    fn ws_urls_map_to_their_http_equivalent_for_the_jar() {
        // The cookie jar only accepts http/https; ws/wss keep the same origin.
        assert_eq!(http_equivalent("ws://h:1/a?b=c"), "http://h:1/a?b=c");
        assert_eq!(http_equivalent("wss://h/a"), "https://h/a");
        assert_eq!(
            http_equivalent("https://h/a"),
            "https://h/a",
            "an http URL is left as is"
        );
        assert_eq!(http_equivalent("h/a"), "h/a");
    }

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
