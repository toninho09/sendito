//! A token provider that refreshes the access token when it expires.

use super::*;

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

    pub(super) async fn endpoints(&self) -> Result<Endpoints, McpError> {
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

impl RefreshingProvider {
    /// The stored tokens, checked against the binding this provider expects.
    pub(super) async fn load(&self, want: Option<&String>) -> Result<Tokens, McpError> {
        let store = self.store.clone();
        let account = self.account.clone();
        let stored = tokio::task::spawn_blocking(move || store.get(&account))
            .await
            .map_err(|_| McpError::Transport("keychain task failed".into()))?
            .map_err(McpError::Transport)?
            .and_then(|s| Tokens::from_json(&s));
        let Some(tokens) = stored else {
            return Err(McpError::NeedsLogin(if self.resource_url.is_some() {
                "not signed in (run `sendito mcp login`)".into()
            } else {
                "not signed in (run `sendito auth login`)".into()
            }));
        };
        if let (Some(have), Some(want)) = (&tokens.bound_to, want)
            && have != want
        {
            return Err(McpError::NeedsLogin(format!(
                "the saved sign-in is for {have}, not {want}; sign in again"
            )));
        }
        Ok(tokens)
    }
}

/// One lock per token-store account, for the whole process: two requests that
/// need a refresh at the same time must not both spend the same refresh token
/// (a server that rotates refresh tokens rejects the second one).
pub(super) fn refresh_lock(account: &str) -> Arc<tokio::sync::Mutex<()>> {
    static LOCKS: std::sync::OnceLock<
        Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    > = std::sync::OnceLock::new();
    let locks = LOCKS.get_or_init(Default::default);
    match locks.lock() {
        Ok(mut map) => map.entry(account.to_string()).or_default().clone(),
        Err(_) => Arc::default(),
    }
}

impl TokenProvider for RefreshingProvider {
    fn token<'a>(
        &'a self,
        force_refresh: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, McpError>> + Send + 'a>>
    {
        Box::pin(async move {
            // What stored tokens must be bound to (`Tokens::bound_to`): the
            // rendered server URL for MCP, the token endpoint for HTTP.
            let want = self
                .resource_url
                .clone()
                .or_else(|| self.settings.token_url.clone());
            let seen = self.load(want.as_ref()).await?;
            if !force_refresh && !seen.expired(unix_now()) {
                return Ok(seen.access);
            }
            let lock = refresh_lock(&self.account);
            let _guard = lock.lock().await;
            // Another request refreshed while this one waited: the token is
            // seconds old, so it is used even when it expires soon.
            let mut tokens = self.load(want.as_ref()).await?;
            if tokens.access != seen.access {
                return Ok(tokens.access);
            }
            let Some(refresh) = tokens.refresh.clone() else {
                return Err(McpError::NeedsLogin(
                    "the token expired; sign in again".into(),
                ));
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
                &self.settings.network,
            )
            .await?;
            tokens.client_secret = client_secret;
            tokens.bound_to = want;
            let store = self.store.clone();
            let account = self.account.clone();
            let serialized = tokens.to_json();
            // The refresh worked: a failed save must not fail this request.
            // The next run refreshes again from the stored refresh token.
            let _ = tokio::task::spawn_blocking(move || store.set(&account, &serialized)).await;
            Ok(tokens.access)
        })
    }
}
