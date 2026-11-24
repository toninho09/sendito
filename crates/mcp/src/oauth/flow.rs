//! The authorization code flow with PKCE, token exchange and refresh.

use super::*;

/// A PKCE pair (RFC 7636): a random verifier and its S256 challenge.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn pkce() -> Result<Pkce, McpError> {
    let verifier = random_urlsafe(48)?;
    let digest = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest.as_ref());
    Ok(Pkce {
        verifier,
        challenge,
    })
}

pub(super) fn random_urlsafe(n: usize) -> Result<String, McpError> {
    use ring::rand::SecureRandom;
    let rng = ring::rand::SystemRandom::new();
    let mut bytes = vec![0u8; n];
    rng.fill(&mut bytes)
        .map_err(|_| McpError::Message("secure random number generation failed".into()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&bytes))
}

/// The authorization-code half of the flow: opens the browser through
/// `opener`, waits `wait` for the redirect on a loopback listener and hands
/// back what the token exchange needs.
pub struct Granted {
    pub code: String,
    pub verifier: String,
    pub redirect_uri: String,
    pub resource: Option<String>,
}

pub async fn authorize(
    endpoints: &Endpoints,
    resource_url: &str,
    client_id: &str,
    scopes: &[String],
    wait: Duration,
    opener: &(dyn Fn(&str) -> bool + Sync),
    note: &(dyn Fn(String) + Sync),
) -> Result<Granted, McpError> {
    authorize_with_resource(
        endpoints,
        Some(resource_url),
        client_id,
        scopes,
        wait,
        opener,
        note,
    )
    .await
}

pub(super) async fn authorize_with_resource(
    endpoints: &Endpoints,
    resource_url: Option<&str>,
    client_id: &str,
    scopes: &[String],
    wait: Duration,
    opener: &(dyn Fn(&str) -> bool + Sync),
    note: &(dyn Fn(String) + Sync),
) -> Result<Granted, McpError> {
    validate_oauth_url(&endpoints.authorize)?;
    let authorize_url = reqwest::Url::parse(&endpoints.authorize)
        .map_err(|_| McpError::Message("invalid OAuth endpoint URL".into()))?;
    if authorize_url.query_pairs().any(|(key, _)| {
        matches!(
            key.as_ref(),
            "response_type"
                | "client_id"
                | "redirect_uri"
                | "resource"
                | "state"
                | "code_challenge"
                | "code_challenge_method"
                | "scope"
        )
    }) {
        return Err(McpError::Message(
            "OAuth authorize_url contains reserved parameters".into(),
        ));
    }
    if let Some(resource_url) = resource_url {
        validate_oauth_url(resource_url)?;
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?;
    let port = listener
        .local_addr()
        .map_err(|e| McpError::Transport(e.to_string()))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let pk = pkce()?;
    let state = random_urlsafe(16)?;
    let mut params: Vec<(String, String)> = vec![
        ("response_type".into(), "code".into()),
        ("client_id".into(), client_id.to_string()),
        ("redirect_uri".into(), redirect_uri.clone()),
    ];
    if let Some(resource_url) = resource_url {
        params.push(("resource".into(), resource_url.to_string()));
    }
    params.extend([
        ("state".into(), state.clone()),
        ("code_challenge".into(), pk.challenge.clone()),
        ("code_challenge_method".into(), "S256".into()),
    ]);
    if !scopes.is_empty() {
        params.push(("scope".into(), scopes.join(" ")));
    }
    let separator = if endpoints.authorize.contains('?') {
        "&"
    } else {
        "?"
    };
    let url = format!("{}{separator}{}", endpoints.authorize, form_encode(&params));
    note(format!("authorization URL:\n  {url}"));
    if opener(&url) {
        note(format!(
            "browser opened; waiting for the redirect ({wait:?})"
        ));
    } else {
        note(format!(
            "open the URL above; waiting for the redirect ({wait:?})"
        ));
    }
    let granted = tokio::time::timeout(wait, async {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                continue;
            };
            let mut buf = [0u8; 4096];
            // Browsers open spare connections that send nothing: drop them
            // after a short wait, so the real redirect is served.
            let n = tokio::time::timeout(
                Duration::from_secs(2),
                tokio::io::AsyncReadExt::read(&mut sock, &mut buf),
            )
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or(0);
            if n == 0 {
                continue;
            }
            let req = String::from_utf8_lossy(&buf[..n]).into_owned();
            let target = req.split(' ').nth(1).unwrap_or_default().to_string();
            let query = target.split_once('?').map(|(_, q)| q.to_string());
            let _ = tokio::io::AsyncWriteExt::write_all(
                &mut sock,
                b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n\
                  Sendito: sign-in complete, you can close this tab.\n",
            )
            .await;
            let Some(q) = query else { continue };
            let map = parse_query(&q);
            if map.get("state").map(String::as_str) != Some(state.as_str()) {
                return Err(McpError::Message(
                    "the redirect state does not match".into(),
                ));
            }
            if let Some(err) = map.get("error") {
                return Err(McpError::Message(format!(
                    "the server denied access: {err}"
                )));
            }
            return Ok(Granted {
                code: map
                    .get("code")
                    .cloned()
                    .ok_or_else(|| McpError::Message("the redirect carried no code".into()))?,
                verifier: pk.verifier.clone(),
                redirect_uri: redirect_uri.clone(),
                resource: resource_url.map(str::to_string),
            });
        }
    })
    .await;
    match granted {
        Ok(Ok(g)) => Ok(g),
        Ok(Err(e)) => Err(e),
        Err(_) => Err(McpError::Timeout(wait)),
    }
}

/// Exchanges the code for tokens.
pub async fn exchange(
    endpoints: &Endpoints,
    granted: Granted,
    client_id: &str,
    client_secret: Option<&str>,
    net: &Network,
) -> Result<Tokens, McpError> {
    let mut form: Vec<(String, String)> = vec![
        ("grant_type".into(), "authorization_code".into()),
        ("code".into(), granted.code),
        ("redirect_uri".into(), granted.redirect_uri),
    ];
    if let Some(resource) = granted.resource {
        form.push(("resource".into(), resource));
    }
    form.extend([
        ("client_id".into(), client_id.to_string()),
        ("code_verifier".into(), granted.verifier),
    ]);
    if let Some(secret) = client_secret {
        form.push(("client_secret".into(), secret.to_string()));
    }
    token_request(endpoints, &form, client_id, net).await
}

/// Refreshes an access token; servers may rotate the refresh token.
pub async fn refresh_tokens(
    endpoints: &Endpoints,
    resource_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
    net: &Network,
) -> Result<Tokens, McpError> {
    refresh_with_resource(
        endpoints,
        Some(resource_url),
        client_id,
        client_secret,
        refresh_token,
        net,
    )
    .await
}

pub(super) async fn refresh_with_resource(
    endpoints: &Endpoints,
    resource_url: Option<&str>,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
    net: &Network,
) -> Result<Tokens, McpError> {
    let mut form: Vec<(String, String)> = vec![
        ("grant_type".into(), "refresh_token".into()),
        ("refresh_token".into(), refresh_token.to_string()),
    ];
    if let Some(resource_url) = resource_url {
        form.push(("resource".into(), resource_url.to_string()));
    }
    form.push(("client_id".into(), client_id.to_string()));
    if let Some(secret) = client_secret {
        form.push(("client_secret".into(), secret.to_string()));
    }
    let mut t = token_request(endpoints, &form, client_id, net).await?;
    if t.refresh.is_none() {
        t.refresh = Some(refresh_token.to_string());
    }
    Ok(t)
}

pub(super) async fn token_request(
    endpoints: &Endpoints,
    form: &[(String, String)],
    client_id: &str,
    net: &Network,
) -> Result<Tokens, McpError> {
    validate_oauth_url(&endpoints.token)?;
    let resp = client(net)?
        .post(&endpoints.token)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form_encode(form))
        .send()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?;
    let status = resp.status();
    let body = resp
        .text()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?;
    let v: Value = serde_json::from_str(&body).map_err(|_| {
        McpError::Transport(format!(
            "HTTP {status}: the token endpoint answered no JSON"
        ))
    })?;
    if !status.is_success() {
        return Err(McpError::Transport(format!(
            "HTTP {status}: {}",
            v.get("error")
                .and_then(Value::as_str)
                .unwrap_or("token request failed")
        )));
    }
    let access = v
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Message("the token endpoint sent no access_token".into()))?;
    let expires_at = v
        .get("expires_in")
        .and_then(Value::as_u64)
        .map(|s| unix_now().saturating_add(s));
    Ok(Tokens {
        access: access.to_string(),
        refresh: v
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_at,
        client_id: client_id.to_string(),
        client_secret: None,
        bound_to: None,
    })
}

/// The whole interactive login, as the CLI runs it. Returns the tokens to
/// persist (they carry the client id).
pub async fn login(
    resource_url: &str,
    settings: &OAuthSettings,
    wait: Duration,
    opener: &(dyn Fn(&str) -> bool + Sync),
    note: &(dyn Fn(String) + Sync),
) -> Result<Tokens, McpError> {
    let endpoints = discover_endpoints(resource_url, settings).await?;
    let (client_id, client_secret) = match &settings.client_id {
        Some(id) => (id.clone(), settings.client_secret.clone()),
        None => {
            let url = endpoints
                .registration
                .clone()
                .or_else(|| settings.registration_url.clone());
            let Some(url) = url else {
                return Err(McpError::Message(
                    "no client_id configured and the server offers no registration".into(),
                ));
            };
            let (id, secret) = register(&url, &settings.network).await?;
            (id, secret.or_else(|| settings.client_secret.clone()))
        }
    };
    let granted = authorize(
        &endpoints,
        resource_url,
        &client_id,
        &settings.scopes,
        wait,
        opener,
        note,
    )
    .await?;
    let mut tokens = exchange(
        &endpoints,
        granted,
        &client_id,
        client_secret.as_deref(),
        &settings.network,
    )
    .await?;
    tokens.client_secret = client_secret;
    tokens.bound_to = Some(resource_url.to_string());
    note(format!(
        "signed in (expires {})",
        tokens
            .expires_at
            .map(|t| t.to_string())
            .unwrap_or_else(|| "never".into())
    ));
    Ok(tokens)
}

/// OAuth authorization-code login for a normal HTTP API. Unlike MCP, it
/// does not discover resource metadata or send the RFC 8707 resource parameter.
pub async fn login_http(
    settings: &OAuthSettings,
    wait: Duration,
    opener: &(dyn Fn(&str) -> bool + Sync),
    note: &(dyn Fn(String) + Sync),
) -> Result<Tokens, McpError> {
    let endpoints = http_endpoints(settings)?;
    let client_id = settings
        .client_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::Message("OAuth client_id is required".into()))?;
    let granted = authorize_with_resource(
        &endpoints,
        None,
        client_id,
        &settings.scopes,
        wait,
        opener,
        note,
    )
    .await?;
    let mut tokens = exchange(
        &endpoints,
        granted,
        client_id,
        settings.client_secret.as_deref(),
        &settings.network,
    )
    .await?;
    tokens.client_secret = settings.client_secret.clone();
    tokens.bound_to = Some(endpoints.token.clone());
    Ok(tokens)
}

pub(super) fn http_endpoints(settings: &OAuthSettings) -> Result<Endpoints, McpError> {
    let authorize = settings
        .authorize_url
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::Message("OAuth authorize_url is required".into()))?;
    let token = settings
        .token_url
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::Message("OAuth token_url is required".into()))?;
    validate_oauth_url(&authorize)?;
    validate_oauth_url(&token)?;
    Ok(Endpoints {
        authorize,
        token,
        registration: None,
    })
}
