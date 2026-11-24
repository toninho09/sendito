//! JSON Web Tokens (RFC 7519) signed at send time: HS256/384/512 with a
//! shared secret, RS256 with an RSA key and ES256 with a P-256 key.

use ring::rand::SystemRandom;
use ring::signature::{self, EcdsaKeyPair, RsaKeyPair};
use rustls::pki_types::PrivateKeyDer;
use rustls::pki_types::pem::PemObject;
use sendito_core::model::JwtAlgorithm;
use serde_json::{Value, json};

use super::b64url;

/// Signs `claims` (a JSON object). `expires_in_s` adds `iat` (now) and
/// `exp` (now + seconds). HS* uses `secret`; RS256 and ES256 use `key_pem`
/// (PKCS#8, or PKCS#1 for RSA).
pub fn sign(
    algorithm: JwtAlgorithm,
    secret: &str,
    key_pem: Option<&[u8]>,
    claims: &str,
    expires_in_s: Option<u64>,
    now: std::time::SystemTime,
) -> Result<String, String> {
    let mut claims: Value = serde_json::from_str(claims)
        .map_err(|e| format!("the JWT claims are not valid JSON: {e}"))?;
    let object = claims
        .as_object_mut()
        .ok_or("the JWT claims must be a JSON object")?;
    if let Some(s) = expires_in_s {
        let iat = super::unix(now);
        object.insert("iat".into(), json!(iat));
        object.insert("exp".into(), json!(iat.saturating_add(s)));
    }
    let header = json!({ "alg": algorithm.as_str(), "typ": "JWT" });
    let input = format!(
        "{}.{}",
        b64url(header.to_string().as_bytes()),
        b64url(claims.to_string().as_bytes())
    );
    let sig = signature_of(algorithm, secret, key_pem, input.as_bytes())?;
    Ok(format!("{input}.{}", b64url(&sig)))
}

fn signature_of(
    algorithm: JwtAlgorithm,
    secret: &str,
    key_pem: Option<&[u8]>,
    input: &[u8],
) -> Result<Vec<u8>, String> {
    let hmac = |alg| {
        if secret.is_empty() {
            return Err("the JWT secret is empty".to_string());
        }
        let key = ring::hmac::Key::new(alg, secret.as_bytes());
        Ok(ring::hmac::sign(&key, input).as_ref().to_vec())
    };
    match algorithm {
        JwtAlgorithm::Hs256 => hmac(ring::hmac::HMAC_SHA256),
        JwtAlgorithm::Hs384 => hmac(ring::hmac::HMAC_SHA384),
        JwtAlgorithm::Hs512 => hmac(ring::hmac::HMAC_SHA512),
        JwtAlgorithm::Rs256 => {
            let key = rsa_key(key_pem.ok_or("RS256 needs a private key file")?)?;
            let mut sig = vec![0; key.public().modulus_len()];
            key.sign(
                &signature::RSA_PKCS1_SHA256,
                &SystemRandom::new(),
                input,
                &mut sig,
            )
            .map_err(|_| "the RSA key could not sign the JWT".to_string())?;
            Ok(sig)
        }
        JwtAlgorithm::Es256 => {
            let key = ec_key(key_pem.ok_or("ES256 needs a private key file")?)?;
            key.sign(&SystemRandom::new(), input)
                .map(|s| s.as_ref().to_vec())
                .map_err(|_| "the EC key could not sign the JWT".to_string())
        }
    }
}

fn pem_key(pem: &[u8]) -> Result<PrivateKeyDer<'static>, String> {
    PrivateKeyDer::from_pem_slice(pem).map_err(|e| format!("no private key in the PEM file: {e}"))
}

fn rsa_key(pem: &[u8]) -> Result<RsaKeyPair, String> {
    let bad = |e: ring::error::KeyRejected| format!("not a usable RSA private key: {e}");
    match pem_key(pem)? {
        PrivateKeyDer::Pkcs8(der) => RsaKeyPair::from_pkcs8(der.secret_pkcs8_der()).map_err(bad),
        PrivateKeyDer::Pkcs1(der) => RsaKeyPair::from_der(der.secret_pkcs1_der()).map_err(bad),
        _ => Err("RS256 needs an RSA private key (PKCS#8 or PKCS#1 PEM)".into()),
    }
}

fn ec_key(pem: &[u8]) -> Result<EcdsaKeyPair, String> {
    match pem_key(pem)? {
        PrivateKeyDer::Pkcs8(der) => EcdsaKeyPair::from_pkcs8(
            &signature::ECDSA_P256_SHA256_FIXED_SIGNING,
            der.secret_pkcs8_der(),
            &SystemRandom::new(),
        )
        .map_err(|e| format!("not a usable P-256 private key: {e}")),
        PrivateKeyDer::Sec1(_) => Err(
            "ES256 needs a PKCS#8 key (\"BEGIN PRIVATE KEY\"); convert it with \
             openssl pkcs8 -topk8 -nocrypt"
                .into(),
        ),
        _ => Err("ES256 needs a P-256 private key (PKCS#8 PEM)".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use ring::signature::KeyPair;

    const RSA_PEM: &str = include_str!("testdata/rsa-test-only.pem");

    fn parts(token: &str) -> (Value, Value, Vec<u8>, String) {
        let p: Vec<&str> = token.split('.').collect();
        assert_eq!(p.len(), 3, "{token}");
        let d = |s: &str| {
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(s)
                .unwrap()
        };
        (
            serde_json::from_slice(&d(p[0])).unwrap(),
            serde_json::from_slice(&d(p[1])).unwrap(),
            d(p[2]),
            format!("{}.{}", p[0], p[1]),
        )
    }

    fn at(s: u64) -> std::time::SystemTime {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(s)
    }

    #[test]
    fn hs256_signs_the_header_and_claims() {
        // The signature is the HMAC of "header.claims" with the secret.
        let t = sign(
            JwtAlgorithm::Hs256,
            "k",
            None,
            r#"{"sub":"a"}"#,
            None,
            at(0),
        )
        .unwrap();
        let (h, c, sig, input) = parts(&t);
        assert_eq!(h, json!({"alg": "HS256", "typ": "JWT"}));
        assert_eq!(c, json!({"sub": "a"}));
        let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"k");
        assert!(ring::hmac::verify(&key, input.as_bytes(), &sig).is_ok());
    }

    #[test]
    fn expiry_adds_iat_and_exp() {
        let t = sign(JwtAlgorithm::Hs512, "s", None, "{}", Some(60), at(1000)).unwrap();
        let (_, c, sig, _) = parts(&t);
        assert_eq!(c, json!({"iat": 1000, "exp": 1060}));
        assert_eq!(sig.len(), 64);
    }

    #[test]
    fn rs256_verifies_with_the_public_key() {
        let t = sign(
            JwtAlgorithm::Rs256,
            "",
            Some(RSA_PEM.as_bytes()),
            "{}",
            None,
            at(0),
        )
        .unwrap();
        let (_, _, sig, input) = parts(&t);
        let key = rsa_key(RSA_PEM.as_bytes()).unwrap();
        signature::UnparsedPublicKey::new(
            &signature::RSA_PKCS1_2048_8192_SHA256,
            key.public().as_ref(),
        )
        .verify(input.as_bytes(), &sig)
        .unwrap();
    }

    #[test]
    fn es256_verifies_with_the_public_key() {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&signature::ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
            .unwrap();
        let pem = format!(
            "-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----\n",
            base64::engine::general_purpose::STANDARD.encode(pkcs8.as_ref())
        );
        let t = sign(
            JwtAlgorithm::Es256,
            "",
            Some(pem.as_bytes()),
            "{}",
            None,
            at(0),
        )
        .unwrap();
        let (_, _, sig, input) = parts(&t);
        let key = ec_key(pem.as_bytes()).unwrap();
        signature::UnparsedPublicKey::new(
            &signature::ECDSA_P256_SHA256_FIXED,
            key.public_key().as_ref(),
        )
        .verify(input.as_bytes(), &sig)
        .unwrap();
    }

    #[test]
    fn errors_name_the_problem() {
        let e = |r: Result<String, String>| r.unwrap_err();
        assert!(
            e(sign(JwtAlgorithm::Hs256, "", None, "{}", None, at(0))).contains("secret is empty")
        );
        assert!(
            e(sign(JwtAlgorithm::Hs256, "k", None, "[1]", None, at(0))).contains("JSON object")
        );
        assert!(
            e(sign(JwtAlgorithm::Hs256, "k", None, "{", None, at(0))).contains("not valid JSON")
        );
        assert!(
            e(sign(JwtAlgorithm::Rs256, "", None, "{}", None, at(0))).contains("private key file")
        );
        assert!(
            e(sign(
                JwtAlgorithm::Es256,
                "",
                Some(RSA_PEM.as_bytes()),
                "{}",
                None,
                at(0)
            ))
            .contains("P-256")
        );
    }
}
