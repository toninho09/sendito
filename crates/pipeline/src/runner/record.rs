//! History entries and the secrets they must not hold.

use super::*;

pub(super) fn history_entry(
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

/// Appends to the history on this thread. Only for the early failure paths
/// that run in closures; [`write_history`] is the normal path.
pub(super) fn append_history(ctx: &RunCtx, entry: &HistoryEntry) {
    if let Some((path, max)) = &ctx.history {
        let _ = history::append(path, entry, *max);
    }
}

/// Appends to the history off the async workers: the append takes a file lock
/// and may trim the file.
pub(super) async fn write_history(ctx: &RunCtx, entry: &HistoryEntry) {
    if let Some((path, max)) = ctx.history.clone() {
        let entry = entry.clone();
        let _ = tokio::task::spawn_blocking(move || history::append(&path, &entry, max)).await;
    }
}

/// Secret values to mask in what the server sends back: every secret variable
/// and, when a Basic password is secret, the encoded credentials.
pub(super) fn http_secrets(req: &HttpRequest, scope: &Scope) -> Vec<String> {
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

/// Masks secret values in assertion results; their text reaches the screen
/// and the CLI output, so this runs whatever `mask_echoed_secrets` says.
pub(super) fn scrub_asserts(asserts: &mut [AssertResult], secrets: &[String]) {
    for a in asserts {
        a.actual = sendito_core::vars::scrub(&a.actual, secrets);
        a.label = sendito_core::vars::scrub(&a.label, secrets);
    }
}

/// Masks secret values in an event shown to the user.
pub(super) fn scrub_event(ev: Event, secrets: &[String]) -> Event {
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
