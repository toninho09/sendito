//! TLS: certificate verifiers, client certificates and the rustls config.

use super::*;

/// Accepts any server certificate (TLS verification turned off by the user).
#[derive(Debug)]
pub struct NoVerify(pub Arc<CryptoProvider>);

impl ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

pub fn provider() -> Arc<CryptoProvider> {
    static P: OnceLock<Arc<CryptoProvider>> = OnceLock::new();
    P.get_or_init(|| Arc::new(rustls::crypto::ring::default_provider()))
        .clone()
}

/// The platform verifier (system trust store), built once.
pub fn platform_verifier() -> Result<Arc<dyn ServerCertVerifier>, String> {
    static V: OnceLock<Result<Arc<rustls_platform_verifier::Verifier>, String>> = OnceLock::new();
    V.get_or_init(|| {
        rustls_platform_verifier::Verifier::new(provider())
            .map(Arc::new)
            .map_err(|e| e.to_string())
    })
    .clone()
    .map(|v| v as Arc<dyn ServerCertVerifier>)
}

/// PEM material from `[http.tls]`: a certificate authority that replaces the
/// platform roots and a client certificate for mutual TLS. Paths are
/// resolved by the caller (relative to the collection root, `~/` expanded)
/// and read when the connection is set up, so a missing file fails the send
/// with its name in the message.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TlsMaterial {
    /// PEM file with CA certificates; the file may hold several.
    pub ca: Option<std::path::PathBuf>,
    /// PEM file with the client certificate chain.
    pub client_cert: Option<std::path::PathBuf>,
    /// PEM file with the client private key.
    pub client_key: Option<std::path::PathBuf>,
}

impl TlsMaterial {
    pub fn is_empty(&self) -> bool {
        self.ca.is_none() && self.client_cert.is_none() && self.client_key.is_none()
    }

    /// `Some(message)` when `client_cert` and `client_key` are not both set
    /// or both unset (they are a pair).
    pub fn pair_error(&self) -> Option<String> {
        match (&self.client_cert, &self.client_key) {
            (None, None) | (Some(_), Some(_)) => None,
            (Some(c), None) => Some(format!(
                "[http.tls] client_cert {} is set without client_key",
                c.display()
            )),
            (None, Some(k)) => Some(format!(
                "[http.tls] client_key {} is set without client_cert",
                k.display()
            )),
        }
    }
}

/// A rustls client config with the given verification mode, PEM material and
/// ALPN protocols. `verify: false` disables server verification, so `ca` is
/// unused there; a client certificate still authenticates the client.
pub fn tls_config(
    verify: bool,
    alpn: &[&[u8]],
    material: &TlsMaterial,
) -> Result<rustls::ClientConfig, String> {
    let builder = rustls::ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let verifier: Arc<dyn ServerCertVerifier> = if verify {
        match &material.ca {
            None => platform_verifier()?,
            Some(ca) => root_verifier(ca)?,
        }
    } else {
        Arc::new(NoVerify(provider()))
    };
    let builder = builder
        .dangerous()
        .with_custom_certificate_verifier(verifier);
    let mut cfg = match (&material.client_cert, &material.client_key) {
        (Some(cert), Some(key)) => {
            let chain = read_cert_chain(cert)?;
            let key = read_key(key)?;
            builder
                .with_client_auth_cert(chain, key)
                .map_err(|e| e.to_string())?
        }
        _ => builder.with_no_client_auth(),
    };
    cfg.alpn_protocols = alpn.iter().map(|p| p.to_vec()).collect();
    Ok(cfg)
}

/// A verifier that trusts only the certificates in `ca` (they replace the
/// platform roots). This is the `[http.tls] ca` behaviour.
pub(super) fn root_verifier(ca: &std::path::Path) -> Result<Arc<dyn ServerCertVerifier>, String> {
    let pem = read_pem(ca)?;
    let mut roots = rustls::RootCertStore::empty();
    let mut n = 0;
    for cert in CertificateDer::pem_slice_iter(&pem) {
        let cert = cert.map_err(|e| format!("{}: {e}", ca.display()))?;
        roots
            .add(cert)
            .map_err(|e| format!("{}: certificate cannot be a trust root: {e}", ca.display()))?;
        n += 1;
    }
    if n == 0 {
        return Err(format!("{} holds no certificates", ca.display()));
    }
    let verifier =
        rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider())
            .build()
            .map_err(|e| e.to_string())?;
    Ok(verifier)
}

pub(super) fn read_cert_chain(
    path: &std::path::Path,
) -> Result<Vec<CertificateDer<'static>>, String> {
    let pem = read_pem(path)?;
    let chain: Vec<_> = CertificateDer::pem_slice_iter(&pem)
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if chain.is_empty() {
        return Err(format!("{} holds no certificates", path.display()));
    }
    Ok(chain)
}

/// A PEM file's bytes. Each send builds its TLS configuration, so the bytes
/// are kept and read again only when the file's size or time changes.
pub(super) fn read_pem(path: &std::path::Path) -> Result<Arc<[u8]>, String> {
    type Key = (std::path::PathBuf, Option<std::time::SystemTime>, u64);
    type Cache = std::collections::HashMap<std::path::PathBuf, (Key, Arc<[u8]>)>;
    static PEMS: OnceLock<Mutex<Cache>> = OnceLock::new();
    let err = |e: std::io::Error| format!("{}: {e}", path.display());
    let meta = std::fs::metadata(path).map_err(err)?;
    let key: Key = (path.to_path_buf(), meta.modified().ok(), meta.len());
    let pems = PEMS.get_or_init(Default::default);
    if let Ok(map) = pems.lock()
        && let Some((k, bytes)) = map.get(path)
        && *k == key
    {
        return Ok(bytes.clone());
    }
    let bytes: Arc<[u8]> = std::fs::read(path).map_err(err)?.into();
    if let Ok(mut map) = pems.lock() {
        map.insert(path.to_path_buf(), (key, bytes.clone()));
    }
    Ok(bytes)
}

pub(super) fn read_key(
    path: &std::path::Path,
) -> Result<rustls::pki_types::PrivateKeyDer<'static>, String> {
    let pem = read_pem(path)?;
    rustls::pki_types::PrivateKeyDer::from_pem_slice(&pem)
        .map_err(|e| format!("{}: {e}", path.display()))
}

pub(super) fn err_chain(e: &(dyn std::error::Error + 'static)) -> String {
    let mut parts = vec![e.to_string()];
    let mut cur = e.source();
    while let Some(s) = cur {
        let t = s.to_string();
        if !parts.iter().any(|p| p.contains(&t)) {
            parts.push(t);
        }
        cur = s.source();
    }
    parts.join(": ")
}
