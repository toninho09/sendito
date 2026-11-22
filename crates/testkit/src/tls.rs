//! Self-signed test certificates (a CA, a server and a client) and the TLS
//! server configs built from them, for exercising `[http] tls` end to end.

use std::sync::Arc;

use rcgen::{BasicConstraints, CertificateParams, IsCa, KeyPair};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};

/// PEM material for a throwaway PKI: everything is signed by `ca` and the
/// server certificate is valid for `127.0.0.1` and `localhost`.
pub struct TestCerts {
    pub ca_pem: Vec<u8>,
    pub server_pem: Vec<u8>,
    pub server_key_pem: Vec<u8>,
    pub client_pem: Vec<u8>,
    pub client_key_pem: Vec<u8>,
}

/// Generates a fresh PKI; callers keep the PEM files in memory only.
pub fn generate() -> Result<TestCerts, Box<dyn std::error::Error + Send + Sync>> {
    let ca_key = KeyPair::generate()?;
    let mut ca_params = CertificateParams::new(Vec::<String>::new())?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    let ca = ca_params.self_signed(&ca_key)?;

    let server_key = KeyPair::generate()?;
    let server_params = CertificateParams::new(vec!["localhost".into(), "127.0.0.1".into()])?;
    let server = server_params.signed_by(&server_key, &ca, &ca_key)?;

    let client_key = KeyPair::generate()?;
    let client_params = CertificateParams::new(vec!["localhost".into()])?;
    let client = client_params.signed_by(&client_key, &ca, &ca_key)?;

    Ok(TestCerts {
        ca_pem: ca.pem().into_bytes(),
        server_pem: server.pem().into_bytes(),
        server_key_pem: server_key.serialize_pem().into_bytes(),
        client_pem: client.pem().into_bytes(),
        client_key_pem: client_key.serialize_pem().into_bytes(),
    })
}

fn certs(pem: &[u8]) -> Result<Vec<CertificateDer<'static>>, String> {
    CertificateDer::pem_slice_iter(pem)
        .collect::<Result<_, _>>()
        .map_err(|e| e.to_string())
}

fn key(pem: &[u8]) -> Result<PrivateKeyDer<'static>, String> {
    PrivateKeyDer::from_pem_slice(pem).map_err(|e| e.to_string())
}

fn roots_of(ca_pem: &[u8]) -> Result<RootCertStore, String> {
    let mut roots = RootCertStore::empty();
    for c in certs(ca_pem)? {
        roots.add(c).map_err(|e| e.to_string())?;
    }
    Ok(roots)
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// A TLS server config: the server presents its certificate and, with
/// `require_client`, refuses clients that do not present one signed by the
/// CA (mutual TLS).
pub fn server_config(
    c: &TestCerts,
    require_client: bool,
    alpn_h2: bool,
) -> Result<ServerConfig, Box<dyn std::error::Error + Send + Sync>> {
    let builder = rustls::ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let builder = if require_client {
        let verifier =
            WebPkiClientVerifier::builder_with_provider(Arc::new(roots_of(&c.ca_pem)?), provider())
                .build()
                .map_err(|e| e.to_string())?;
        builder.with_client_cert_verifier(verifier)
    } else {
        builder.with_no_client_auth()
    };
    let mut cfg = builder
        .with_single_cert(certs(&c.server_pem)?, key(&c.server_key_pem)?)
        .map_err(|e| e.to_string())?;
    cfg.alpn_protocols = if alpn_h2 {
        vec![b"h2".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    Ok(cfg)
}
