//! Auths computed just before a send: a JWT signed with the clock, an AWS
//! Signature Version 4 over the request as it is sent, and the answer to an
//! HTTP Digest challenge. The request fields arrive rendered
//! (`sendito_core::resolve::SignedAuth`).

pub mod digest;
pub mod jwt;
pub mod sigv4;

use base64::Engine;

/// Lower-case hex of `bytes`.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lower-case hex SHA-256 of `bytes` (the SigV4 payload hash).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex(&sha2::Sha256::digest(bytes))
}

pub(crate) fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Seconds since the Unix epoch.
pub(crate) fn unix(now: std::time::SystemTime) -> u64 {
    now.duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
