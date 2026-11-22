//! End-to-end OAuth tests against the fake authorization server: discovery,
//! dynamic registration, PKCE, the loopback redirect, refresh and the 401
//! retry path.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sendito_mcp::oauth::{OAuthSettings, RefreshingProvider, TokenStore, login};
use sendito_mcp::{Options, Transport, call_tool};

#[derive(Default)]
struct MemStore(Mutex<BTreeMap<String, String>>);

impl TokenStore for MemStore {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        Ok(self.0.lock().ok().and_then(|g| g.get(account).cloned()))
    }
    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        if let Ok(mut g) = self.0.lock() {
            g.insert(account.into(), value.into());
        }
        Ok(())
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        if let Ok(mut g) = self.0.lock() {
            g.remove(account);
        }
        Ok(())
    }
}

/// A fake browser: grabs the URL the flow reports and follows the redirect
/// (a plain `Client::new()` would panic: the workspace builds rustls without
/// a default provider).
fn spawn_browser(slot: Arc<Mutex<Option<String>>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let tls = match sendito_http::tls_config(false, &[]) {
            Ok(t) => t,
            Err(_) => return,
        };
        let client = match reqwest::Client::builder()
            .tls_backend_preconfigured(tls)
            .build()
        {
            Ok(c) => c,
            Err(_) => return,
        };
        for _ in 0..500 {
            if let Some(url) = slot.lock().ok().and_then(|g| g.clone()) {
                let _ = client.get(&url).send().await;
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
}

#[tokio::test]
async fn login_flow_and_call() -> Result<(), Box<dyn std::error::Error>> {
    let srv = sendito_testkit::mcp_auth::start().await?;
    let captured = Arc::new(Mutex::new(None::<String>));
    let notes: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));

    let browser = spawn_browser(captured.clone());

    let opener = move |url: &str| -> bool {
        if let Ok(mut g) = captured.lock() {
            *g = Some(url.to_string());
        }
        false // "no browser": the flow must also print the URL
    };
    let note_fn = |n: String| {
        if let Ok(mut v) = notes.lock() {
            v.push(n);
        }
    };
    let tokens = login(
        &srv.mcp_url(),
        &OAuthSettings::default(),
        Duration::from_secs(10),
        &opener,
        &note_fn,
    )
    .await?;
    let _ = browser.await;

    assert!(srv.registered(), "dynamic registration must have run");
    assert_eq!(srv.logins(), 1);
    assert!(
        notes
            .lock()
            .ok()
            .is_some_and(|v| v.iter().any(|n| n.contains("authorization URL:\n  http"))),
        "without a browser the URL must be printed"
    );

    // The provider refreshes (expires_in 1s < the 30s slack) and calls fine.
    let store = Arc::new(MemStore::default());
    store.set("acct", &tokens.to_json())?;
    let provider = Arc::new(RefreshingProvider::new(
        store,
        "acct".into(),
        srv.mcp_url(),
        OAuthSettings::default(),
    ));
    let t = Transport::Http {
        url: srv.mcp_url(),
        headers: Vec::new(),
        verify_tls: false,
        auth: Some(provider),
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let result = call_tool(&t, "add", "{}", &Options::default(), tx).await?;
    assert_eq!(result.text(), "ok behind auth");
    assert!(srv.refreshes() >= 1, "the expired token must be refreshed");
    Ok(())
}

struct RetryToken {
    valid: String,
    refreshes: AtomicUsize,
}

impl sendito_mcp::TokenProvider for RetryToken {
    fn token<'a>(
        &'a self,
        force_refresh: bool,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<String, sendito_mcp::McpError>> + Send + 'a>,
    > {
        if force_refresh {
            self.refreshes.fetch_add(1, Ordering::SeqCst);
        }
        Box::pin(std::future::ready(Ok(if force_refresh {
            self.valid.clone()
        } else {
            "rejected-token".into()
        })))
    }
}

#[tokio::test]
async fn unauthorized_refreshes_and_retries_once() -> Result<(), Box<dyn std::error::Error>> {
    let srv = sendito_testkit::mcp_auth::start().await?;
    let captured = Arc::new(Mutex::new(None::<String>));
    let browser = spawn_browser(captured.clone());
    let opener = |url: &str| {
        if let Ok(mut slot) = captured.lock() {
            *slot = Some(url.to_string());
        }
        false
    };
    let tokens = login(
        &srv.mcp_url(),
        &OAuthSettings::default(),
        Duration::from_secs(10),
        &opener,
        &|_| {},
    )
    .await?;
    let _ = browser.await;
    let provider = Arc::new(RetryToken {
        valid: tokens.access,
        refreshes: AtomicUsize::new(0),
    });
    let transport = Transport::Http {
        url: srv.mcp_url(),
        headers: Vec::new(),
        verify_tls: false,
        auth: Some(provider.clone()),
    };
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    assert_eq!(
        call_tool(&transport, "add", "{}", &Options::default(), tx)
            .await?
            .text(),
        "ok behind auth"
    );
    // Each of the three POSTs is refused once before it is retried.
    assert_eq!(provider.refreshes.load(Ordering::SeqCst), 3);
    Ok(())
}

#[tokio::test]
async fn not_signed_in_is_a_clear_error() -> Result<(), Box<dyn std::error::Error>> {
    let srv = sendito_testkit::mcp_auth::start().await?;
    let provider = Arc::new(RefreshingProvider::new(
        Arc::new(MemStore::default()),
        "acct".into(),
        srv.mcp_url(),
        OAuthSettings::default(),
    ));
    let t = Transport::Http {
        url: srv.mcp_url(),
        headers: Vec::new(),
        verify_tls: false,
        auth: Some(provider),
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = call_tool(&t, "add", "{}", &Options::default(), tx)
        .await
        .err()
        .map(|e| e.to_string())
        .unwrap_or_default();
    assert!(err.contains("not signed in"), "got: {err}");
    Ok(())
}

#[tokio::test]
async fn explicit_endpoints_skip_discovery() -> Result<(), Box<dyn std::error::Error>> {
    let srv = sendito_testkit::mcp_auth::start().await?;
    let settings = OAuthSettings {
        authorize_url: Some(format!("{}/authorize", srv.url())),
        token_url: Some(format!("{}/token", srv.url())),
        registration_url: Some(format!("{}/register", srv.url())),
        ..OAuthSettings::default()
    };
    let captured = Arc::new(Mutex::new(None::<String>));
    let browser = spawn_browser(captured.clone());
    let opener = |url: &str| {
        if let Ok(mut g) = captured.lock() {
            *g = Some(url.to_string());
        }
        false
    };
    let tokens = login(
        &srv.mcp_url(),
        &settings,
        Duration::from_secs(10),
        &opener,
        &|_| {},
    )
    .await?;
    let _ = browser.await;
    assert!(tokens.access.starts_with("tok-"));
    Ok(())
}
