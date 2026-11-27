//! Sends an HTTP request with its cookie jar and OAuth token.

use super::*;

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
    /// The configuration problem behind a failure, or behind a `401`, a
    /// `403` or a `407` that one causes.
    pub hint: Option<diagnose::Hint>,
}

pub(super) fn http_oauth_provider(
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
    let network = sendito_mcp::oauth::Network {
        tls: ctx.send.tls.clone(),
        proxy: ctx.send.proxy.clone(),
    };
    let settings = oauth_settings(identity, scope, network)?;
    Ok(sendito_mcp::oauth::RefreshingProvider::new_http(
        Arc::new(KeychainStore(info.keychain.clone())),
        oauth_token_account(&info.collection, ctx.env.as_deref(), &rendered.text),
        settings,
    ))
}

/// Reads the jar's matching `Cookie` header off the UI thread.
pub(super) async fn jar_request(
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
pub(super) async fn jar_store(
    root: &Path,
    env: &str,
    url: &str,
    sets: &[String],
) -> Result<(), String> {
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

/// The cookie jar on each redirect hop: stores what the hop's response sets
/// and gives the next hop its own `Cookie` header. The values it reads are
/// kept for masking.
pub(super) struct JarHops<'a> {
    root: &'a Path,
    env: &'a str,
    secrets: Mutex<Vec<String>>,
}

impl sendito_http::RedirectCookies for JarHops<'_> {
    fn hop<'a>(
        &'a self,
        from: &'a str,
        set_cookies: Vec<String>,
        to: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<Option<String>, String>> + Send + 'a>,
    > {
        Box::pin(async move {
            let mut found = sendito_store::cookies::response_secrets(&set_cookies);
            if !set_cookies.is_empty() {
                jar_store(self.root, self.env, from, &set_cookies).await?;
            }
            let (header, secrets) = jar_request(self.root, self.env, to).await?;
            found.extend(secrets);
            if let Ok(mut s) = self.secrets.lock() {
                s.extend(found);
            }
            Ok(header)
        })
    }
}

pub(super) fn set_cookie_headers(headers: &[(String, String)]) -> Vec<String> {
    headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
        .map(|(_, v)| v.clone())
        .collect()
}

struct Exchange {
    head: HttpResponse,
    stream: Option<sendito_http::HttpStream>,
}
impl std::ops::Deref for Exchange {
    type Target = HttpResponse;
    fn deref(&self) -> &HttpResponse {
        &self.head
    }
}
impl std::ops::DerefMut for Exchange {
    fn deref_mut(&mut self) -> &mut HttpResponse {
        &mut self.head
    }
}
pub(super) struct HttpRun {
    pub outcome: HttpOutcome,
    pub stream: Option<sendito_http::HttpStream>,
    pub secrets: Vec<String>,
}
/// Sends one HTTP request; with the cookie jar, each redirect hop goes
/// through it.
async fn send_http(
    resolved: &sendito_core::resolve::ResolvedHttp,
    ctx: &RunCtx,
    jar: Option<&JarHops<'_>>,
    streaming: bool,
) -> Result<Exchange, sendito_http::HttpError> {
    let cookies = jar.map(|j| j as &dyn sendito_http::RedirectCookies);
    if streaming {
        let stream = sendito_http::open_stream(resolved, &ctx.send, cookies)
            .await?
            .into_stream();
        Ok(Exchange {
            head: stream.head(),
            stream: Some(stream),
        })
    } else {
        Ok(Exchange {
            head: sendito_http::send_with(resolved, &ctx.send, cookies).await?,
            stream: None,
        })
    }
}

/// Runs an HTTP request through the full pipeline.
pub async fn exec_http(
    ctx: &RunCtx,
    req: &HttpRequest,
    scope: Scope,
    scope_errors: Vec<String>,
) -> HttpOutcome {
    exec_http_inner(ctx, req, scope, scope_errors, false)
        .await
        .outcome
}
pub(super) async fn exec_http_inner(
    ctx: &RunCtx,
    req: &HttpRequest,
    mut scope: Scope,
    scope_errors: Vec<String>,
    streaming: bool,
) -> HttpRun {
    let PrePhase {
        req,
        mut results,
        error: pre_error,
    } = prepare(ctx, req, &mut scope).await;
    let req = &req;
    let mut resolved = resolve_http(req, &scope, &ctx.root);
    let template = Request::Http(req.clone());
    let target = match mask_request(&template, &scope) {
        Request::Http(h) => sendito_core::resolve::normalize_url(&h.url),
        _ => String::new(),
    };
    // What a diagnosis needs about the request: the OAuth identity is one
    // only when its token went out (an explicit Authorization header wins).
    let oauth_name = match &req.auth {
        Auth::OAuth { name } => Some(name.clone()),
        _ => None,
    };
    let mut cx = diagnose::Context {
        timeout_ms: resolved.timeout_ms,
        verify_tls: Some(resolved.verify_tls),
        ..diagnose::Context::of(ctx)
    };
    if let Err(message) = ctx.check_files(body_files(&resolved.body)) {
        resolved.body = sendito_core::resolve::ResolvedBody::Invalid(message);
    }
    if let Some(message) = pre_error {
        log_stopped(ctx, &template, &target, &message);
        let hint = http_hint(&message, &resolved, &cx);
        mask_shown(&mut resolved, &http_secrets(req, &scope));
        return HttpRun {
            stream: None,
            secrets: Vec::new(),
            outcome: HttpOutcome {
                resolved,
                response: Err(message),
                doc: Doc::default(),
                body_size: 0,
                scripts: results,
                scope_errors,
                hint,
            },
        };
    }
    if let sendito_core::resolve::ResolvedBody::Invalid(message) = &resolved.body {
        let message = sendito_core::vars::scrub(message, &scope.secret_values());
        log_stopped(ctx, &template, &target, &message);
        let hint = http_hint(&message, &resolved, &cx);
        mask_shown(&mut resolved, &http_secrets(req, &scope));
        return HttpRun {
            stream: None,
            secrets: Vec::new(),
            outcome: HttpOutcome {
                resolved,
                response: Err(message.clone()),
                doc: Doc::default(),
                body_size: 0,
                scripts: results,
                scope_errors,
                hint,
            },
        };
    }
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
                        .push(sendito_core::resolve::HeaderOrigin::Jar);
                    jar_cookie = Some(resolved.headers.len() - 1);
                }
                Ok(())
            }
            Err(e) => Err(sendito_http::HttpError::Send(e)),
        }
    } else {
        Ok(())
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
                        resolved
                            .headers
                            .push(("Authorization".into(), format!("Bearer {token}")));
                        resolved
                            .header_origins
                            .push(sendito_core::resolve::HeaderOrigin::Auth);
                        provider = Some(p);
                        Ok(())
                    }
                    Err(e @ sendito_mcp::McpError::NeedsLogin(_)) => {
                        Err(sendito_http::HttpError::Send(format!(
                            "OAuth token unavailable ({e}); run sendito auth login for this identity and environment"
                        )))
                    }
                    Err(e) => Err(sendito_http::HttpError::Send(format!(
                        "OAuth token request failed: {e}"
                    ))),
                },
                Err(e) => Err(sendito_http::HttpError::Send(e)),
            }
        }
        _ => Ok(()),
    };
    // JWT and AWS SigV4 go on now that the headers are final (the jar's
    // Cookie included); Digest answers the server's challenge below.
    let auth_setup = match auth_setup {
        Ok(()) => sign_before_send(ctx, &mut resolved, &mut cookie_secrets).await,
        Err(e) => Err(e),
    };
    let jar_hops = JarHops {
        root: &ctx.root,
        env: jar_env,
        secrets: Mutex::new(Vec::new()),
    };
    let jar = Jar {
        on: cookies_on,
        env: jar_env,
        hops: &jar_hops,
        streaming,
    };
    // One timeout covers the first send, a token refresh and the retry.
    let budget = resolved
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(ctx.send.default_timeout);
    let started = Instant::now();
    let mut sent = match cookie_setup.and(auth_setup) {
        Ok(()) => send_http(&resolved, ctx, cookies_on.then_some(&jar_hops), streaming).await,
        Err(e) => Err(e),
    };
    // A 401 from the origin the request was sent to.
    let unauthorized = |sent: &Result<Exchange, sendito_http::HttpError>, url: &str| {
        sent.as_ref().is_ok_and(|r| {
            r.status == 401
                && url::Url::parse(url)
                    .ok()
                    .zip(url::Url::parse(&r.url).ok())
                    .is_some_and(|(original, final_url)| original.origin() == final_url.origin())
        })
    };
    // The OAuth token went out only when the setup added it: a 401 or a 403
    // is about that token then, and about the other credentials otherwise.
    let oauth_sent = provider.is_some();
    if unauthorized(&sent, &resolved.url)
        && let Some(provider) = provider
        && let Ok(first) = &sent
    {
        use sendito_mcp::TokenProvider;
        let left = budget.saturating_sub(started.elapsed());
        let refreshed = tokio::time::timeout(left, provider.token(true)).await;
        let left = budget.saturating_sub(started.elapsed());
        sent = match refreshed {
            Ok(Err(e)) => Err(sendito_http::HttpError::Send(format!(
                "OAuth refresh failed ({e}); sign in again"
            ))),
            Ok(Ok(token)) if !left.is_zero() => {
                resolved.timeout_ms = Some((left.as_millis() as u64).max(1));
                cookie_secrets.push(token.clone());
                let first = first.head.clone();
                retry_401(
                    ctx,
                    &mut resolved,
                    &first,
                    format!("Bearer {token}"),
                    &jar,
                    &mut jar_cookie,
                    &mut cookie_secrets,
                )
                .await
            }
            _ => Err(sendito_http::HttpError::Timeout(budget.as_millis())),
        };
    } else if unauthorized(&sent, &resolved.url)
        && let Some(SignedAuth::Digest { username, password }) = resolved.signed.clone()
        && let Ok(first) = &sent
    {
        let values: Vec<&str> = first
            .headers
            .iter()
            .filter(|(n, _)| n.eq_ignore_ascii_case("www-authenticate"))
            .map(|(_, v)| v.as_str())
            .collect();
        match sendito_http::auth::digest::challenge(&values) {
            None => resolved
                .notes
                .push("Digest: the 401 has no Digest challenge Sendito can answer".into()),
            Some(challenge) => {
                let left = budget.saturating_sub(started.elapsed());
                if left.is_zero() {
                    sent = Err(sendito_http::HttpError::Timeout(budget.as_millis()));
                } else {
                    resolved.timeout_ms = Some((left.as_millis() as u64).max(1));
                    let value = sendito_http::auth::digest::authorization(
                        &challenge,
                        &username,
                        &password,
                        &resolved.method,
                        &request_target(&resolved.url),
                        &sendito_http::auth::digest::cnonce(),
                        1,
                    );
                    cookie_secrets.push(value.clone());
                    let first = first.head.clone();
                    sent = retry_401(
                        ctx,
                        &mut resolved,
                        &first,
                        value,
                        &jar,
                        &mut jar_cookie,
                        &mut cookie_secrets,
                    )
                    .await;
                }
            }
        }
    }
    cx.oauth = oauth_sent.then_some(oauth_name.as_deref()).flatten();
    if let Ok(mut hop_secrets) = jar_hops.secrets.lock() {
        cookie_secrets.append(&mut hop_secrets);
    }
    if let Ok(resp) = &sent {
        // Cookie values are secret even when the jar is off: a `cookie`
        // assertion or extraction must not show them.
        let sets = set_cookie_headers(&resp.headers);
        cookie_secrets.extend(sendito_store::cookies::response_secrets(&sets));
        if cookies_on
            && !sets.is_empty()
            && let Err(e) = jar_store(&ctx.root, jar_env, &resp.url, &sets).await
        {
            resolved.notes.push(format!("cookies not stored: {e}"));
        }
    }
    if streaming && let Ok(exchange) = &mut sent {
        let mut secrets = http_secrets(req, &scope);
        secrets.extend(cookie_secrets);
        return HttpRun {
            stream: exchange.stream.take(),
            secrets,
            outcome: HttpOutcome {
                resolved,
                response: Ok(exchange.head.clone()),
                doc: Doc::default(),
                body_size: 0,
                scripts: results,
                scope_errors,
                hint: None,
            },
        };
    }
    let mut body_size = 0;
    let (response, doc, hint) = match sent {
        Ok(mut resp) => {
            let body = std::mem::take(&mut resp.body);
            let timing = http_timing(&resp.timing);
            let view = ResponseView {
                status: resp.status,
                headers: &resp.headers,
                body: &body,
                timing: &timing,
            };
            response_phase(
                &req.common.scripts,
                &view,
                &mut scope,
                &mut results,
                &cookie_secrets,
            );
            // `$response` reads the last response of a saved request.
            if let Some(path) = &ctx.path {
                let kept = Cached {
                    at: std::time::SystemTime::now(),
                    status: resp.status,
                    reason: resp.reason.clone(),
                    body: body.clone(),
                };
                ctx.responses.put(&ctx.env, path, kept);
            }
            let mut assert_secrets = http_secrets(req, &scope);
            assert_secrets.extend(cookie_secrets.iter().cloned());
            scrub_asserts(&mut results.asserts, &assert_secrets);
            // The history always masks the secrets the server echoes, also
            // in `sendito run`, which shows the body unchanged.
            let mut history_secrets = http_secrets(req, &scope);
            history_secrets.extend(cookie_secrets.iter().cloned());
            let history_body = ctx
                .history
                .as_ref()
                .map(|h| h.max_body)
                .filter(|max| *max > 0);
            let history_headers = history_body.map(|_| {
                let headers = resp
                    .headers
                    .iter()
                    .map(|(n, v)| (n.clone(), sendito_core::vars::scrub(v, &history_secrets)))
                    .collect();
                sendito_store::examples::mask_credential_headers(headers)
            });
            // Secrets echoed back by the server are masked before display.
            let secrets = if ctx.mask_echoed_secrets {
                history_secrets.clone()
            } else {
                Vec::new()
            };
            // A redirect can carry a secret in the query of the final URL.
            resp.url = sendito_core::vars::scrub(&resp.url, &secrets);
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
            let (doc, kept) = tokio::task::spawn_blocking(move || {
                // Mask before the cut: a secret across the cut is not kept.
                let kept = history_body.map(|max| {
                    let masked = sendito_core::vars::scrub_bytes(&body, &history_secrets);
                    sendito_store::history::cut_body(masked.as_deref().unwrap_or(&body), max)
                });
                let body = sendito_core::vars::scrub_bytes(&body, &secrets).unwrap_or(body);
                (Doc::from_bytes(body, ct.as_deref()), kept)
            })
            .await
            .unwrap_or_default();
            let response =
                kept.zip(history_headers)
                    .map(|(body, headers)| sendito_store::examples::Example {
                        name: String::new(),
                        status: resp.status,
                        reason: resp.reason.clone(),
                        headers,
                        body,
                    });
            let entry = history_entry(
                ctx,
                &template,
                target,
                Some(resp.status as i64),
                resp.timing.total.as_millis() as u64,
                size,
                None,
            );
            write_history(ctx, &entry, response).await;
            let hint = diagnose::status(resp.status, &cx);
            (Ok(resp.head), doc, hint)
        }
        Err(e) => {
            let mut secrets = http_secrets(req, &scope);
            secrets.extend(cookie_secrets);
            let msg = sendito_core::vars::scrub(&e.to_string(), &secrets);
            let hint = http_hint(&msg, &resolved, &cx);
            let entry = history_entry(
                ctx,
                &template,
                target,
                None,
                start.elapsed().as_millis() as u64,
                0,
                Some(msg.clone()),
            );
            write_history(ctx, &entry, None).await;
            (Err(msg), Doc::default(), hint)
        }
    };
    // `resolved` is only shown from here on (the Headers tab lists the
    // request as sent): a secret in the query or a header is masked.
    mask_shown(&mut resolved, &http_secrets(req, &scope));
    HttpRun {
        stream: None,
        secrets: Vec::new(),
        outcome: HttpOutcome {
            resolved,
            response,
            doc,
            body_size,
            scripts: results,
            scope_errors,
            hint,
        },
    }
}

/// Masks the secret values in the parts of `resolved` that are shown (the
/// Headers tab lists the request as sent): the URL, the headers and the notes.
fn mask_shown(resolved: &mut ResolvedHttp, secrets: &[String]) {
    resolved.url = sendito_core::vars::scrub(&resolved.url, secrets);
    for (_, v) in &mut resolved.headers {
        *v = sendito_core::vars::scrub(v, secrets);
    }
    for n in &mut resolved.notes {
        *n = sendito_core::vars::scrub(n, secrets);
    }
}

/// The configuration problem behind a failed send: a variable the URL
/// keeps, the OAuth setup, then the transport.
pub(super) fn http_hint(
    message: &str,
    resolved: &ResolvedHttp,
    cx: &diagnose::Context<'_>,
) -> Option<diagnose::Hint> {
    diagnose::unresolved_url(&resolved.url)
        .or_else(|| diagnose::oauth(message))
        .or_else(|| diagnose::transport(message, cx))
}

/// The cookie jar of a send, for a retry after a `401`.
struct Jar<'a> {
    on: bool,
    env: &'a str,
    hops: &'a JarHops<'a>,
    streaming: bool,
}

/// Sends `resolved` again after a `401`, with `authorization` as its
/// `Authorization` header. With the cookie jar, the cookies the 401 set are
/// stored and the jar's new `Cookie` header replaces the first send's.
async fn retry_401(
    ctx: &RunCtx,
    resolved: &mut sendito_core::resolve::ResolvedHttp,
    first: &sendito_http::HttpResponse,
    authorization: String,
    jar: &Jar<'_>,
    jar_cookie: &mut Option<usize>,
    secrets: &mut Vec<String>,
) -> Result<Exchange, sendito_http::HttpError> {
    match resolved
        .headers
        .iter_mut()
        .find(|(n, _)| n.eq_ignore_ascii_case("authorization"))
    {
        Some((_, value)) => *value = authorization,
        None => {
            resolved
                .headers
                .push(("Authorization".into(), authorization));
            resolved
                .header_origins
                .push(sendito_core::resolve::HeaderOrigin::Auth);
        }
    }
    if jar.on {
        let sets = set_cookie_headers(&first.headers);
        if !sets.is_empty() {
            secrets.extend(sendito_store::cookies::response_secrets(&sets));
            if let Err(e) = jar_store(&ctx.root, jar.env, &first.url, &sets).await {
                resolved.notes.push(format!("cookies not stored: {e}"));
            }
        }
        match jar_request(&ctx.root, jar.env, &resolved.url).await {
            Ok((header, found)) => {
                secrets.extend(found);
                match (*jar_cookie, header) {
                    (Some(i), Some(h)) => resolved.headers[i].1 = h,
                    (Some(i), None) => {
                        resolved.headers.remove(i);
                        resolved.header_origins.remove(i);
                        *jar_cookie = None;
                    }
                    (None, Some(h)) => {
                        resolved.headers.push(("Cookie".into(), h));
                        resolved
                            .header_origins
                            .push(sendito_core::resolve::HeaderOrigin::Jar);
                        *jar_cookie = Some(resolved.headers.len() - 1);
                    }
                    (None, None) => {}
                }
            }
            Err(e) => resolved.notes.push(format!("cookie jar: {e}")),
        }
    }
    send_http(resolved, ctx, jar.on.then_some(jar.hops), jar.streaming).await
}

/// The path and query of a URL, as the request line has them (Digest `uri`).
fn request_target(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(u) => match u.query() {
            Some(q) => format!("{}?{q}", u.path()),
            None => u.path().to_string(),
        },
        Err(_) => url.to_string(),
    }
}

/// Adds the headers of a JWT or an AWS SigV4 auth; the values are masked
/// like other secrets.
async fn sign_before_send(
    ctx: &RunCtx,
    resolved: &mut sendito_core::resolve::ResolvedHttp,
    secrets: &mut Vec<String>,
) -> Result<(), sendito_http::HttpError> {
    let fail = sendito_http::HttpError::Send;
    let added = match resolved.signed.clone() {
        Some(SignedAuth::Jwt { .. }) => {
            let token = jwt_token(&ctx.root, ctx.confine_files, resolved.signed.as_ref())
                .await
                .map_err(fail)?;
            vec![(
                "Authorization".to_string(),
                format!("Bearer {}", token.unwrap_or_default()),
            )]
        }
        Some(SignedAuth::AwsSigV4 {
            access_key,
            secret_key,
            session_token,
            region,
            service,
        }) => {
            let payload = payload_sha256(&resolved.body).await.map_err(fail)?;
            if !session_token.is_empty() {
                secrets.push(session_token.clone());
            }
            sendito_http::auth::sigv4::sign(
                &resolved.method,
                &resolved.url,
                &resolved.headers,
                &payload,
                &sendito_http::auth::sigv4::Credentials {
                    access_key: &access_key,
                    secret_key: &secret_key,
                    session_token: &session_token,
                    region: &region,
                    service: &service,
                },
                std::time::SystemTime::now(),
            )
            .map_err(fail)?
        }
        _ => return Ok(()),
    };
    for (name, value) in added {
        if name == "Authorization" {
            secrets.push(value.clone());
        }
        resolved.headers.push((name, value));
        resolved
            .header_origins
            .push(sendito_core::resolve::HeaderOrigin::Auth);
    }
    Ok(())
}

/// The SHA-256 of the body as it is sent (AWS SigV4).
async fn payload_sha256(body: &sendito_core::resolve::ResolvedBody) -> Result<String, String> {
    use sendito_core::resolve::ResolvedBody;
    match body {
        ResolvedBody::None => Ok(sendito_http::auth::sha256_hex(b"")),
        ResolvedBody::Bytes(b) => Ok(sendito_http::auth::sha256_hex(b)),
        ResolvedBody::File(p) => tokio::fs::read(p)
            .await
            .map(|b| sendito_http::auth::sha256_hex(&b))
            .map_err(|e| format!("AWS SigV4: cannot read {}: {e}", p.display())),
        ResolvedBody::Multipart(_) => Err(
            "AWS SigV4 cannot sign a multipart body: its boundary is made when it is sent; \
             use a file or raw body"
                .into(),
        ),
        ResolvedBody::Invalid(msg) => Err(msg.clone()),
    }
}

/// The files a body reads: the file of a file body, the file parts of a
/// multipart body.
fn body_files(body: &sendito_core::resolve::ResolvedBody) -> Vec<&Path> {
    use sendito_core::resolve::{PartData, ResolvedBody};
    match body {
        ResolvedBody::File(p) => vec![p.as_path()],
        ResolvedBody::Multipart(parts) => parts
            .iter()
            .filter_map(|p| match &p.data {
                PartData::File { path, .. } => Some(path.as_path()),
                PartData::Text(_) => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The token of a JWT auth, signed now; `None` for any other auth. The
/// private key path is relative to the collection root; with `confine`, it
/// must pass [`confined`].
pub(super) async fn jwt_token(
    root: &Path,
    confine: bool,
    signed: Option<&SignedAuth>,
) -> Result<Option<String>, String> {
    let Some(SignedAuth::Jwt {
        algorithm,
        secret,
        private_key,
        claims,
        expires_in_s,
    }) = signed
    else {
        return Ok(None);
    };
    let key = if algorithm.uses_secret() {
        None
    } else if private_key.is_empty() {
        return Err(format!("{} needs a private key file", algorithm.as_str()));
    } else {
        let path = sendito_store::env::expand(root, private_key);
        if confine {
            confined(root, &path)?;
        }
        Some(
            tokio::fs::read(&path)
                .await
                .map_err(|e| format!("JWT key {}: {e}", path.display()))?,
        )
    };
    sendito_http::auth::jwt::sign(
        *algorithm,
        secret,
        key.as_deref(),
        claims,
        *expires_in_s,
        std::time::SystemTime::now(),
    )
    .map(Some)
    .map_err(|e| format!("JWT: {e}"))
}

/// The measured phases of an HTTP exchange, for the `time` script source.
fn http_timing(t: &sendito_http::Timing) -> Vec<(&'static str, Duration)> {
    let mut out = vec![
        ("total", t.total),
        ("ttfb", t.ttfb),
        ("download", t.download),
    ];
    for (name, d) in [("dns", t.dns), ("connect", t.connect), ("tls", t.tls)] {
        if let Some(d) = d {
            out.push((name, d));
        }
    }
    out
}
