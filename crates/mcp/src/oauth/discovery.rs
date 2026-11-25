//! Endpoint discovery, URL checks and dynamic client registration.

use super::*;

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
                .trim_start_matches('[')
                .trim_end_matches(']')
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
    let c = client(&s.network)?;
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

pub(super) fn resource_metadata_from_challenge(challenge: &str) -> Option<&str> {
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
pub async fn register(
    registration_url: &str,
    net: &Network,
) -> Result<(String, Option<String>), McpError> {
    validate_oauth_url(registration_url)?;
    let v: Value = client(net)?
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
