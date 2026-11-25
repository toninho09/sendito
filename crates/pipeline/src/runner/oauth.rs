//! OAuth settings, token accounts and the MCP sign-in flow.

use super::*;

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
    network: sendito_mcp::oauth::Network,
) -> Result<sendito_mcp::oauth::OAuthSettings, String> {
    let render = |text: &str, field: &str, allow_secret: bool| {
        render_oauth_field(scope, text, field, allow_secret)
    };
    use sendito_mcp::oauth::Grant;
    use sendito_store::config::OAuthGrant;
    let opt =
        |v: &Option<String>, field: &str| v.as_deref().map(|s| render(s, field, true)).transpose();
    Ok(sendito_mcp::oauth::OAuthSettings {
        grant: match identity.grant {
            OAuthGrant::AuthorizationCode => Grant::AuthorizationCode,
            OAuthGrant::ClientCredentials => Grant::ClientCredentials,
            OAuthGrant::Password => Grant::Password,
            OAuthGrant::DeviceCode => Grant::DeviceCode,
        },
        username: opt(&identity.username, "username")?,
        password: opt(&identity.password, "password")?,
        device_authorization_url: Some(render(
            &identity.device_authorization_url,
            "device_authorization_url",
            false,
        )?),
        client_id: Some(render(&identity.client_id, "client_id", false)?),
        client_secret: opt(&identity.client_secret, "client_secret")?,
        authorize_url: Some(render(&identity.authorize_url, "authorize_url", false)?),
        token_url: Some(render(&identity.token_url, "token_url", false)?),
        scopes: identity
            .scopes
            .iter()
            .map(|s| render(s, "scope", false))
            .collect::<Result<_, _>>()?,
        registration_url: None,
        network,
    })
}

/// Renders one OAuth setting. Only the client secret, the username and the
/// password may hold a secret.
pub(super) fn render_oauth_field(
    scope: &Scope,
    text: &str,
    field: &str,
    allow_secret: bool,
) -> Result<String, String> {
    let r = scope.render(text);
    if !r.unresolved.is_empty() || r.text.contains("{{") {
        return Err(format!("OAuth {field} contains unresolved variables"));
    }
    if r.secret && !allow_secret {
        return Err(format!("OAuth {field} must not contain secret variables"));
    }
    Ok(r.text)
}

/// The OAuth settings of an MCP server, with `{{variables}}` rendered. A
/// server without an OAuth table gets empty settings: discovery and dynamic
/// client registration then supply everything.
pub fn mcp_oauth_settings(
    auth: Option<&sendito_store::config::McpServerAuth>,
    scope: &Scope,
    network: sendito_mcp::oauth::Network,
) -> Result<sendito_mcp::oauth::OAuthSettings, String> {
    let Some(sendito_store::config::McpServerAuth::Oauth {
        client_id,
        client_secret,
        authorize_url,
        token_url,
        registration_url,
        scopes,
    }) = auth
    else {
        return Ok(sendito_mcp::oauth::OAuthSettings {
            network,
            ..Default::default()
        });
    };
    let opt = |v: &Option<String>, field: &str, allow_secret: bool| {
        v.as_deref()
            .map(|t| render_oauth_field(scope, t, field, allow_secret))
            .transpose()
    };
    Ok(sendito_mcp::oauth::OAuthSettings {
        client_id: opt(client_id, "client_id", false)?,
        client_secret: opt(client_secret, "client_secret", true)?,
        authorize_url: opt(authorize_url, "authorize_url", false)?,
        token_url: opt(token_url, "token_url", false)?,
        registration_url: opt(registration_url, "registration_url", false)?,
        scopes: scopes
            .iter()
            .map(|t| render_oauth_field(scope, t, "scope", false))
            .collect::<Result<_, _>>()?,
        network,
        ..Default::default()
    })
}

/// Opens `url` in the default browser. Returns `false` when no browser starts.
pub fn open_browser(url: &str) -> bool {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// The interactive OAuth sign-in for an HTTP server of `[mcp.servers]`.
/// `scope` renders the URL and the OAuth settings.
/// Stores the tokens in the keychain. A server without an auth table works
/// too: the caller writes `type = "oauth"` to the config.
pub async fn mcp_login(
    info: &McpRunInfo,
    server: &str,
    scope: Scope,
    opener: &(dyn Fn(&str) -> bool + Sync),
    note: &(dyn Fn(String) + Sync),
) -> Result<(), String> {
    let Some(sendito_store::config::McpServer::Http(h)) = info.servers.get(server) else {
        return Err(format!(
            "server {server:?} is not an HTTP entry of [mcp.servers]"
        ));
    };
    if matches!(
        h.auth,
        Some(sendito_store::config::McpServerAuth::Bearer { .. })
    ) {
        return Err(format!(
            "server {server:?} uses a bearer token; set its auth to oauth first"
        ));
    }
    let settings = mcp_oauth_settings(h.auth.as_ref(), &scope, info.network())?;
    let url = render_oauth_field(&scope, &h.url, "server URL", false)?;
    // `Scope` is not `Sync`: let go of it before the first await.
    drop(scope);
    let tokens = sendito_mcp::oauth::login(&url, &settings, Duration::from_secs(300), opener, note)
        .await
        .map_err(|e| format!("login failed: {e}"))?;
    let keychain = info.keychain.clone();
    let account = mcp_token_account(&info.collection, server);
    tokio::task::spawn_blocking(move || keychain.set(&account, &tokens.to_json()))
        .await
        .map_err(|_| "keychain task failed".to_string())?
        .map_err(|e| format!("cannot store the tokens: {e}"))
}

/// Forgets the stored tokens of an MCP server.
pub async fn mcp_logout(info: &McpRunInfo, server: &str) -> Result<(), String> {
    let keychain = info.keychain.clone();
    let account = mcp_token_account(&info.collection, server);
    tokio::task::spawn_blocking(move || keychain.delete(&account))
        .await
        .map_err(|_| "keychain task failed".to_string())?
}
