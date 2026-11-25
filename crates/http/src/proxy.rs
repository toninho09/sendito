//! Proxy settings: explicit URL, environment variables and credential masking.

/// Outbound proxy from `[http] proxy` in the config.
#[derive(Clone, PartialEq, Eq)]
pub enum ProxySpec {
    /// `proxy = "env"`: `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and
    /// `NO_PROXY` (lowercase too), with curl's precedence — the scheme
    /// variable beats `ALL_PROXY`, and `NO_PROXY` exempts hosts.
    Env,
    /// A proxy URL such as `"http://host:port"` (also `https://`,
    /// `socks4://`, `socks4a://`, `socks5://`, `socks5h://`); credentials may
    /// be part of the URL.
    Url(String),
}

/// Credentials in a proxy URL never reach logs: `Debug` shows them redacted.
impl std::fmt::Debug for ProxySpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProxySpec::Env => f.write_str("Env"),
            ProxySpec::Url(u) => f.debug_tuple("Url").field(&redact_url(u)).finish(),
        }
    }
}

impl ProxySpec {
    /// Parses the `[http] proxy` value.
    pub fn parse(value: &str) -> Result<Self, String> {
        let v = value.trim();
        if v.eq_ignore_ascii_case("env") {
            return Ok(ProxySpec::Env);
        }
        const SCHEMES: [&str; 6] = [
            "http://",
            "https://",
            "socks4://",
            "socks4a://",
            "socks5://",
            "socks5h://",
        ];
        if SCHEMES.iter().any(|s| v.starts_with(s)) {
            return Ok(ProxySpec::Url(v.to_string()));
        }
        Err(format!(
            "invalid [http] proxy {:?}: use \"env\" or a URL such as \"http://host:port\" or \"socks5://host:port\"",
            redact_url(v)
        ))
    }

    /// Adds the proxy to a client builder. Reads the environment only for
    /// [`ProxySpec::Env`], at send time.
    pub fn apply(&self, mut b: reqwest::ClientBuilder) -> Result<reqwest::ClientBuilder, String> {
        match self {
            ProxySpec::Url(u) => {
                let p = reqwest::Proxy::all(u.clone())
                    .map_err(|e| proxy_error("[http] proxy", u, e))?;
                Ok(b.proxy(p))
            }
            ProxySpec::Env => {
                let read = |k: &str| std::env::var(k).ok();
                let (https, http) = env_proxy_urls(&read);
                if let Some(u) = https {
                    let p = reqwest::Proxy::https(u.clone())
                        .map_err(|e| proxy_error("HTTPS_PROXY", &u, e))?
                        .no_proxy(reqwest::NoProxy::from_env());
                    b = b.proxy(p);
                }
                if let Some(u) = http {
                    let p = reqwest::Proxy::http(u.clone())
                        .map_err(|e| proxy_error("HTTP_PROXY", &u, e))?
                        .no_proxy(reqwest::NoProxy::from_env());
                    b = b.proxy(p);
                }
                Ok(b)
            }
        }
    }
}

/// Curl precedence: `HTTPS_PROXY` and `HTTP_PROXY` beat `ALL_PROXY`; the
/// result is the proxy for `https` and `http` requests respectively.
pub(super) fn env_proxy_urls(
    get: &impl Fn(&str) -> Option<String>,
) -> (Option<String>, Option<String>) {
    let all = env_proxy("ALL_PROXY", get);
    let https = env_proxy("HTTPS_PROXY", get).or_else(|| all.clone());
    let http = env_proxy("HTTP_PROXY", get).or(all);
    (https, http)
}

/// One proxy variable, uppercase before lowercase, empty values skipped.
pub(super) fn env_proxy(name: &str, get: &impl Fn(&str) -> Option<String>) -> Option<String> {
    for key in [name, &name.to_ascii_lowercase()] {
        if let Some(v) = get(key) {
            let v = v.trim().to_string();
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Builds a proxy error message with the credentials in `raw` redacted. The
/// underlying error may repeat the URL, so the same redaction is applied to
/// its text.
pub(super) fn proxy_error(what: &str, raw: &str, err: impl std::fmt::Display) -> String {
    let hidden = redact_url(raw);
    let msg = err.to_string();
    let msg = if raw == hidden {
        msg
    } else {
        msg.replace(raw, &hidden)
    };
    format!("invalid {what} {hidden:?}: {msg}")
}

/// A proxy URL without its userinfo (`scheme://user:pass@host` becomes
/// `scheme://host`), so error messages never print credentials.
pub(super) fn redact_url(raw: &str) -> String {
    let Some((scheme, rest)) = raw.split_once("://") else {
        return raw.to_string();
    };
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let Some((_, host)) = rest[..end].rsplit_once('@') else {
        return raw.to_string();
    };
    format!("{scheme}://{host}{}", &rest[end..])
}
