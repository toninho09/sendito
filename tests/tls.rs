//! `[http] tls`: a custom CA replaces the platform roots, and a client
//! certificate satisfies servers that require mutual TLS. Covers the HTTP
//! sender and the gRPC connector; MCP HTTP shares `tls_config`.

use std::sync::Arc;

use sendito_core::resolve::{HeaderOrigin, ResolvedBody, ResolvedHttp};
use sendito_grpc::{ConnSettings, connect};
use sendito_http::{TlsMaterial, send};
use sendito_pipeline::runner::Project;
use sendito_testkit::{grpc_echo, http_echo, tls};

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
        signed: None,
    }
}

/// Writes the PEM files of a generated PKI into `dir` and returns the
/// matching `[http.tls]` material.
fn write_certs(dir: &std::path::Path, c: &tls::TestCerts) -> TlsMaterial {
    let ca = dir.join("ca.pem");
    let cert = dir.join("client.pem");
    let key = dir.join("client.key");
    std::fs::write(&ca, &c.ca_pem).must();
    std::fs::write(&cert, &c.client_pem).must();
    std::fs::write(&key, &c.client_key_pem).must();
    TlsMaterial {
        ca: Some(ca),
        client_cert: Some(cert),
        client_key: Some(key),
    }
}

#[tokio::test]
async fn custom_ca_is_trusted_and_the_platform_roots_are_not() {
    let certs = tls::generate().must();
    let srv = http_echo::start_tls(&certs, false).await.must();
    let dir = tempfile::tempdir().must();
    let material = write_certs(dir.path(), &certs);

    // With the CA: the request succeeds over TLS.
    let opts = sendito_http::SendOptions {
        tls: material.clone(),
        ..Default::default()
    };
    let r = send(&get(format!("{}/echo", srv.url())), &opts)
        .await
        .must();
    assert_eq!(r.status, 200);

    // Without it: the self-signed server certificate is not trusted.
    let r = send(&get(srv.url()), &sendito_http::SendOptions::default()).await;
    assert!(r.is_err(), "an unknown CA must fail verification");

    // CA only (no client pair): still fine when the server wants no client
    // certificate.
    let opts = sendito_http::SendOptions {
        tls: TlsMaterial {
            ca: material.ca.clone(),
            client_cert: None,
            client_key: None,
        },
        ..Default::default()
    };
    let r = send(&get(srv.url()), &opts).await.must();
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn mutual_tls_presents_the_client_certificate() {
    let certs = tls::generate().must();
    let srv = http_echo::start_tls(&certs, true).await.must();
    let dir = tempfile::tempdir().must();
    let material = write_certs(dir.path(), &certs);

    // The full pair satisfies the server.
    let opts = sendito_http::SendOptions {
        tls: material,
        ..Default::default()
    };
    let r = send(&get(srv.url()), &opts).await.must();
    assert_eq!(r.status, 200);

    // A CA alone is not enough: the handshake fails without a client
    // certificate.
    let opts = sendito_http::SendOptions {
        tls: TlsMaterial {
            ca: Some(dir.path().join("ca.pem")),
            client_cert: None,
            client_key: None,
        },
        ..Default::default()
    };
    let r = send(&get(srv.url()), &opts).await;
    assert!(
        r.is_err(),
        "a server requiring a client certificate must refuse"
    );
}

#[tokio::test]
async fn grpc_over_tls_with_the_custom_ca() {
    let certs = tls::generate().must();
    let srv = grpc_echo::start_tls(&certs).await.must();
    let dir = tempfile::tempdir().must();
    let material = write_certs(dir.path(), &certs);
    let settings = |tls: TlsMaterial| ConnSettings {
        uri: format!("https://{}", srv.target()),
        verify_tls: true,
        connect_timeout: std::time::Duration::from_secs(5),
        tls,
    };
    connect(&settings(material)).await.must();
    let r = connect(&settings(TlsMaterial::default())).await;
    assert!(r.is_err(), "an unknown CA must fail verification");
}

/// A `[http.tls]` pointing at files that do not exist must not break a plain
/// `http://` request: the PEM material is only read for TLS.
#[tokio::test]
async fn plain_http_ignores_the_tls_material() {
    let echo = http_echo::start().await.must();
    let missing = std::path::PathBuf::from("/nonexistent/ca.pem");
    let opts = sendito_http::SendOptions {
        tls: TlsMaterial {
            ca: Some(missing.clone()),
            client_cert: Some(missing.clone()),
            client_key: Some(missing),
        },
        ..Default::default()
    };
    let r = send(&get(format!("{}/echo", echo.url())), &opts)
        .await
        .must();
    assert_eq!(r.status, 200);
}

/// MCP over Streamable HTTP goes through the same `tls_config`: the custom CA
/// is what makes the TLS server trusted.
#[tokio::test]
async fn mcp_http_over_tls_with_the_custom_ca() {
    use sendito_testkit::mcp_echo;
    let certs = tls::generate().must();
    let srv = mcp_echo::start_tls(&certs, false).await.must();
    let dir = tempfile::tempdir().must();
    let material = write_certs(dir.path(), &certs);
    let transport = |tls: TlsMaterial| sendito_mcp::Transport::Http {
        url: format!("{}/mcp", srv.url()),
        headers: Vec::new(),
        verify_tls: true,
        proxy: None,
        tls,
        auth: None,
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let tools = sendito_mcp::list_tools(&transport(material), &sendito_mcp::Options::default(), tx)
        .await
        .must();
    assert_eq!(tools.len(), 2);

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let r = sendito_mcp::list_tools(
        &transport(TlsMaterial::default()),
        &sendito_mcp::Options::default(),
        tx,
    )
    .await;
    assert!(r.is_err(), "an unknown CA must fail verification");
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
fn project_resolves_and_validates_tls_config() {
    // Relative paths are resolved against the collection root.
    let (_dir, p) = collection_config(
        "[http.tls]\nca = \"certs/ca.pem\"\nclient_cert = \"c.pem\"\nclient_key = \"k.pem\"\n",
    );
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
    assert_eq!(p.tls.ca.clone().must(), p.root.join("certs/ca.pem"));
    assert_eq!(p.send_options().tls, p.tls);
    assert_eq!(p.tls.client_cert.clone().must(), p.root.join("c.pem"));
    assert_eq!(p.tls.client_key.clone().must(), p.root.join("k.pem"));

    // A pair set on one side only warns.
    let (_dir, p) = collection_config("[http.tls]\nclient_cert = \"c.pem\"\n");
    assert!(
        p.warnings
            .iter()
            .any(|w| w.contains("client_cert") && w.contains("client_key")),
        "{:?}",
        p.warnings
    );

    // Parsing tolerates an absent table.
    let (_dir, p) = collection_config("[http]\ntimeout_ms = 1\n");
    assert!(p.tls.is_empty());
    assert!(p.warnings.is_empty(), "{:?}", p.warnings);
}
