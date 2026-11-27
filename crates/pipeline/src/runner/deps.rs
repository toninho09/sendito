//! `$response('Folder/request', '$.path', 'policy')`: a value from the body
//! of another request's last response (`FORMAT.md`, "Values from another
//! response"). The value comes from this session, then from the history; the
//! policy decides when the request is sent first.

use std::future::Future;
use std::pin::Pin;
use std::time::SystemTime;

use sendito_core::funcs::{Policy, PreSend};
use sendito_core::vars::{MAX_DEPTH, Rendered};
use sendito_store::collection::{EXT, name_of};

use super::*;

/// A response kept for `$response`: the body as the scripts read it.
#[derive(Debug, Clone)]
pub struct Cached {
    pub at: SystemTime,
    pub status: u16,
    pub reason: String,
    pub body: Vec<u8>,
}

/// The last response of each saved HTTP request sent in this session, by
/// environment and request path. Clones share one cache.
#[derive(Debug, Clone, Default)]
pub struct ResponseCache(Arc<Mutex<HashMap<CacheKey, Cached>>>);

/// The environment (`None`: no environment) and the request path.
type CacheKey = (Option<String>, String);

impl ResponseCache {
    fn get(&self, env: &Option<String>, path: &str) -> Option<Cached> {
        let map = self.0.lock().ok()?;
        map.get(&(env.clone(), path.to_string())).cloned()
    }

    pub(super) fn put(&self, env: &Option<String>, path: &str, c: Cached) {
        if let Ok(mut map) = self.0.lock() {
            map.insert((env.clone(), path.to_string()), c);
        }
    }
}

/// Gives `scope` the value of every `$response` call of `req`. A dependency
/// is sent first when its policy asks; what its scripts set goes to the
/// runtime layer of `scope` and to the returned `pre` list. The returned
/// logs say where each value came from.
///
/// The future has its type erased: a dependency send runs the whole pipeline
/// again, which comes back here.
pub(super) fn deps_phase<'a>(
    ctx: &'a RunCtx,
    req: &'a Request,
    scope: &'a mut Scope,
) -> Pin<Box<dyn Future<Output = Result<ScriptResults, String>> + Send + 'a>> {
    Box::pin(deps_phase_inner(ctx, req, scope))
}

async fn deps_phase_inner(
    ctx: &RunCtx,
    req: &Request,
    scope: &mut Scope,
) -> Result<ScriptResults, String> {
    let mut results = ScriptResults::default();
    for (text, p) in sendito_core::resolve::pre_send_calls(req, scope) {
        let PreSend::Response { path, expr, policy } = p else {
            continue;
        };
        let rel = format!("{path}.{EXT}");
        let max_age = match policy {
            Policy::Always => Some(Duration::ZERO),
            Policy::IfOlder(s) => Some(Duration::from_secs(s)),
            Policy::Never | Policy::IfMissing => None,
        };
        let found = if policy == Policy::Always {
            None
        } else {
            kept(ctx, &rel, max_age)
        };
        let (cached, how) = match found {
            Some(f) => f,
            None if policy == Policy::Never => {
                return Err(format!(
                    "{text}: {path} has no response yet (the policy is never)"
                ));
            }
            None => {
                let (cached, how, sets) = send(ctx, base_scope(scope), &path, &rel).await?;
                for (var, value) in sets {
                    scope.set(RUNTIME_LAYER, &var, value.clone());
                    results.pre.push((var, value));
                }
                (cached, how)
            }
        };
        let value = script::json_path_value(&cached.body, &expr)
            .map_err(|e| format!("{text}: {e}"))?
            .ok_or_else(|| {
                format!(
                    "{text}: {expr} has no match in the response of {path} ({} {})",
                    cached.status, cached.reason
                )
            })?;
        scope.keep(
            &text,
            Rendered {
                text: value,
                secret: true,
                unresolved: vec![],
            },
        );
        results.logs.push(format!("dependency {path}: {how}"));
    }
    Ok(results)
}

/// The kept response of `rel` that is not older than `max_age`: from this
/// session, else from the history of the same environment.
fn kept(ctx: &RunCtx, rel: &str, max_age: Option<Duration>) -> Option<(Cached, String)> {
    let fresh = |at: SystemTime| max_age.is_none_or(|max| at.elapsed().is_ok_and(|age| age <= max));
    if let Some(c) = ctx.responses.get(&ctx.env, rel).filter(|c| fresh(c.at)) {
        return Some((c, "the response of this session".into()));
    }
    let path = history::history_path(&ctx.root);
    let entry = history::load(&path, usize::MAX)
        .ok()?
        .into_iter()
        .rev()
        .find(|e| e.path.as_deref() == Some(rel) && e.env == ctx.env && e.response.is_some())?;
    let at = time::OffsetDateTime::parse(&entry.ts, &time::format_description::well_known::Rfc3339)
        .ok()
        .map(SystemTime::from)?;
    if !fresh(at) {
        return None;
    }
    let r = history::load_response(&path, entry.response.as_deref()?).ok()?;
    let c = Cached {
        at,
        status: r.status,
        reason: r.reason,
        body: r.body,
    };
    Some((
        c,
        format!("the response of the history entry of {}", entry.ts),
    ))
}

/// Sends the saved request `rel` with its folders and the variables of
/// `dscope` (see [`base_scope`]).
/// Returns its response, a log text and the variables its scripts set.
async fn send(
    ctx: &RunCtx,
    mut dscope: Scope,
    path: &str,
    rel: &str,
) -> Result<(Cached, String, Vec<(String, VarValue)>), String> {
    if ctx.deps.iter().any(|d| d == rel) {
        let mut chain = ctx.deps.clone();
        chain.push(rel.to_string());
        return Err(format!("$response cycle: {}", chain.join(" → ")));
    }
    if ctx.deps.len() >= MAX_DEPTH {
        return Err(format!(
            "{path}: the dependencies nest deeper than {MAX_DEPTH}"
        ));
    }
    let req = Collection::new(&ctx.root, Vec::new())
        .load(rel)
        .map_err(|e| format!("{path}: {e}"))?;
    if !matches!(req, Request::Http(_)) {
        return Err(format!(
            "{path} is not an HTTP request: $response reads HTTP responses only"
        ));
    }
    let chain = folder_chain(&ctx.root, Some(rel))?;
    let req = with_folders(&req, &chain, &mut dscope);
    let asks = sendito_core::resolve::pre_send_calls(&req, &dscope)
        .into_iter()
        .any(|(_, p)| !matches!(p, PreSend::Response { .. }));
    if asks {
        return Err(format!(
            "{path} uses $prompt or $select: a dependency cannot ask for a value"
        ));
    }
    let un = unresolved_loaded(&req, &dscope, &[]);
    if !un.is_empty() {
        return Err(format!(
            "{path}: unresolved variables: {}",
            unresolved_list(&un, &dscope)
        ));
    }
    let Request::Http(h) = req else {
        return Err(format!("{path} is not an HTTP request"));
    };
    let mut dctx = ctx.clone();
    dctx.name = name_of(rel).to_string();
    dctx.path = Some(rel.to_string());
    dctx.deps.push(rel.to_string());
    let o = exec_http(&dctx, &h, dscope, Vec::new()).await;
    let head = o.response.map_err(|e| format!("{path}: {e}"))?;
    let cached = ctx
        .responses
        .get(&ctx.env, rel)
        .ok_or_else(|| format!("{path}: the response was not kept"))?;
    let how = format!(
        "sent ({} {}, {} ms)",
        head.status,
        head.reason,
        head.timing.total.as_millis()
    );
    Ok((cached, how, o.scripts.updates()))
}

/// `scope` without the folder variables and the signed values of the
/// request that is sent now: a dependency gets its own.
fn base_scope(scope: &Scope) -> Scope {
    let mut out = Scope::new();
    out.set_catalog(scope.catalog().clone());
    for l in scope.layers() {
        if !l.name.starts_with(inherit::FOLDER_LAYER) && l.name != SIGNED_LAYER {
            out.push_layer(l.name.clone(), l.vars.clone());
        }
    }
    out
}
