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

use crate::{McpError, TokenProvider};

mod discovery;
mod encode;
mod flow;
mod provider;
pub use discovery::*;
pub use encode::*;
pub use flow::*;
pub use provider::*;

/// How an HTTP OAuth identity gets its tokens (MCP servers always use the
/// authorization code).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Grant {
    #[default]
    AuthorizationCode,
    ClientCredentials,
    Password,
    DeviceCode,
}

impl Grant {
    /// Gets a token without a person: when a request needs one.
    pub fn automatic(self) -> bool {
        matches!(self, Grant::ClientCredentials | Grant::Password)
    }
}

/// OAuth settings from `[mcp.servers.<name>.auth]` (type = "oauth") or from
/// an HTTP identity (`[oauth.<name>]`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OAuthSettings {
    pub grant: Grant,
    /// Password grant only.
    pub username: Option<String>,
    pub password: Option<String>,
    /// Device code only (RFC 8628).
    pub device_authorization_url: Option<String>,
    pub client_id: Option<String>,
    pub client_secret: Option<String>,
    pub authorize_url: Option<String>,
    pub token_url: Option<String>,
    pub registration_url: Option<String>,
    pub scopes: Vec<String>,
    /// How the OAuth requests reach the authorization server.
    pub network: Network,
}

/// `[http] tls` and `[http] proxy`, so discovery, registration and token
/// requests go out the same way as the API requests they authorize.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Network {
    pub tls: sendito_http::TlsMaterial,
    pub proxy: Option<sendito_http::ProxySpec>,
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
    /// The MCP server URL or the token endpoint the tokens were issued for.
    /// Tokens are never sent to another one. `None` in logins saved before
    /// this field existed.
    pub bound_to: Option<String>,
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
            "bound_to": self.bound_to,
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
            bound_to: v
                .get("bound_to")
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

fn client(net: &Network) -> Result<reqwest::Client, McpError> {
    let tls = sendito_http::tls_config(true, &[b"h2", b"http/1.1"], &net.tls)
        .map_err(McpError::Transport)?;
    let mut builder = reqwest::Client::builder();
    if let Some(proxy) = &net.proxy {
        builder = proxy.apply(builder).map_err(McpError::Transport)?;
    }
    builder
        .tls_backend_preconfigured(tls)
        // Never forward authorization codes, refresh tokens or client secrets
        // to an endpoint selected by an HTTP redirect.
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| McpError::Transport(e.to_string()))
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
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
        use sha2::Digest;
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
        assert!(validate_oauth_url("http://[::1]:1234/token").is_ok());
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
            bound_to: Some("https://a.example/mcp".into()),
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
