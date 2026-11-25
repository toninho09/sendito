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
    match token_call(&endpoints.token, form, client_id, net).await? {
        TokenReply::Tokens(t) => Ok(t),
        TokenReply::Refused { status, error } => Err(McpError::Transport(format!(
            "HTTP {status}: {}",
            error.as_deref().unwrap_or("token request failed")
        ))),
    }
}

/// What a token endpoint answered.
enum TokenReply {
    Tokens(Tokens),
    /// An error status, with the OAuth `error` code when the body has one.
    Refused {
        status: reqwest::StatusCode,
        error: Option<String>,
    },
}

async fn token_call(
    token_url: &str,
    form: &[(String, String)],
    client_id: &str,
    net: &Network,
) -> Result<TokenReply, McpError> {
    validate_oauth_url(token_url)?;
    let resp = client(net)?
        .post(token_url)
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
        return Ok(TokenReply::Refused {
            status,
            error: v.get("error").and_then(Value::as_str).map(str::to_string),
        });
    }
    let access = v
        .get("access_token")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Message("the token endpoint sent no access_token".into()))?;
    let expires_at = v
        .get("expires_in")
        .and_then(Value::as_u64)
        .map(|s| unix_now().saturating_add(s));
    Ok(TokenReply::Tokens(Tokens {
        access: access.to_string(),
        refresh: v
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string),
        expires_at,
        client_id: client_id.to_string(),
        client_secret: None,
        bound_to: None,
    }))
}

/// The client id an HTTP identity must have.
fn required_client_id(settings: &OAuthSettings) -> Result<&str, McpError> {
    settings
        .client_id
        .as_deref()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::Message("OAuth client_id is required".into()))
}

/// The form fields every token request of an HTTP identity ends with: the
/// scopes, the client id and the client secret.
fn client_fields(settings: &OAuthSettings, client_id: &str) -> Vec<(String, String)> {
    let mut form = Vec::new();
    if !settings.scopes.is_empty() {
        form.push(("scope".into(), settings.scopes.join(" ")));
    }
    form.push(("client_id".into(), client_id.to_string()));
    if let Some(secret) = &settings.client_secret {
        form.push(("client_secret".into(), secret.clone()));
    }
    form
}

/// A token for the client credentials or the password grant, from the token
/// endpoint alone: no person signs in.
pub async fn fetch_token(settings: &OAuthSettings) -> Result<Tokens, McpError> {
    let endpoints = http_endpoints(settings)?;
    let client_id = required_client_id(settings)?;
    let mut form: Vec<(String, String)> = match settings.grant {
        Grant::ClientCredentials => vec![("grant_type".into(), "client_credentials".into())],
        Grant::Password => {
            let username = settings
                .username
                .clone()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| McpError::Message("the password grant needs a username".into()))?;
            vec![
                ("grant_type".into(), "password".into()),
                ("username".into(), username),
                (
                    "password".into(),
                    settings.password.clone().unwrap_or_default(),
                ),
            ]
        }
        _ => {
            return Err(McpError::Message(
                "this OAuth grant needs a sign-in (sendito auth login)".into(),
            ));
        }
    };
    form.extend(client_fields(settings, client_id));
    let mut tokens = token_request(&endpoints, &form, client_id, &settings.network).await?;
    tokens.client_secret = settings.client_secret.clone();
    tokens.bound_to = Some(endpoints.token.clone());
    Ok(tokens)
}

/// The device authorization grant (RFC 8628): asks for a code, shows it with
/// `note`, opens the verification page with `opener`, then polls the token
/// endpoint until the user accepts, refuses or the code expires.
pub async fn device_login(
    settings: &OAuthSettings,
    opener: &(dyn Fn(&str) -> bool + Sync),
    note: &(dyn Fn(String) + Sync),
) -> Result<Tokens, McpError> {
    let token_url = settings
        .token_url
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::Message("OAuth token_url is required".into()))?;
    let device_url = settings
        .device_authorization_url
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::Message("OAuth device_authorization_url is required".into()))?;
    validate_oauth_url(&device_url)?;
    let client_id = required_client_id(settings)?;
    let resp = client(&settings.network)?
        .post(&device_url)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(form_encode(&client_fields(settings, client_id)))
        .send()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?;
    let status = resp.status();
    let v: Value = resp.json().await.map_err(|_| {
        McpError::Transport(format!(
            "HTTP {status}: the device endpoint answered no JSON"
        ))
    })?;
    if !status.is_success() {
        return Err(McpError::Transport(format!(
            "HTTP {status}: {}",
            v.get("error")
                .and_then(Value::as_str)
                .unwrap_or("device authorization failed")
        )));
    }
    let field = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let (Some(device_code), Some(user_code), Some(uri)) = (
        field("device_code"),
        field("user_code"),
        field("verification_uri"),
    ) else {
        return Err(McpError::Message(
            "the device endpoint sent no device_code, user_code or verification_uri".into(),
        ));
    };
    let complete = field("verification_uri_complete");
    let expires_in = v.get("expires_in").and_then(Value::as_u64).unwrap_or(600);
    let mut interval = v
        .get("interval")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .max(1);
    note(format!(
        "device sign-in:\n  Open {uri} and enter the code {user_code}. It expires in {} min.",
        expires_in.div_ceil(60)
    ));
    let _ = opener(complete.as_deref().unwrap_or(&uri));
    let deadline = std::time::Instant::now() + Duration::from_secs(expires_in);
    let mut form = vec![
        (
            "grant_type".to_string(),
            "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        ),
        ("device_code".into(), device_code),
        ("client_id".into(), client_id.to_string()),
    ];
    if let Some(secret) = &settings.client_secret {
        form.push(("client_secret".into(), secret.clone()));
    }
    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if std::time::Instant::now() >= deadline {
            return Err(McpError::Message(
                "the device code expired; sign in again".into(),
            ));
        }
        match token_call(&token_url, &form, client_id, &settings.network).await? {
            TokenReply::Tokens(mut tokens) => {
                tokens.client_secret = settings.client_secret.clone();
                tokens.bound_to = Some(token_url);
                return Ok(tokens);
            }
            TokenReply::Refused { error, status } => match error.as_deref() {
                Some("authorization_pending") => {}
                Some("slow_down") => interval += 5,
                Some("access_denied") => {
                    return Err(McpError::Message("the sign-in was refused".into()));
                }
                Some("expired_token") => {
                    return Err(McpError::Message(
                        "the device code expired; sign in again".into(),
                    ));
                }
                other => {
                    return Err(McpError::Transport(format!(
                        "HTTP {status}: {}",
                        other.unwrap_or("token request failed")
                    )));
                }
            },
        }
    }
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
    match settings.grant {
        Grant::AuthorizationCode => {}
        Grant::DeviceCode => return device_login(settings, opener, note).await,
        // No sign-in: getting a token now checks the credentials.
        Grant::ClientCredentials | Grant::Password => return fetch_token(settings).await,
    }
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
    // Only the authorization code grant uses the authorize endpoint.
    let authorize = match settings.grant {
        Grant::AuthorizationCode => settings
            .authorize_url
            .clone()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| McpError::Message("OAuth authorize_url is required".into()))?,
        _ => String::new(),
    };
    let token = settings
        .token_url
        .clone()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| McpError::Message("OAuth token_url is required".into()))?;
    if !authorize.is_empty() {
        validate_oauth_url(&authorize)?;
    }
    validate_oauth_url(&token)?;
    Ok(Endpoints {
        authorize,
        token,
        registration: None,
    })
}
