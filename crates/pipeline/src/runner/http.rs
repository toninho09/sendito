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

/// Sends one HTTP request; with the cookie jar, each redirect hop goes
/// through it.
pub(super) async fn send_http(
    resolved: &sendito_core::resolve::ResolvedHttp,
    ctx: &RunCtx,
    jar: Option<&JarHops<'_>>,
) -> Result<sendito_http::HttpResponse, sendito_http::HttpError> {
    match jar {
        Some(j) => sendito_http::send_with(resolved, &ctx.send, Some(j)).await,
        None => sendito_http::send(resolved, &ctx.send).await,
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
                        resolved
                            .headers
                            .push(("Authorization".into(), format!("Bearer {token}")));
                        resolved
                            .header_origins
                            .push(sendito_core::resolve::HeaderOrigin::Auth);
                        provider = Some(p);
                        Ok(())
                    }
                    Err(e) => Err(sendito_http::HttpError::Send(format!(
                        "OAuth token unavailable ({e}); run sendito auth login for this identity and environment"
                    ))),
                },
                Err(e) => Err(sendito_http::HttpError::Send(e)),
            }
        }
        _ => Ok(()),
    };
    let jar_hops = JarHops {
        root: &ctx.root,
        env: jar_env,
        secrets: Mutex::new(Vec::new()),
    };
    // One timeout covers the first send, a token refresh and the retry.
    let budget = resolved
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(ctx.send.default_timeout);
    let started = Instant::now();
    let mut sent = match cookie_setup.and(auth_setup) {
        Ok(()) => send_http(&resolved, ctx, cookies_on.then_some(&jar_hops)).await,
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
        let left = budget.saturating_sub(started.elapsed());
        let refreshed = tokio::time::timeout(left, provider.token(true)).await;
        let left = budget.saturating_sub(started.elapsed());
        match refreshed {
            Ok(Err(e)) => {
                sent = Err(sendito_http::HttpError::Send(format!(
                    "OAuth refresh failed ({e}); sign in again"
                )));
            }
            Ok(Ok(token)) if !left.is_zero() => {
                resolved.timeout_ms = Some((left.as_millis() as u64).max(1));
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
                                        .push(sendito_core::resolve::HeaderOrigin::Jar);
                                }
                                (None, None) => {}
                            }
                        }
                        Err(e) => resolved.notes.push(format!("cookie jar: {e}")),
                    }
                }
                sent = send_http(&resolved, ctx, cookies_on.then_some(&jar_hops)).await;
            }
            _ => sent = Err(sendito_http::HttpError::Timeout(budget.as_millis())),
        }
    };
    if let Ok(mut hop_secrets) = jar_hops.secrets.lock() {
        cookie_secrets.append(&mut hop_secrets);
    }
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
            let mut assert_secrets = http_secrets(req, &scope);
            assert_secrets.extend(cookie_secrets.iter().cloned());
            scrub_asserts(&mut results.asserts, &assert_secrets);
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
            write_history(ctx, &entry).await;
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
            write_history(ctx, &entry).await;
            (Err(msg), Doc::default())
        }
    };
    // `resolved` is only shown from here on (the Headers tab lists the
    // request as sent): a secret in the query or a header is masked.
    let shown = http_secrets(req, &scope);
    resolved.url = sendito_core::vars::scrub(&resolved.url, &shown);
    for (_, v) in &mut resolved.headers {
        *v = sendito_core::vars::scrub(v, &shown);
    }
    for n in &mut resolved.notes {
        *n = sendito_core::vars::scrub(n, &shown);
    }
    HttpOutcome {
        resolved,
        response,
        doc,
        body_size,
        scripts: results,
        scope_errors,
    }
}
