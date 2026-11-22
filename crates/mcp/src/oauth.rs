//! OAuth 2.1 for HTTP MCP servers: authorization code + PKCE, with metadata
//! discovery (RFC 9728 / RFC 8414) and dynamic client registration (RFC 7591).
//!
//! The browser step is injectable: `login` takes an opener closure (the CLI
//! opens the default browser; tests pass a no-op) and always reports the URL,
//! so the flow also works over SSH by pasting the URL elsewhere. Tokens live
//! with the caller — see [`TokenStore`] and [`RefreshingProvider`].

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use serde_json::{Value, json};
use sha2::Digest;

use crate::{McpError, TokenProvider};

/// OAuth settings from `[mcp.servers.<name>.auth]` (type = "oauth").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthSettings {
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub registration_url: Option<String>,
    pub scopes: Vec<String>,
}

/// The authorization server endpoints a login needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoints {
    pub authorize: String,
    pub token: String,
    pub registration: Option<String>,
}

/// Access and refresh tokens, serialized into the token store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tokens {
    pub access: String,
    pub refresh: Option<String>,
    /// Unix seconds; `None` when the server sends no `expires_in`.
    pub expires_at: Option<u64>,
    /// The client id the tokens were issued to (needed to refresh).
    pub client_id: String,
    /// A dynamically registered client secret, stored with the tokens.
    pub client_secret: Option<String>,
}

const EXPIRY_SLACK: u64 = 30;

impl Tokens {
    pub fn expired(&self, now: u64) -> bool {
        self.expires_at
            .is_some_and(|t| t <= now.saturating_add(EXPIRY_SLACK))
    }

    pub fn to_json(&self) -> String {
        json!({
            "access": self.access,
            "refresh": self.refresh,
            "expires_at": self.expires_at,
            "client_id": self.client_id,
            "client_secret": self.client_secret,
        })
        .to_string()
    }

    pub fn from_json(text: &str) -> Option<Self> {
        let v: Value = serde_json::from_str(text).ok()?;
        Some(Self {
            access: v.get("access")?.as_str()?.to_string(),
            refresh: v.get("refresh").and_then(Value::as_str).map(str::to_string),
            expires_at: v.get("expires_at").and_then(Value::as_u64),
            client_id: v.get("client_id")?.as_str()?.to_string(),
            client_secret: v
                .get("client_secret")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

/// Where tokens are kept: the OS keychain (via an adapter) or a test double.
pub trait TokenStore: Send + Sync {
    fn get(&self, account: &str) -> Result<Option<String>, String>;
    fn set(&self, account: &str, value: &str) -> Result<(), String>;
    fn delete(&self, account: &str) -> Result<(), String>;
}

fn client() -> Result<reqwest::Client, McpError> {
    let tls = sendito_http::tls_config(true, &[b"h2", b"http/1.1"], &Default::default())
        .map_err(McpError::Transport)?;
    reqwest::Client::builder()
        .tls_backend_preconfigured(tls)
        // Never forward authorization codes, refresh tokens or client secrets
        // to an endpoint selected by an HTTP redirect.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| McpError::Transport(e.to_string()))
}

/// OAuth secrets may only travel over TLS, except to a local test server.
pub fn validate_oauth_url(url: &str) -> Result<(), McpError> {
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| McpError::Message("invalid OAuth endpoint URL".into()))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(McpError::Message(
            "OAuth endpoint URLs must not contain credentials".into(),
        ));
    }
    if parsed.fragment().is_some() {
        return Err(McpError::Message(
            "OAuth URLs must not contain fragments".into(),
        ));
    }
    let local = parsed.host_str().is_some_and(|host| {
        host.eq_ignore_ascii_case("localhost")
            || host
                .parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
    });
    if parsed.scheme() != "https" && !(parsed.scheme() == "http" && local) {
        return Err(McpError::Message(
            "OAuth endpoints require HTTPS except on loopback".into(),
        ));
    }
    Ok(())
}

/// Resolves the endpoints: explicit settings win, otherwise the discovery
/// chain the MCP spec defines (401 + `WWW-Authenticate` → protected resource
/// metadata → authorization server metadata).
pub async fn discover_endpoints(
    resource_url: &str,
    s: &OAuthSettings,
) -> Result<Endpoints, McpError> {
    validate_oauth_url(resource_url)?;
    if let (Some(authorize), Some(token)) = (&s.authorize_url, &s.token_url) {
        validate_oauth_url(authorize)?;
        validate_oauth_url(token)?;
        if let Some(registration) = &s.registration_url {
            validate_oauth_url(registration)?;
        }
        return Ok(Endpoints {
            authorize: authorize.clone(),
            token: token.clone(),
            registration: s.registration_url.clone(),
        });
    }
    let c = client()?;
    // Step 1: the resource points at its metadata.
    let resp = c
        .post(resource_url)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body("{}")
        .send()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?;
    let meta_url = resp
        .headers()
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .and_then(resource_metadata_from_challenge)
        .map(str::to_string)
        .ok_or_else(|| {
            McpError::Message("the server sent no WWW-Authenticate resource_metadata".into())
        })?;
    // Step 2: the resource metadata names the authorization server.
    validate_oauth_url(&meta_url)?;
    let v: Value = c
        .get(&meta_url)
        .send()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?
        .json()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?;
    let resource = v
        .get("resource")
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Message("resource metadata has no resource identifier".into()))?;
    if reqwest::Url::parse(resource).ok() != reqwest::Url::parse(resource_url).ok() {
        return Err(McpError::Message(
            "resource metadata identifies a different resource".into(),
        ));
    }
    let issuer = v
        .get("authorization_servers")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .and_then(Value::as_str)
        .ok_or_else(|| McpError::Message("no authorization_servers".into()))?;
    // Step 3: the authorization server metadata has the endpoints.
    validate_oauth_url(issuer)?;
    let v: Value = c
        .get(well_known(issuer))
        .send()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?
        .json()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?;
    if v.get("issuer").and_then(Value::as_str) != Some(issuer) {
        return Err(McpError::Message(
            "authorization metadata issuer does not match".into(),
        ));
    }
    let get = |k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
    let endpoints = Endpoints {
        authorize: get("authorization_endpoint")
            .ok_or_else(|| McpError::Message("no authorization_endpoint".into()))?,
        token: get("token_endpoint")
            .ok_or_else(|| McpError::Message("no token_endpoint".into()))?,
        registration: get("registration_endpoint"),
    };
    validate_oauth_url(&endpoints.authorize)?;
    validate_oauth_url(&endpoints.token)?;
    if let Some(registration) = &endpoints.registration {
        validate_oauth_url(registration)?;
    }
    Ok(endpoints)
}

fn resource_metadata_from_challenge(challenge: &str) -> Option<&str> {
    let value = challenge.split_once("resource_metadata=")?.1.trim_start();
    if let Some(quoted) = value.strip_prefix('"') {
        quoted.split_once('"').map(|(url, _)| url)
    } else {
        Some(value.split([',', ' ']).next()?)
    }
}

/// Insert the well-known suffix before the issuer path (RFC 8414 §3.1).
pub fn well_known(issuer: &str) -> String {
    let Ok(mut url) = reqwest::Url::parse(issuer) else {
        return issuer.to_string();
    };
    let path = url.path().trim_end_matches('/');
    url.set_path(&format!("/.well-known/oauth-authorization-server{path}"));
    url.into()
}

/// Dynamic client registration (RFC 7591). The loopback redirect is
/// registered without a port: RFC 8252 §7.3 lets the port vary per login.
pub async fn register(registration_url: &str) -> Result<(String, Option<String>), McpError> {
    validate_oauth_url(registration_url)?;
    let v: Value = client()?
        .post(registration_url)
        .json(&json!({
            "client_name": "sendito",
            "redirect_uris": ["http://127.0.0.1/callback"],
        }))
        .send()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?
        .error_for_status()
        .map_err(|e| McpError::Transport(e.to_string()))?
        .json()
        .await
        .map_err(|e| McpError::Transport(e.to_string()))?;
    Ok((
        v.get("client_id")
            .and_then(Value::as_str)
            .ok_or_else(|| McpError::Message("registration sent no client_id".into()))?
            .to_string(),
        v.get("client_secret")
            .and_then(Value::as_str)
            .map(str::to_string),
    ))
}

/// A PKCE pair (RFC 7636): a random verifier and its S256 challenge.
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

pub fn pkce() -> Result<Pkce, McpError> {
    let verifier = random_urlsafe(48)?;
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    Ok(Pkce {
        verifier,
        challenge,
    })
}

fn random_urlsafe(n: usize) -> Result<String, McpError> {
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

async fn authorize_with_resource(
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
            let n = tokio::io::AsyncReadExt::read(&mut sock, &mut buf)
                .await
                .unwrap_or(0);
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
    token_request(endpoints, &form, client_id).await
}

/// Refreshes an access token; servers may rotate the refresh token.
pub async fn refresh_tokens(
    endpoints: &Endpoints,
    resource_url: &str,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
) -> Result<Tokens, McpError> {
    refresh_with_resource(
        endpoints,
        Some(resource_url),
        client_id,
        client_secret,
        refresh_token,
    )
    .await
}

async fn refresh_with_resource(
    endpoints: &Endpoints,
    resource_url: Option<&str>,
    client_id: &str,
    client_secret: Option<&str>,
    refresh_token: &str,
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
    let mut t = token_request(endpoints, &form, client_id).await?;
    if t.refresh.is_none() {
        t.refresh = Some(refresh_token.to_string());
    }
    Ok(t)
}

async fn token_request(
    endpoints: &Endpoints,
    form: &[(String, String)],
    client_id: &str,
) -> Result<Tokens, McpError> {
    validate_oauth_url(&endpoints.token)?;
    let resp = client()?
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
            let (id, secret) = register(&url).await?;
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
    let mut tokens = exchange(&endpoints, granted, &client_id, client_secret.as_deref()).await?;
    tokens.client_secret = client_secret;
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
    )
    .await?;
    tokens.client_secret = settings.client_secret.clone();
    Ok(tokens)
}

fn http_endpoints(settings: &OAuthSettings) -> Result<Endpoints, McpError> {
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

/// A [`TokenProvider`] backed by a [`TokenStore`]: serves the stored access
/// token, refreshes it when missing, expired or forced (after a 401), and
/// saves what comes back.
pub struct RefreshingProvider {
    pub store: Arc<dyn TokenStore>,
    pub account: String,
    pub resource_url: Option<String>,
    pub settings: OAuthSettings,
    endpoints: Mutex<Option<Endpoints>>,
}

impl RefreshingProvider {
    pub fn new(
        store: Arc<dyn TokenStore>,
        account: String,
        resource_url: String,
        settings: OAuthSettings,
    ) -> Self {
        Self {
            store,
            account,
            resource_url: Some(resource_url),
            settings,
            endpoints: Mutex::new(None),
        }
    }

    pub fn new_http(store: Arc<dyn TokenStore>, account: String, settings: OAuthSettings) -> Self {
        Self {
            store,
            account,
            resource_url: None,
            settings,
            endpoints: Mutex::new(None),
        }
    }

    async fn endpoints(&self) -> Result<Endpoints, McpError> {
        if let Some(e) = self.endpoints.lock().ok().and_then(|g| g.clone()) {
            return Ok(e);
        }
        let e = match &self.resource_url {
            Some(url) => discover_endpoints(url, &self.settings).await?,
            None => http_endpoints(&self.settings)?,
        };
        if let Ok(mut g) = self.endpoints.lock() {
            *g = Some(e.clone());
        }
        Ok(e)
    }
}

impl TokenProvider for RefreshingProvider {
    fn token<'a>(
        &'a self,
        force_refresh: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, McpError>> + Send + 'a>>
    {
        Box::pin(async move {
            let store = self.store.clone();
            let account = self.account.clone();
            let stored = tokio::task::spawn_blocking(move || store.get(&account))
                .await
                .map_err(|_| McpError::Transport("keychain task failed".into()))?
                .map_err(McpError::Transport)?
                .and_then(|s| Tokens::from_json(&s));
            let Some(mut tokens) = stored else {
                return Err(McpError::Message(if self.resource_url.is_some() {
                    "not signed in (run `sendito mcp login`)".into()
                } else {
                    "not signed in (run `sendito auth login`)".into()
                }));
            };
            if !force_refresh && !tokens.expired(unix_now()) {
                return Ok(tokens.access);
            }
            let Some(refresh) = tokens.refresh.clone() else {
                return Err(McpError::Message("the token expired; sign in again".into()));
            };
            let endpoints = self.endpoints().await?;
            let client_secret = tokens
                .client_secret
                .clone()
                .or_else(|| self.settings.client_secret.clone());
            tokens = refresh_with_resource(
                &endpoints,
                self.resource_url.as_deref(),
                &tokens.client_id,
                client_secret.as_deref(),
                &refresh,
            )
            .await?;
            tokens.client_secret = client_secret;
            let store = self.store.clone();
            let account = self.account.clone();
            let serialized = tokens.to_json();
            tokio::task::spawn_blocking(move || store.set(&account, &serialized))
                .await
                .map_err(|_| McpError::Transport("keychain task failed".into()))?
                .map_err(McpError::Transport)?;
            Ok(tokens.access)
        })
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `application/x-www-form-urlencoded` for the few pairs we send.
pub fn form_encode(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", urlencode(k), urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Parses `a=1&b=2` (no repeated keys needed).
fn parse_query(q: &str) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for pair in q.split('&') {
        let Some((k, v)) = pair.split_once('=') else {
            continue;
        };
        out.insert(urldecode(k), urldecode(v));
    }
    out
}

fn urldecode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(b) = u8::from_str_radix(hex, 16) {
                    out.push(b);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_inserts_before_path() {
        assert_eq!(
            well_known("https://as.example.com"),
            "https://as.example.com/.well-known/oauth-authorization-server"
        );
        assert_eq!(
            well_known("https://as.example.com/oauth2/"),
            "https://as.example.com/.well-known/oauth-authorization-server/oauth2"
        );
        assert_eq!(
            well_known("https://as.example.com?a=1"),
            "https://as.example.com/.well-known/oauth-authorization-server?a=1"
        );
    }

    #[test]
    fn pkce_challenge_is_the_hashed_verifier() -> Result<(), McpError> {
        let pk = pkce()?;
        assert_eq!(pk.verifier.len(), 64);
        let digest = sha2::Sha256::digest(pk.verifier.as_bytes());
        let expect = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(pk.challenge, expect);
        Ok(())
    }

    #[test]
    fn metadata_challenge_handles_following_parameters() {
        assert_eq!(
            resource_metadata_from_challenge(
                "Bearer resource_metadata=\"https://example.com/meta\", scope=\"read\""
            ),
            Some("https://example.com/meta")
        );
        assert_eq!(
            resource_metadata_from_challenge(
                "Bearer resource_metadata=https://example.com/meta, scope=read"
            ),
            Some("https://example.com/meta")
        );
    }

    #[test]
    fn oauth_urls_require_tls_except_loopback() {
        assert!(validate_oauth_url("https://example.com/token").is_ok());
        assert!(validate_oauth_url("http://127.0.0.1:1234/token").is_ok());
        assert!(validate_oauth_url("http://localhost:1234/token").is_ok());
        assert!(validate_oauth_url("http://example.com/token").is_err());
        assert!(validate_oauth_url("https://user:password@example.com/token").is_err());
    }

    #[test]
    fn tokens_round_trip_and_expiry() {
        let t = Tokens {
            access: "a".into(),
            refresh: Some("r".into()),
            expires_at: Some(100),
            client_id: "cli".into(),
            client_secret: Some("client-secret".into()),
        };
        let back = Tokens::from_json(&t.to_json());
        assert_eq!(back.as_ref(), Some(&t));
        assert!(t.expired(71));
        assert!(!t.expired(69));
    }

    #[test]
    fn form_and_query_codecs() {
        assert_eq!(form_encode(&[("a b".into(), "x&y".into())]), "a%20b=x%26y");
        let m = parse_query("code=c+1&state=%2Fz");
        assert_eq!(m.get("code").map(String::as_str), Some("c 1"));
        assert_eq!(m.get("state").map(String::as_str), Some("/z"));
    }
}
