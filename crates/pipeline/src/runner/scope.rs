//! The variable scope of a request and the names it leaves unresolved.

use super::*;

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
    let (scope, errors, _) = build_scope_checked(envs, active, runtime);
    (scope, errors)
}

/// [`build_scope`], plus the names of the variables whose value did not load
/// (a missing keychain entry or `secrets.toml` value). An active-environment
/// name that failed would resolve from the collection layer below, which is
/// not the value the user chose; [`unresolved_loaded`] prevents that.
pub fn build_scope_checked(
    envs: &Envs,
    active: Option<&str>,
    runtime: &BTreeMap<String, VarValue>,
) -> (Scope, Vec<String>, Vec<String>) {
    let mut scope = Scope::new();
    let mut errors = Vec::new();
    let mut failed = Vec::new();
    for name in [Some(COLLECTION_ENV), active].into_iter().flatten() {
        match envs.load(name) {
            Ok(env) => {
                let r = envs.resolve(&env);
                for (var, e) in r.errors {
                    errors.push(format!("{{{{{var}}}}}: {e}"));
                    failed.push(var);
                }
                scope.push_layer(name, r.vars);
            }
            Err(e) => {
                errors.push(e.to_string());
                // The active environment cannot be read: none of its values
                // are known, so no collection value may stand in for one.
                if name != COLLECTION_ENV
                    && let Some(base) = scope.layers().first()
                {
                    failed.extend(base.vars.keys().cloned());
                }
            }
        }
    }
    scope.push_layer(RUNTIME_LAYER, runtime.clone());
    (scope, errors, failed)
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

/// Unresolved variables against the loaded scope, where a name in `failed`
/// (see [`build_scope_checked`]) counts as unresolved even when a lower layer
/// defines it.
pub fn unresolved_loaded(req: &Request, scope: &Scope, failed: &[String]) -> Vec<String> {
    let mut scope = without(scope, failed);
    let req = dry_pre(req, &mut scope);
    unresolved(&req, &scope)
}

/// The request after a dry run of the pre phase on `scope`, so the variables
/// the Lua pre scripts set and the request fields they change count. The
/// scripts reach nothing outside the run, so running them twice is safe. A
/// script error leaves the request as it is: the send reports the error.
fn dry_pre(req: &Request, scope: &mut Scope) -> Request {
    if req.scripts().lua_pre.is_empty() {
        return req.clone();
    }
    script::run_pre(&req.scripts().pre, scope);
    let mut out = req.clone();
    match lua_script::run_lua_pre(&req.scripts().lua_pre, &mut out, scope) {
        Ok(_) => out,
        Err(_) => req.clone(),
    }
}

/// The check before a send with the loaded scope: [`unresolved_loaded`], plus
/// for an MCP request the failed names its server's configuration uses.
pub fn unresolved_send(
    req: &Request,
    scope: &Scope,
    failed: &[String],
    servers: &BTreeMap<String, sendito_store::config::McpServer>,
) -> Vec<String> {
    let mut un = unresolved_loaded(req, scope, failed);
    if let Request::Mcp(m) = req {
        let server = scope.render(&m.server).text;
        for name in mcp_config_failed(servers, &server, scope, failed) {
            if !un.contains(&name) {
                un.push(name);
            }
        }
    }
    un
}

/// `scope` with `names` (active-environment variables that did not load)
/// removed from the collection layer below, so they never resolve from it,
/// also inside another value. A runtime value above still wins.
pub(super) fn without(scope: &Scope, names: &[String]) -> Scope {
    let mut out = Scope::new();
    for l in scope.layers() {
        let vars = l
            .vars
            .iter()
            .filter(|(k, _)| l.name != COLLECTION_ENV || !names.contains(k))
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        out.push_layer(l.name.clone(), vars);
    }
    out
}

/// The names in `failed` that an MCP server's configuration uses (command,
/// arguments, environment, URL, headers, bearer token), also through other
/// values. They are rendered when the tool is called, outside the request.
pub(super) fn mcp_config_failed(
    servers: &BTreeMap<String, sendito_store::config::McpServer>,
    server: &str,
    scope: &Scope,
    failed: &[String],
) -> Vec<String> {
    use sendito_store::config::{McpServer, McpServerAuth};
    if failed.is_empty() {
        return Vec::new();
    }
    let mut texts: Vec<&str> = Vec::new();
    match servers.get(server) {
        Some(McpServer::Stdio(s)) => {
            texts.push(&s.command);
            texts.extend(s.args.iter().map(String::as_str));
            texts.extend(s.env.values().map(String::as_str));
        }
        Some(McpServer::Http(h)) => {
            texts.push(&h.url);
            texts.extend(h.headers.values().map(String::as_str));
            // An Authorization header wins over the token, which is then unused.
            let explicit = h
                .headers
                .keys()
                .any(|k| k.eq_ignore_ascii_case("authorization"));
            if let (Some(McpServerAuth::Bearer { token }), false) = (&h.auth, explicit) {
                texts.push(token);
            }
        }
        None => {}
    }
    let stripped = without(scope, failed);
    let mut out = Vec::new();
    for t in texts {
        for name in stripped.unresolved(t) {
            if failed.contains(&name) && !out.contains(&name) {
                out.push(name);
            }
        }
    }
    out
}

/// The folder chain of the request at `rel` (relative to the collection
/// root): the `folder.toml` settings from the root to its folder. An unsaved
/// request (`None`) gets the root's settings only.
pub fn folder_chain(root: &Path, rel: Option<&str>) -> Result<Vec<ChainItem>, String> {
    Collection::new(root, Vec::new())
        .chain(rel.unwrap_or(""))
        .map_err(|e| e.to_string())
}

/// The request with its folders applied (see `sendito_core::inherit`), and
/// the folder variables added to `scope` below the runtime variables. Apply
/// it once per send: folder scripts would run twice otherwise.
pub fn with_folders(req: &Request, chain: &[ChainItem], scope: &mut Scope) -> Request {
    add_folder_vars(scope, chain);
    inherit::apply(req, chain)
}

/// Adds the folder variable layers below the runtime variables.
pub fn add_folder_vars(scope: &mut Scope, chain: &[ChainItem]) {
    scope.insert_below(RUNTIME_LAYER, inherit::var_layers(chain));
}
