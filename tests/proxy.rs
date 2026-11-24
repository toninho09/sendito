//! `[http] proxy`: requests go through a forward proxy when one is
//! configured, and connect directly when it is not. The wire bytes are the
//! request's own (the corpus checks those); here we check the routing.

use std::sync::Arc;

use sendito_core::resolve::{HeaderOrigin, ResolvedBody, ResolvedHttp};
use sendito_http::{ProxySpec, SendOptions, send};
use sendito_pipeline::runner::Project;
use sendito_testkit::{http_echo, mcp_echo, proxy};

trait Must<T> {
    fn must(self) -> T;
}

impl<T, E: std::fmt::Debug> Must<T> for Result<T, E> {
    #[track_caller]
    fn must(self) -> T {
        match self {
            Ok(v) => v,
            Err(e) => panic!("{e:?}"),
        }
    }
}

impl<T> Must<T> for Option<T> {
    #[track_caller]
    fn must(self) -> T {
        match self {
            Some(v) => v,
            None => panic!("unexpected None"),
        }
    }
}

fn get(url: String) -> ResolvedHttp {
    ResolvedHttp {
        method: "GET".into(),
        url,
        headers: vec![],
        header_origins: vec![HeaderOrigin::User],
        body: ResolvedBody::None,
        timeout_ms: Some(5000),
        follow_redirects: true,
        verify_tls: true,
        unresolved: vec![],
        notes: vec![],
    }
}

#[tokio::test]
async fn explicit_proxy_forwards_the_request() {
    let echo = http_echo::start().await.must();
    let px = proxy::start().await.must();
    let opts = SendOptions {
        proxy: Some(ProxySpec::Url(px.url())),
        ..Default::default()
    };
    let r = send(&get(format!("{}/x?q=1", echo.url())), &opts)
        .await
        .must();
    assert_eq!(r.status, 200);
    // The proxy saw the absolute-form target, the origin saw the request.
    let targets = px.take();
    assert_eq!(targets.len(), 1, "{targets:?}");
    assert!(targets[0].starts_with(&echo.url()), "{targets:?}");
    assert!(targets[0].contains("/x?q=1"), "{targets:?}");
    let last = echo.last().must();
    assert_eq!(last.request_line(), "GET /x?q=1 HTTP/1.1");
}

#[tokio::test]
async fn without_a_proxy_the_proxy_sees_nothing() {
    let echo = http_echo::start().await.must();
    let px = proxy::start().await.must();
    let r = send(&get(echo.url()), &SendOptions::default()).await.must();
    assert_eq!(r.status, 200);
    assert!(px.take().is_empty());
}

#[tokio::test]
async fn post_bodies_cross_the_proxy() {
    let echo = http_echo::start().await.must();
    let px = proxy::start().await.must();
    let mut rq = get(format!("{}/echo", echo.url()));
    rq.method = "POST".into();
    rq.headers
        .push(("Content-Type".into(), "application/json".into()));
    rq.body = ResolvedBody::Bytes(b"{\"a\":1}".to_vec());
    let opts = SendOptions {
        proxy: Some(ProxySpec::Url(px.url())),
        ..Default::default()
    };
    let r = send(&rq, &opts).await.must();
    assert_eq!(r.status, 200);
    let last = echo.last().must();
    assert_eq!(last.method(), "POST");
    assert_eq!(last.body(), b"{\"a\":1}");
}

#[tokio::test]
async fn mcp_http_server_through_the_proxy() {
    let echo = mcp_echo::start().await.must();
    let px = proxy::start().await.must();
    let transport = sendito_mcp::Transport::Http {
        url: format!("{}/mcp", echo.url()),
        headers: Vec::new(),
        verify_tls: false,
        proxy: Some(ProxySpec::Url(px.url())),
        tls: Default::default(),
        auth: None,
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let tools = sendito_mcp::list_tools(&transport, &sendito_mcp::Options::default(), tx)
        .await
        .must();
    assert_eq!(tools.len(), 2);
    // Three POSTs crossed the proxy: initialize, the initialized
    // notification, and tools/list.
    assert_eq!(px.take().len(), 3);
}

fn collection_config(text: &str) -> (tempfile::TempDir, Project) {
    let dir = tempfile::tempdir().must();
    let root = dir.path().to_path_buf();
    std::fs::create_dir_all(root.join(".sendito")).must();
    std::fs::write(root.join(".sendito/config.toml"), text).must();
    let keychain: Arc<dyn sendito_store::env::Keychain> =
        Arc::new(sendito_store::env::MemoryKeychain::default());
    let project = Project::open_with(&root, keychain, None);
    (dir, project)
}

#[test]
fn project_config_sets_and_validates_the_proxy() {
    let (_dir, p) = collection_config("[http]\nproxy = \"socks5://127.0.0.1:1080\"\n");
    assert_eq!(
        p.send_options().proxy,
        Some(ProxySpec::Url("socks5://127.0.0.1:1080".into()))
    );
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);

    let (_dir, p) = collection_config("[http]\nproxy = \"nope\"\n");
    assert_eq!(p.send_options().proxy, None);
    assert!(
        p.warnings.iter().any(|w| w.contains("[http] proxy")),
        "{:?}",
        p.warnings
    );
}

/// Runs `sendito run req.toml` in `dir` with the proxy variables explicitly
/// set (and everything else cleared), so `proxy = "env"` is exercised end to
/// end without touching this process's environment. The child runs on a
/// blocking thread so this runtime keeps polling the echo and proxy servers.
async fn run_with_proxy_env(dir: &std::path::Path, vars: &[(&str, &str)]) -> std::process::Output {
    let dir = dir.to_path_buf();
    let vars: Vec<(String, String)> = vars
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_sendito"));
        cmd.current_dir(&dir)
            .args(["run", "req.toml", "--no-history"])
            .env("SENDITO_KEYCHAIN", "memory")
            // Isolate from the developer's global config.
            .env("XDG_CONFIG_HOME", dir.join("xdg"));
        for key in [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
            "NO_PROXY",
            "no_proxy",
        ] {
            cmd.env_remove(key);
        }
        for (k, v) in &vars {
            cmd.env(k, v);
        }
        cmd.output().must()
    })
    .await
    .must()
}

#[tokio::test]
async fn env_mode_routes_http_and_honours_no_proxy() {
    let echo = http_echo::start().await.must();
    let px = proxy::start().await.must();
    let dir = tempfile::tempdir().must();
    std::fs::create_dir_all(dir.path().join(".sendito")).must();
    std::fs::write(
        dir.path().join(".sendito/config.toml"),
        "[http]\nproxy = \"env\"\n",
    )
    .must();
    std::fs::write(
        dir.path().join("req.toml"),
        format!(
            "kind = \"http\"\nurl = \"{}/echo\"\n\n[options]\ntimeout_ms = 5000\n",
            echo.url()
        ),
    )
    .must();
    let proxy_url = px.url();

    // HTTP_PROXY applies to plain HTTP requests.
    let out = run_with_proxy_env(dir.path(), &[("HTTP_PROXY", proxy_url.as_str())]).await;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert_eq!(px.take().len(), 1, "{stderr}");

    // ALL_PROXY is the fallback when HTTP_PROXY is absent.
    let out = run_with_proxy_env(dir.path(), &[("ALL_PROXY", proxy_url.as_str())]).await;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert_eq!(px.take().len(), 1, "{stderr}");

    // NO_PROXY exempts the host, so the request goes direct.
    let out = run_with_proxy_env(
        dir.path(),
        &[
            ("HTTP_PROXY", proxy_url.as_str()),
            ("NO_PROXY", "127.0.0.1"),
        ],
    )
    .await;
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    assert!(out.status.success(), "{stderr}");
    assert!(px.take().is_empty(), "{stderr}");
}
