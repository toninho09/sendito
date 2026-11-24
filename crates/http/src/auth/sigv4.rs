//! AWS Signature Version 4 (header form) over the request as it is sent.
//!
//! Signed headers: `host`, `content-type` when the request has one, and every
//! `x-amz-*` header (the ones added here and the request's own). The payload
//! hash comes from the caller, which knows the body bytes.

use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use time::OffsetDateTime;

use super::{hex, sha256_hex};

/// The credentials and scope, rendered.
pub struct Credentials<'a> {
    pub access_key: &'a str,
    pub secret_key: &'a str,
    /// Empty for long-term credentials.
    pub session_token: &'a str,
    pub region: &'a str,
    pub service: &'a str,
}

/// Unreserved characters stay literal (RFC 3986): letters, digits, `-._~`.
const AWS: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// [`AWS`] for a path: `/` also stays literal.
const AWS_PATH: &AsciiSet = &AWS.remove(b'/');

/// The headers to add to the request, in this order: `X-Amz-Date`,
/// `X-Amz-Content-Sha256` (service `s3` only), `X-Amz-Security-Token` (with
/// a session token) and `Authorization`. `headers` are the request's own.
pub fn sign(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    payload_sha256: &str,
    creds: &Credentials,
    now: std::time::SystemTime,
) -> Result<Vec<(String, String)>, String> {
    if creds.access_key.is_empty() || creds.secret_key.is_empty() {
        return Err("AWS SigV4 needs an access key and a secret key".into());
    }
    if creds.region.is_empty() || creds.service.is_empty() {
        return Err("AWS SigV4 needs a region and a service".into());
    }
    let url = reqwest::Url::parse(url).map_err(|e| format!("AWS SigV4: invalid URL: {e}"))?;
    let host = match (url.host_str(), url.port()) {
        (Some(h), Some(p)) => format!("{h}:{p}"),
        (Some(h), None) => h.to_string(),
        (None, _) => return Err("AWS SigV4: the URL has no host".into()),
    };
    let when = OffsetDateTime::from(now);
    let date = format!(
        "{:04}{:02}{:02}",
        when.year(),
        u8::from(when.month()),
        when.day()
    );
    let amz_date = format!(
        "{date}T{:02}{:02}{:02}Z",
        when.hour(),
        when.minute(),
        when.second()
    );
    let s3 = creds.service == "s3";
    let mut added = vec![("X-Amz-Date".to_string(), amz_date.clone())];
    if s3 {
        added.push(("X-Amz-Content-Sha256".into(), payload_sha256.to_string()));
    }
    if !creds.session_token.is_empty() {
        added.push((
            "X-Amz-Security-Token".into(),
            creds.session_token.to_string(),
        ));
    }

    // The headers to sign, by lower-case name; values trimmed, inner runs of
    // spaces collapsed, several values of one name joined with commas.
    let mut signed: Vec<(String, String)> = vec![("host".into(), host)];
    for (n, v) in headers.iter().chain(added.iter()) {
        let name = n.trim().to_ascii_lowercase();
        if name != "content-type" && !name.starts_with("x-amz-") {
            continue;
        }
        let value = v.split_whitespace().collect::<Vec<_>>().join(" ");
        match signed.iter_mut().find(|(k, _)| *k == name) {
            Some((_, existing)) => {
                existing.push(',');
                existing.push_str(&value);
            }
            None => signed.push((name, value)),
        }
    }
    signed.sort_by(|a, b| a.0.cmp(&b.0));
    let canonical_headers: String = signed.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
    let signed_names = signed
        .iter()
        .map(|(k, _)| k.as_str())
        .collect::<Vec<_>>()
        .join(";");

    let canonical = format!(
        "{}\n{}\n{}\n{canonical_headers}\n{signed_names}\n{payload_sha256}",
        method.to_ascii_uppercase(),
        canonical_uri(url.path(), s3),
        canonical_query(url.query().unwrap_or("")),
    );
    let scope = format!("{date}/{}/{}/aws4_request", creds.region, creds.service);
    let to_sign = format!(
        "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
        sha256_hex(canonical.as_bytes())
    );
    let mut key = hmac(
        format!("AWS4{}", creds.secret_key).as_bytes(),
        date.as_bytes(),
    );
    for part in [creds.region, creds.service, "aws4_request"] {
        key = hmac(&key, part.as_bytes());
    }
    let signature = hex(&hmac(&key, to_sign.as_bytes()));
    added.push((
        "Authorization".into(),
        format!(
            "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_names}, Signature={signature}",
            creds.access_key
        ),
    ));
    Ok(added)
}

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let key = ring::hmac::Key::new(ring::hmac::HMAC_SHA256, key);
    ring::hmac::sign(&key, data).as_ref().to_vec()
}

/// The path as AWS expects it. The URL path is already percent-encoded:
/// S3 wants it encoded once (decoded, then encoded with the AWS rules); the
/// other services encode each segment twice, so the encoded path is encoded
/// again.
fn canonical_uri(path: &str, s3: bool) -> String {
    let path = if path.is_empty() { "/" } else { path };
    if s3 {
        let decoded = percent_decode_str(path).decode_utf8_lossy();
        utf8_percent_encode(&decoded, AWS_PATH).to_string()
    } else {
        utf8_percent_encode(path, AWS_PATH).to_string()
    }
}

/// The query pairs, decoded, encoded with the AWS rules and sorted by name
/// then value. `+` stays a plus sign: Sendito encodes spaces as `%20`.
fn canonical_query(query: &str) -> String {
    let enc = |s: &str| {
        let decoded = percent_decode_str(s).decode_utf8_lossy();
        utf8_percent_encode(&decoded, AWS).to_string()
    };
    let mut pairs: Vec<(String, String)> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) => (enc(k), enc(v)),
            None => (enc(p), String::new()),
        })
        .collect();
    pairs.sort();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 2015-08-30T12:36:00Z, the date of the AWS SigV4 test suite.
    fn suite_time() -> std::time::SystemTime {
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_440_938_160)
    }

    fn suite_creds() -> Credentials<'static> {
        Credentials {
            access_key: "AKIDEXAMPLE",
            secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token: "",
            region: "us-east-1",
            service: "service",
        }
    }

    fn authorization(added: &[(String, String)]) -> &str {
        added
            .iter()
            .find(|(n, _)| n == "Authorization")
            .map(|(_, v)| v.as_str())
            .unwrap()
    }

    #[test]
    fn get_vanilla() {
        let added = sign(
            "GET",
            "https://example.amazonaws.com/",
            &[],
            &sha256_hex(b""),
            &suite_creds(),
            suite_time(),
        )
        .unwrap();
        assert_eq!(added[0], ("X-Amz-Date".into(), "20150830T123600Z".into()));
        assert_eq!(
            authorization(&added),
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, \
             SignedHeaders=host;x-amz-date, \
             Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn get_vanilla_query_order_key_case() {
        let added = sign(
            "GET",
            "https://example.amazonaws.com/?Param2=value2&Param1=value1",
            &[],
            &sha256_hex(b""),
            &suite_creds(),
            suite_time(),
        )
        .unwrap();
        assert!(
            authorization(&added).ends_with(
                "Signature=b97d918cfa904a5beff61c982a1b6f458b799221646efd99d3219ec94cdf2500"
            ),
            "{}",
            authorization(&added)
        );
    }

    #[test]
    fn s3_and_session_token_add_their_headers() {
        let creds = Credentials {
            service: "s3",
            session_token: "tok",
            ..suite_creds()
        };
        let added = sign(
            "PUT",
            "https://bucket.s3.amazonaws.com/a%20b",
            &[("Content-Type".into(), "text/plain".into())],
            &sha256_hex(b"x"),
            &creds,
            suite_time(),
        )
        .unwrap();
        let names: Vec<&str> = added.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            [
                "X-Amz-Date",
                "X-Amz-Content-Sha256",
                "X-Amz-Security-Token",
                "Authorization"
            ]
        );
        assert!(authorization(&added).contains(
            "SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token,"
        ));
    }

    #[test]
    fn paths_and_queries_are_canonical() {
        assert_eq!(canonical_uri("/a%20b/", true), "/a%20b/");
        assert_eq!(canonical_uri("/a%20b/", false), "/a%2520b/");
        assert_eq!(canonical_uri("", false), "/");
        assert_eq!(
            canonical_query("b=2&a=x%20y&a=1&flag"),
            "a=1&a=x%20y&b=2&flag="
        );
        assert_eq!(canonical_query("q=a+b"), "q=a%2Bb");
    }

    #[test]
    fn missing_credentials_are_refused() {
        let creds = Credentials {
            secret_key: "",
            ..suite_creds()
        };
        assert!(sign("GET", "https://h/", &[], "", &creds, suite_time()).is_err());
    }
}
