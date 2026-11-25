//! HTTP Digest access authentication (RFC 7616): read the server's
//! challenge and write the `Authorization` answer. `qop=auth` and the legacy
//! form without `qop`; MD5, SHA-256 and their `-sess` variants.

use md5::Md5;
use sha2::{Digest, Sha256};

use super::hex;

/// A Digest challenge from a `WWW-Authenticate` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Challenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    /// As the server wrote it (`MD5`, `SHA-256`, `MD5-sess`, ...).
    pub algorithm: String,
    /// `auth` when the server offers it; `None` for the legacy form.
    pub qop: Option<String>,
}

/// The best Digest challenge among the `WWW-Authenticate` values: SHA-256
/// before MD5. `None` when there is none Sendito can answer.
pub fn challenge(values: &[&str]) -> Option<Challenge> {
    let mut found: Vec<Challenge> = values.iter().filter_map(|v| parse(v)).collect();
    found.sort_by_key(|c| !c.algorithm.to_ascii_uppercase().starts_with("SHA-256"));
    found.into_iter().next()
}

fn parse(value: &str) -> Option<Challenge> {
    let value = value.trim();
    let (scheme, rest) = value.split_once(char::is_whitespace)?;
    if !scheme.eq_ignore_ascii_case("digest") {
        return None;
    }
    let params = params(rest);
    let get = |k: &str| {
        params
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(k))
            .map(|(_, v)| v.clone())
    };
    let algorithm = get("algorithm").unwrap_or_else(|| "MD5".into());
    let base = algorithm.to_ascii_uppercase();
    let base = base.trim_end_matches("-SESS");
    if base != "MD5" && base != "SHA-256" {
        return None;
    }
    let qop = match get("qop") {
        None => None,
        Some(q) => {
            if q.split(',').any(|o| o.trim().eq_ignore_ascii_case("auth")) {
                Some("auth".into())
            } else {
                // Only auth-int: the body would be part of the hash.
                return None;
            }
        }
    };
    Some(Challenge {
        realm: get("realm").unwrap_or_default(),
        nonce: get("nonce")?,
        opaque: get("opaque"),
        algorithm,
        qop,
    })
}

/// `name=value` and `name="quoted value"` pairs, separated by commas. A
/// token without `=` starts another scheme: the parsing stops there.
fn params(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut chars = s.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| *c == ',' || c.is_whitespace()) {
            chars.next();
        }
        let name: String =
            std::iter::from_fn(|| chars.next_if(|c| *c != '=' && *c != ',' && !c.is_whitespace()))
                .collect();
        if name.is_empty() {
            break;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        if chars.next_if_eq(&'=').is_none() {
            break;
        }
        while chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let mut value = String::new();
        if chars.next_if_eq(&'"').is_some() {
            while let Some(c) = chars.next() {
                match c {
                    '\\' => value.extend(chars.next()),
                    '"' => break,
                    c => value.push(c),
                }
            }
        } else {
            value = std::iter::from_fn(|| chars.next_if(|c| *c != ','))
                .collect::<String>()
                .trim()
                .to_string();
        }
        out.push((name, value));
    }
    out
}

fn h(algorithm: &str, data: &str) -> String {
    if algorithm.to_ascii_uppercase().starts_with("SHA-256") {
        hex(&Sha256::digest(data.as_bytes()))
    } else {
        hex(&Md5::digest(data.as_bytes()))
    }
}

/// A new client nonce.
pub fn cnonce() -> String {
    (0..16)
        .map(|_| format!("{:02x}", fastrand::u8(..)))
        .collect()
}

/// The `Authorization` value that answers `c` for one request: `uri` is the
/// request target (path and query), `nc` counts the uses of the nonce.
pub fn authorization(
    c: &Challenge,
    username: &str,
    password: &str,
    method: &str,
    uri: &str,
    cnonce: &str,
    nc: u32,
) -> String {
    let alg = &c.algorithm;
    let mut ha1 = h(alg, &format!("{username}:{}:{password}", c.realm));
    if alg.to_ascii_uppercase().ends_with("-SESS") {
        ha1 = h(alg, &format!("{ha1}:{}:{cnonce}", c.nonce));
    }
    let ha2 = h(alg, &format!("{method}:{uri}"));
    let nc = format!("{nc:08x}");
    let response = match &c.qop {
        Some(qop) => h(alg, &format!("{ha1}:{}:{nc}:{cnonce}:{qop}:{ha2}", c.nonce)),
        None => h(alg, &format!("{ha1}:{}:{ha2}", c.nonce)),
    };
    let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
    let mut out = format!(
        "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{}\", algorithm={alg}",
        q(username),
        q(&c.realm),
        q(&c.nonce),
        q(uri)
    );
    if let Some(qop) = &c.qop {
        out.push_str(&format!(", qop={qop}, nc={nc}, cnonce=\"{}\"", q(cnonce)));
    }
    out.push_str(&format!(", response=\"{response}\""));
    if let Some(o) = &c.opaque {
        out.push_str(&format!(", opaque=\"{}\"", q(o)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response_of(header: &str) -> &str {
        header
            .split("response=\"")
            .nth(1)
            .and_then(|r| r.split('"').next())
            .unwrap()
    }

    #[test]
    fn rfc_2617_md5_example() {
        let c = challenge(&[
            "Digest realm=\"testrealm@host.com\", qop=\"auth,auth-int\", \
             nonce=\"dcd98b7102dd2f0e8b11d0f600bfb0c093\", opaque=\"5ccc069c403ebaf9f0171e9517f40e41\"",
        ])
        .unwrap();
        assert_eq!(c.qop.as_deref(), Some("auth"));
        let a = authorization(
            &c,
            "Mufasa",
            "Circle Of Life",
            "GET",
            "/dir/index.html",
            "0a4f113b",
            1,
        );
        assert_eq!(response_of(&a), "6629fae49393a05397450978507c4ef1");
        assert!(
            a.contains("nc=00000001") && a.contains("opaque=\"5ccc069c403ebaf9f0171e9517f40e41\"")
        );
    }

    #[test]
    fn rfc_7616_sha256_is_preferred_and_matches() {
        let values = [
            "Digest realm=\"http-auth@example.org\", qop=\"auth, auth-int\", algorithm=MD5, \
             nonce=\"7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v\", \
             opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\"",
            "Digest realm=\"http-auth@example.org\", qop=\"auth, auth-int\", algorithm=SHA-256, \
             nonce=\"7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v\", \
             opaque=\"FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS\"",
        ];
        let cnonce = "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ";
        let c = challenge(&values).unwrap();
        assert_eq!(c.algorithm, "SHA-256");
        let a = authorization(
            &c,
            "Mufasa",
            "Circle of Life",
            "GET",
            "/dir/index.html",
            cnonce,
            1,
        );
        assert_eq!(
            response_of(&a),
            "753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1"
        );
        let md5 = challenge(&values[..1]).unwrap();
        let a = authorization(
            &md5,
            "Mufasa",
            "Circle of Life",
            "GET",
            "/dir/index.html",
            cnonce,
            1,
        );
        assert_eq!(response_of(&a), "8ca523f5e9506fed4657c9700eebdbec");
    }

    #[test]
    fn unsupported_challenges_are_skipped() {
        assert!(challenge(&["Basic realm=\"x\""]).is_none());
        assert!(challenge(&["Digest realm=\"x\", nonce=\"n\", qop=\"auth-int\""]).is_none());
        assert!(challenge(&["Digest realm=\"x\", nonce=\"n\", algorithm=SHA-512-256"]).is_none());
        assert!(challenge(&["Digest realm=\"x\""]).is_none());
        // The legacy form has no qop.
        let c = challenge(&["Digest realm=\"r\", nonce=\"n\""]).unwrap();
        let a = authorization(&c, "u", "p", "GET", "/", "c", 1);
        assert!(!a.contains("qop=") && !a.contains("cnonce"));
    }

    #[test]
    fn a_second_scheme_in_the_same_header_stops_the_parameters() {
        let p = params("realm=\"a, b\", nonce=n Basic realm=\"x\"");
        assert_eq!(p[0], ("realm".into(), "a, b".into()));
        assert_eq!(p[1].0, "nonce");
    }
}
