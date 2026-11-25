//! curl command import and export.

use std::path::Path;

use sendito_core::model::*;
use sendito_core::resolve::{ResolvedBody, resolve_http};
use sendito_core::vars::Scope;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CurlError {
    #[error("cannot parse command: {0}")]
    Parse(String),
    #[error("no URL found in the command")]
    NoUrl,
    #[error("option {0} needs a value")]
    MissingValue(String),
    #[error("{0}")]
    WebSocket(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CurlImport {
    pub request: HttpRequest,
    /// Options that were ignored or approximated.
    pub notes: Vec<String>,
}

/// Options that only affect curl's own output; they are dropped silently.
const OUTPUT_ONLY: &[&str] = &[
    "-s",
    "--silent",
    "-S",
    "--show-error",
    "-v",
    "--verbose",
    "-i",
    "--include",
    "-#",
    "--progress-bar",
    "-f",
    "--fail",
    "--fail-with-body",
    "-N",
    "--no-buffer",
    "--no-progress-meter",
];

/// Options that take a value but do not change the request.
const OUTPUT_ONLY_WITH_VALUE: &[&str] = &[
    "-o",
    "--output",
    "-w",
    "--write-out",
    "--trace",
    "--trace-ascii",
    "--stderr",
    "-D",
    "--dump-header",
    "--output-dir",
    "--etag-save",
    "--libcurl",
];

fn normalize_continuations(cmd: &str) -> String {
    cmd.replace("\\\r\n", " ").replace("\\\n", " ")
}

/// Short options that take a value (`-XPOST` and `-X POST` are the same).
const SHORT_WITH_VALUE: &[char] = &[
    'X', 'H', 'd', 'F', 'u', 'A', 'e', 'b', 'm', 'o', 'w', 'x', 'c', 'T', 'E', 'K', 'r', 'z', 'U',
    'Y', 'y', 'Q', 'P', 't', 'C', 'D',
];

/// Long options that take a value and that the importer reads.
const LONG_WITH_VALUE: &[&str] = &[
    "--request",
    "--header",
    "--data",
    "--data-ascii",
    "--data-raw",
    "--data-binary",
    "--data-urlencode",
    "--json",
    "--form",
    "--form-string",
    "--user",
    "--user-agent",
    "--referer",
    "--cookie",
    "--max-time",
    "--url",
    "--proxy",
    "--oauth2-bearer",
];

/// Options that take a value but cannot be imported; the value is skipped
/// with a note, so it is never read as the URL.
const UNSUPPORTED_WITH_VALUE: &[&str] = &[
    "--connect-timeout",
    "--retry",
    "--retry-delay",
    "--retry-max-time",
    "--max-redirs",
    "--cacert",
    "--capath",
    "--cert",
    "--cert-type",
    "--key",
    "--key-type",
    "--pass",
    "--resolve",
    "--connect-to",
    "-c",
    "--cookie-jar",
    "-T",
    "--upload-file",
    "-E",
    "-K",
    "--config",
    "--limit-rate",
    "-r",
    "--range",
    "-z",
    "--time-cond",
    "-U",
    "--proxy-user",
    "--noproxy",
    "--unix-socket",
    "--abstract-unix-socket",
    "--interface",
    "--local-port",
    "--dns-servers",
    "-Y",
    "--speed-limit",
    "-y",
    "--speed-time",
    "--max-filesize",
    "--expect100-timeout",
    "--keepalive-time",
    "--pinnedpubkey",
    "--ciphers",
    "--tls-max",
    "--aws-sigv4",
    "--netrc-file",
    "--proto",
    "--proto-redir",
    "-Q",
    "--quote",
    "-P",
    "--ftp-port",
    "-t",
    "--telnet-option",
    "-C",
    "--continue-at",
    "--socks4",
    "--socks4a",
    "--socks5",
    "--socks5-hostname",
    "--preproxy",
    "--proxy-header",
    "--crlfile",
    "--request-target",
    "--url-query",
    "--etag-compare",
    "--happy-eyeballs-timeout-ms",
    "--hostpubmd5",
    "--hostpubsha256",
    "--mail-from",
    "--mail-rcpt",
    "--variable",
];

fn takes_value(opt: &str) -> bool {
    LONG_WITH_VALUE.contains(&opt)
        || UNSUPPORTED_WITH_VALUE.contains(&opt)
        || OUTPUT_ONLY_WITH_VALUE.contains(&opt)
        || opt
            .strip_prefix('-')
            .and_then(|c| {
                let mut it = c.chars();
                it.next().filter(|_| it.next().is_none())
            })
            .is_some_and(|c| SHORT_WITH_VALUE.contains(&c))
}

/// Splits combined short flags (`-sSL` → `-s -S -L`) and attached values
/// (`-XPOST` → `-X POST`, `-HAccept:x` → `-H Accept:x`). The value of an
/// option is never split, so `-d -10` keeps `-10` as the body.
fn expand(tokens: Vec<String>) -> Vec<String> {
    let mut out = Vec::new();
    let mut value_next = false;
    for t in tokens {
        if value_next {
            value_next = false;
            out.push(t);
            continue;
        }
        if t.starts_with("--") || !t.starts_with('-') || t.len() <= 2 {
            value_next = takes_value(&t);
            out.push(t);
            continue;
        }
        let chars: Vec<char> = t[1..].chars().collect();
        let mut i = 0;
        while i < chars.len() {
            let c = chars[i];
            out.push(format!("-{c}"));
            if SHORT_WITH_VALUE.contains(&c) {
                let rest: String = chars[i + 1..].iter().collect();
                if rest.is_empty() {
                    value_next = true;
                } else {
                    out.push(rest);
                }
                break;
            }
            i += 1;
        }
    }
    out
}

/// Splits a `-F` value into its content and its `;type=`/`;filename=`
/// parameters. A content in double quotes (`name="a;b"`) is literal text:
/// `\\` and `\"` are its escapes. The flag is true for a quoted content.
fn split_form_value(rest: &str, literal: bool) -> (String, Vec<String>, bool) {
    if literal {
        return (rest.to_string(), Vec::new(), false);
    }
    if let Some(body) = rest.strip_prefix('"') {
        let mut value = String::new();
        let mut chars = body.char_indices();
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, n)) = chars.next() {
                        value.push(n);
                    }
                }
                '"' => {
                    let tail = &body[i + 1..];
                    let segs = tail
                        .strip_prefix(';')
                        .map(|t| t.split(';').map(str::to_string).collect())
                        .unwrap_or_default();
                    return (value, segs, true);
                }
                _ => value.push(c),
            }
        }
    }
    let mut segs: Vec<String> = rest.split(';').map(str::to_string).collect();
    let first = segs.remove(0);
    (first, segs, false)
}

/// Parses a curl command line into a request.
pub fn import(cmd: &str) -> Result<CurlImport, CurlError> {
    let tokens = shell_words::split(&normalize_continuations(cmd.trim()))
        .map_err(|e| CurlError::Parse(e.to_string()))?;
    let mut tokens = expand(tokens);
    if tokens.first().map(String::as_str) == Some("curl") {
        tokens.remove(0);
    }
    let mut notes = Vec::new();
    let mut method: Option<String> = None;
    let mut url: Option<String> = None;
    let mut headers: Vec<KeyValue> = Vec::new();
    let mut data: Vec<String> = Vec::new();
    let mut data_file: Option<String> = None;
    let mut form: Vec<MultipartPart> = Vec::new();
    let mut auth = Auth::Inherit;
    let mut get = false;
    let mut head = false;
    let mut json = false;
    let mut options = HttpOptions {
        follow_redirects: false,
        ..Default::default()
    };
    let mut it = tokens.into_iter().peekable();
    while let Some(t) = it.next() {
        let mut val = |name: &str| {
            it.next()
                .ok_or_else(|| CurlError::MissingValue(name.to_string()))
        };
        match t.as_str() {
            "-X" | "--request" => method = Some(val(&t)?),
            "-H" | "--header" => {
                let h = val(&t)?;
                if let Some((n, v)) = h.split_once(':') {
                    if v.trim().is_empty() {
                        // curl removes a header given with no value instead of
                        // sending it; keep the row, disabled, so nothing is sent.
                        let n = n.trim();
                        headers.push(KeyValue::new(n, "").disabled());
                        notes.push(format!(
                            "-H {h:?} tells curl not to send {n}; imported disabled \
                             (curl sends an empty value with -H '{n};')"
                        ));
                    } else {
                        headers.push(KeyValue::new(n.trim(), v.trim_start()));
                    }
                } else if let Some(n) = h.strip_suffix(';') {
                    headers.push(KeyValue::new(n.trim(), ""));
                } else {
                    notes.push(format!("ignored malformed header {h:?}"));
                }
            }
            "-d" | "--data" | "--data-ascii" => {
                let d = val(&t)?;
                if let Some(f) = d.strip_prefix('@') {
                    notes.push(format!("{t} @{f}: file sent as-is (curl would strip newlines)"));
                    data_file = Some(f.to_string());
                } else {
                    data.push(d);
                }
            }
            "--data-raw" => data.push(val(&t)?),
            "--data-binary" => {
                let d = val(&t)?;
                if let Some(f) = d.strip_prefix('@') {
                    data_file = Some(f.to_string());
                } else {
                    data.push(d);
                }
            }
            "--data-urlencode" => {
                let d = val(&t)?;
                let enc = match d.split_once('=') {
                    Some((n, v)) if !n.is_empty() => format!("{n}={}", sendito_core::resolve::form_urlencode(v)),
                    Some((_, v)) => sendito_core::resolve::form_urlencode(v),
                    None => sendito_core::resolve::form_urlencode(&d),
                };
                data.push(enc);
            }
            "--json" => {
                json = true;
                data.push(val(&t)?);
            }
            "-F" | "--form" | "--form-string" => {
                let f = val(&t)?;
                let literal = t == "--form-string";
                let Some((name, rest)) = f.split_once('=') else {
                    notes.push(format!("ignored malformed form field {f:?}"));
                    continue;
                };
                let (first, segs, quoted) = split_form_value(rest, literal);
                let mut ct = String::new();
                let mut filename = None;
                for s in segs {
                    if let Some(v) = s.strip_prefix("type=") {
                        ct = v.to_string();
                    } else if let Some(v) = s.strip_prefix("filename=") {
                        filename = Some(v.trim_matches('"').to_string());
                    }
                }
                let plain = literal || quoted;
                let kind = if let (false, Some(p)) = (plain, first.strip_prefix('@')) {
                    PartKind::File { path: p.to_string(), filename }
                } else if let (false, Some(p)) = (plain, first.strip_prefix('<')) {
                    notes.push(format!("form field {name}=<{p}: file contents sent as a file part"));
                    PartKind::File { path: p.to_string(), filename }
                } else {
                    PartKind::Text { value: first }
                };
                form.push(MultipartPart {
                    name: name.to_string(),
                    kind,
                    content_type: ct,
                    enabled: true,
                });
            }
            "-u" | "--user" => {
                let u = val(&t)?;
                let (user, pass) = u.split_once(':').unwrap_or((u.as_str(), ""));
                auth = Auth::Basic {
                    username: user.to_string(),
                    password: pass.to_string(),
                };
            }
            "-A" | "--user-agent" => headers.push(KeyValue::new("User-Agent", val(&t)?)),
            "-e" | "--referer" => headers.push(KeyValue::new("Referer", val(&t)?)),
            "-b" | "--cookie" => {
                let c = val(&t)?;
                if c.contains('=') {
                    headers.push(KeyValue::new("Cookie", c));
                } else {
                    notes.push(format!("cookie file {c:?} not imported"));
                }
            }
            "-G" | "--get" => get = true,
            "-I" | "--head" => head = true,
            "-L" | "--location" => options.follow_redirects = true,
            "-k" | "--insecure" => options.verify_tls = false,
            "-m" | "--max-time" => {
                let v = val(&t)?;
                match v.parse::<f64>() {
                    Ok(s) if s > 0.0 => options.timeout_ms = Some((s * 1000.0).round() as u64),
                    _ => notes.push(format!("ignored invalid --max-time {v:?}")),
                }
            }
            "--url" => url = Some(val(&t)?),
            "--compressed" => notes.push("--compressed ignored: Sendito sends no Accept-Encoding and shows bodies as received".into()),
            "--http1.1" | "--http2" | "--http1.0" | "--http2-prior-knowledge" | "--http3" => {
                notes.push(format!("{t} ignored: the HTTP version is negotiated automatically"))
            }
            "-x" | "--proxy" => {
                let v = val(&t)?;
                notes.push(format!(
                    "{t} {v} not imported: set [http] proxy in .sendito/config.toml"
                ))
            }
            "--oauth2-bearer" => auth = Auth::Bearer { token: val(&t)? },
            s if UNSUPPORTED_WITH_VALUE.contains(&s) => {
                let v = val(&t)?;
                notes.push(format!("{t} {v} not supported; ignored"));
            }
            s if OUTPUT_ONLY.contains(&s) => {}
            s if OUTPUT_ONLY_WITH_VALUE.contains(&s) => {
                let _ = val(&t)?;
            }
            s if s.starts_with('-') && s.len() > 1 => notes.push(format!("unknown option {s} ignored")),
            _ => {
                if url.is_none() {
                    url = Some(t);
                } else {
                    notes.push(format!("extra argument {t:?} ignored"));
                }
            }
        }
    }
    let mut url = url.ok_or(CurlError::NoUrl)?;
    let has_ct = headers
        .iter()
        .any(|h| h.name.eq_ignore_ascii_case("content-type"));
    if json {
        if !has_ct {
            headers.push(KeyValue::new("Content-Type", "application/json"));
        }
        if !headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("accept"))
        {
            headers.push(KeyValue::new("Accept", "application/json"));
        }
    }
    let joined = data.join("&");
    let mut body = Body::None;
    if get && !data.is_empty() {
        url.push(if url.contains('?') { '&' } else { '?' });
        url.push_str(&joined);
    } else if !form.is_empty() {
        body = Body::Multipart { parts: form };
    } else if let Some(f) = data_file {
        body = Body::File {
            path: f,
            content_type: if has_ct || json {
                String::new()
            } else {
                "application/x-www-form-urlencoded".into()
            },
        };
    } else if !data.is_empty() {
        let ct = headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("content-type"))
            .map(|h| h.value.to_ascii_lowercase());
        body = match ct {
            // The header already says JSON; the body kind only drives editor help.
            Some(c) if c.contains("json") => Body::Json { text: joined },
            Some(_) => Body::Raw {
                content_type: String::new(),
                text: joined,
            },
            None => Body::Raw {
                content_type: "application/x-www-form-urlencoded".into(),
                text: joined,
            },
        };
    }
    // An explicit Content-Type header wins over the body's implied one, as in curl.
    if let Body::Json { .. } = body
        && let Some(i) = headers
            .iter()
            .position(|h| h.name.eq_ignore_ascii_case("content-type"))
        && headers[i].value.eq_ignore_ascii_case("application/json")
    {
        headers.remove(i);
    }
    // A WebSocket session is not an HTTP request: curl has no equivalent
    // description (the upgrade headers alone do not make one).
    if url.starts_with("ws://") || url.starts_with("wss://") {
        let u = &url;
        return Err(CurlError::WebSocket(format!(
            "{} is a WebSocket URL; curl cannot describe a session — \
             open one with :newws and paste the URL",
            u
        )));
    }
    let method = match method {
        Some(m) => m.to_ascii_uppercase(),
        None if head => "HEAD".into(),
        None if get => "GET".into(),
        None if !matches!(body, Body::None) => "POST".into(),
        None => "GET".into(),
    };
    Ok(CurlImport {
        request: HttpRequest {
            method,
            url,
            params: vec![],
            headers,
            body,
            auth,
            options,
            common: RequestCommon::default(),
        },
        notes,
    })
}

/// Quotes a word for POSIX shells.
pub fn shell_quote(s: &str) -> String {
    if !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_./:@%+=,".contains(&b))
    {
        return s.to_string();
    }
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Exports a request as a curl command.
///
/// With `include_secrets` false, variables whose value is secret stay as
/// `{{placeholders}}`; everything else is substituted.
pub fn export(req: &HttpRequest, scope: &Scope, include_secrets: bool, base_dir: &Path) -> String {
    let (template, empty);
    let (src, sc): (&HttpRequest, &Scope) = if include_secrets {
        (req, scope)
    } else {
        template = match sendito_core::resolve::mask_request(&Request::Http(req.clone()), scope) {
            Request::Http(h) => h,
            _ => req.clone(),
        };
        empty = Scope::new();
        (&template, &empty)
    };
    let mut no_auth = src.clone();
    no_auth.auth = match &src.auth {
        Auth::Basic { .. } => Auth::None,
        Auth::OAuth { .. } => Auth::None,
        other => other.clone(),
    };
    let mut r = resolve_http(&no_auth, sc, base_dir);
    crate::preserve_export_templates(&no_auth, sc, &mut r);
    if let ResolvedBody::Invalid(message) = &r.body {
        return format!("# cannot export: {message}");
    }
    let own_authorization = r
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("authorization"));
    if !own_authorization {
        match &src.auth {
            Auth::OAuth { name } => r.headers.push((
                "Authorization".into(),
                format!("Bearer {}", crate::oauth_placeholder(&sc.render(name).text)),
            )),
            // Signed when Sendito sends: a JWT has no value until then.
            Auth::Jwt { .. } => r.headers.push((
                "Authorization".into(),
                format!("Bearer {}", crate::JWT_PLACEHOLDER),
            )),
            Auth::AwsSigV4 { session_token, .. } if !session_token.is_empty() => r
                .headers
                .push(("X-Amz-Security-Token".into(), sc.render(session_token).text)),
            _ => {}
        }
    }
    let mut args: Vec<String> = vec!["curl".into()];
    let has_body = !matches!(r.body, ResolvedBody::None);
    match r.method.as_str() {
        "GET" if !has_body => {}
        "POST" if has_body && !matches!(r.body, ResolvedBody::Multipart(_)) => {}
        "POST" if matches!(r.body, ResolvedBody::Multipart(_)) => {}
        "HEAD" if !has_body => args.push("--head".into()),
        m => {
            args.push("-X".into());
            args.push(shell_quote(m));
        }
    }
    args.push(shell_quote(&r.url));
    match &src.auth {
        _ if own_authorization => {}
        Auth::Basic { username, password } | Auth::Digest { username, password } => {
            if matches!(src.auth, Auth::Digest { .. }) {
                args.push("--digest".into());
            }
            args.push("-u".into());
            args.push(shell_quote(&format!(
                "{}:{}",
                sc.render(username).text,
                sc.render(password).text
            )));
        }
        Auth::AwsSigV4 {
            access_key,
            secret_key,
            region,
            service,
            ..
        } => {
            args.push("--aws-sigv4".into());
            args.push(shell_quote(&format!(
                "aws:amz:{}:{}",
                sc.render(region).text,
                sc.render(service).text
            )));
            args.push("-u".into());
            args.push(shell_quote(&format!(
                "{}:{}",
                sc.render(access_key).text,
                sc.render(secret_key).text
            )));
        }
        _ => {}
    }
    for (n, v) in &r.headers {
        args.push("-H".into());
        // `-H 'Name:'` and `-H 'Name: '` make curl drop the header; `Name;`
        // sends it with an empty value.
        if v.is_empty() {
            args.push(shell_quote(&format!("{n};")));
        } else {
            args.push(shell_quote(&format!("{n}: {v}")));
        }
    }
    match &r.body {
        ResolvedBody::None => {}
        ResolvedBody::Bytes(b) => {
            args.push("--data-raw".into());
            args.push(shell_quote(&String::from_utf8_lossy(b)));
        }
        ResolvedBody::File(p) => {
            args.push("--data-binary".into());
            args.push(shell_quote(&format!("@{}", p.display())));
        }
        ResolvedBody::Multipart(parts) => {
            for p in parts {
                match &p.data {
                    sendito_core::resolve::PartData::Text(t) => {
                        if let Some(ct) = &p.content_type {
                            args.push("-F".into());
                            // A quoted value keeps a leading `@` or `<` and any `;`
                            // literal: curl does not read a file for it.
                            let v = t.replace('\\', "\\\\").replace('"', "\\\"");
                            args.push(shell_quote(&format!("{}=\"{v}\";type={ct}", p.name)));
                        } else {
                            args.push("--form-string".into());
                            args.push(shell_quote(&format!("{}={}", p.name, t)));
                        }
                    }
                    sendito_core::resolve::PartData::File { path, filename } => {
                        let mut s = format!("{}=@{}", p.name, path.display());
                        let default_name = path
                            .file_name()
                            .map(|f| f.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        if *filename != default_name {
                            s.push_str(&format!(";filename={filename}"));
                        }
                        if let Some(ct) = &p.content_type {
                            s.push_str(&format!(";type={ct}"));
                        }
                        args.push("-F".into());
                        args.push(shell_quote(&s));
                    }
                }
            }
        }
        ResolvedBody::Invalid(_) => {}
    }
    if r.follow_redirects {
        args.push("-L".into());
    }
    if !r.verify_tls {
        args.push("-k".into());
    }
    if let Some(t) = r.timeout_ms {
        args.push("--max-time".into());
        args.push(format!("{}", t as f64 / 1000.0));
    }
    // One option per line for readability.
    let mut out = String::new();
    let mut i = 0;
    while i < args.len() {
        if i > 0 {
            out.push_str(" \\\n  ");
        }
        out.push_str(&args[i]);
        if args[i].starts_with('-')
            && i + 1 < args.len()
            && !matches!(args[i].as_str(), "-L" | "-k" | "--head")
        {
            out.push(' ');
            out.push_str(&args[i + 1]);
            i += 1;
        } else if i == 0 && i + 1 < args.len() && !args[i + 1].starts_with('-') {
            // `curl 'url'` stays on the first line.
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sendito_core::vars::VarValue;
    use std::collections::BTreeMap;

    #[test]
    fn websocket_urls_are_refused_with_guidance() {
        for u in ["ws://127.0.0.1:9001/chat", "wss://example.com/socket"] {
            match import(&format!("curl {u}")) {
                Err(CurlError::WebSocket(msg)) => {
                    assert!(msg.contains(":newws"), "{msg}");
                }
                other => panic!("{u}: expected a WebSocket refusal, got {other:?}"),
            }
        }
        // An HTTP URL still imports.
        assert!(import("curl https://example.com/x").is_ok());
    }

    #[test]
    fn blank_header_is_disabled_and_empty_header_exports_with_semicolon() {
        let r = import("curl http://h -H 'X-Gone: ' -H 'X-Empty;'").unwrap();
        assert!(!r.request.headers[0].enabled);
        assert!(r.notes.iter().any(|n| n.contains("X-Gone")));
        let out = export(&r.request, &Scope::new(), true, Path::new(""));
        assert!(out.contains("-H 'X-Empty;'"), "{out}");
        assert!(!out.contains("X-Gone"), "{out}");
    }

    #[test]
    fn imports_common_forms() {
        let r = import(
            "curl 'https://api.x.com/users?a=1' -H 'Accept: application/json' -H 'X-Empty;' -sS",
        )
        .unwrap();
        assert_eq!(r.request.method, "GET");
        assert_eq!(r.request.url, "https://api.x.com/users?a=1");
        assert_eq!(
            r.request.headers,
            vec![
                KeyValue::new("Accept", "application/json"),
                KeyValue::new("X-Empty", "")
            ]
        );
        assert!(!r.request.options.follow_redirects);
        assert!(r.notes.is_empty());

        let r =
            import("curl -XPUT -L -k -m 2.5 --url http://h/x -d 'a=1' -d b=2 -u ana:pw").unwrap();
        assert_eq!(r.request.method, "PUT");
        assert_eq!(
            r.request.body,
            Body::Raw {
                content_type: "application/x-www-form-urlencoded".into(),
                text: "a=1&b=2".into()
            }
        );
        assert_eq!(
            r.request.auth,
            Auth::Basic {
                username: "ana".into(),
                password: "pw".into()
            }
        );
        assert!(r.request.options.follow_redirects);
        assert!(!r.request.options.verify_tls);
        assert_eq!(r.request.options.timeout_ms, Some(2500));

        let r = import("curl http://h -H 'Content-Type: application/json' --data-raw '{\"a\": 1}'")
            .unwrap();
        assert_eq!(r.request.method, "POST");
        assert_eq!(
            r.request.body,
            Body::Json {
                text: "{\"a\": 1}".into()
            }
        );
        assert!(r.request.headers.is_empty());

        let r = import("curl http://h -F 'meta={\"a\":1};type=application/json' -F file=@/tmp/a.png -F 'doc=@b.pdf;filename=c.pdf;type=application/pdf' --form-string 'raw=x;y'").unwrap();
        let Body::Multipart { parts } = &r.request.body else {
            panic!()
        };
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].content_type, "application/json");
        assert_eq!(
            parts[1].kind,
            PartKind::File {
                path: "/tmp/a.png".into(),
                filename: None
            }
        );
        assert_eq!(
            parts[2].kind,
            PartKind::File {
                path: "b.pdf".into(),
                filename: Some("c.pdf".into())
            }
        );
        assert_eq!(
            parts[3].kind,
            PartKind::Text {
                value: "x;y".into()
            }
        );

        let r = import("curl -G http://h/s --data-urlencode 'q=a b' -d x=1").unwrap();
        assert_eq!(r.request.method, "GET");
        assert_eq!(r.request.url, "http://h/s?q=a+b&x=1");

        let r = import("curl --json '{\"x\":1}' http://h").unwrap();
        assert_eq!(
            r.request.headers,
            vec![KeyValue::new("Accept", "application/json")]
        );
        assert!(matches!(r.request.body, Body::Json { .. }));

        let r = import("curl -I http://h --compressed --http2 -o out.txt -b 'a=b' -A ua -e ref --data-binary @f.bin -x proxy:1 --weird").unwrap();
        assert_eq!(r.request.method, "HEAD");
        assert!(matches!(r.request.body, Body::File { .. }));
        assert_eq!(r.notes.len(), 4, "{:?}", r.notes);
        let r = import("curl -I http://h").unwrap();
        assert_eq!(r.request.method, "HEAD");

        let r = import("curl \\\n  http://h \\\n  -H 'A: b'").unwrap();
        assert_eq!(r.request.headers.len(), 1);
        assert_eq!(import("curl -H"), Err(CurlError::MissingValue("-H".into())));
        assert_eq!(import("curl -s"), Err(CurlError::NoUrl));
        assert!(matches!(
            import("curl 'unterminated"),
            Err(CurlError::Parse(_))
        ));
        let r =
            import("curl http://h -d @data.txt -b cookies.txt -H bad -m x -F bad extra").unwrap();
        assert_eq!(r.notes.len(), 6, "{:?}", r.notes);
    }

    #[test]
    fn option_values_are_not_split_or_read_as_the_url() {
        let r = import("curl -d '-10.5' https://x/a").unwrap();
        assert_eq!(r.request.url, "https://x/a");
        assert!(matches!(&r.request.body, Body::Raw { text, .. } if text == "-10.5"));
        assert!(r.notes.is_empty(), "{:?}", r.notes);

        let r = import("curl -D h.txt -sD- https://api.example/x").unwrap();
        assert_eq!(r.request.url, "https://api.example/x");
        assert!(r.notes.is_empty(), "{:?}", r.notes);

        let r = import("curl --oauth2-bearer abc https://api.example/x").unwrap();
        assert_eq!(r.request.url, "https://api.example/x");
        assert_eq!(
            r.request.auth,
            Auth::Bearer {
                token: "abc".into()
            }
        );

        for opt in [
            "-c jar.txt",
            "-T up.bin",
            "--limit-rate 1k",
            "-E c.pem",
            "--socks5 h:1",
        ] {
            let r = import(&format!("curl {opt} https://api.example/x")).unwrap();
            assert_eq!(r.request.url, "https://api.example/x", "{opt}");
            assert_eq!(r.notes.len(), 1, "{opt}: {:?}", r.notes);
        }
    }

    #[test]
    fn export_quotes_method_and_typed_text_parts() {
        let req = HttpRequest {
            method: "GET; touch pwned #".into(),
            url: "http://h/x".into(),
            ..Default::default()
        };
        let out = export(&req, &Scope::new(), true, Path::new(""));
        assert!(out.contains("-X 'GET; TOUCH PWNED #'"), "{out}");

        let req = HttpRequest {
            method: "POST".into(),
            url: "http://h/x".into(),
            body: Body::Multipart {
                parts: vec![MultipartPart {
                    name: "note".into(),
                    kind: PartKind::Text {
                        value: "@dr\"a;ft".into(),
                    },
                    content_type: "text/plain".into(),
                    enabled: true,
                }],
            },
            ..Default::default()
        };
        let out = export(&req, &Scope::new(), true, Path::new(""));
        assert!(out.contains(r#"note="@dr\"a;ft";type=text/plain"#), "{out}");
        let back = import(&out).unwrap();
        let Body::Multipart { parts } = &back.request.body else {
            panic!("{out}")
        };
        assert_eq!(
            parts[0].kind,
            PartKind::Text {
                value: "@dr\"a;ft".into()
            }
        );
        assert_eq!(parts[0].content_type, "text/plain");
    }

    fn scope() -> Scope {
        let mut m = BTreeMap::new();
        m.insert("base".into(), VarValue::plain("https://api.x.com"));
        m.insert("tok".into(), VarValue::secret("S3CRET"));
        Scope::new().with_layer("env", m)
    }

    #[test]
    fn export_masks_secrets() {
        let req = HttpRequest {
            method: "POST".into(),
            url: "{{base}}/u".into(),
            params: vec![KeyValue::new("q", "a b")],
            headers: vec![KeyValue::new("X-T", "{{tok}}")],
            body: Body::Json {
                text: "{\"it's\": 1}".into(),
            },
            auth: Auth::Basic {
                username: "ana".into(),
                password: "{{tok}}".into(),
            },
            ..Default::default()
        };
        let masked = export(&req, &scope(), false, Path::new("/c"));
        assert!(!masked.contains("S3CRET"), "{masked}");
        assert!(masked.contains("https://api.x.com/u?q=a%20b"));
        assert!(masked.contains("-u 'ana:{{tok}}'"));
        assert!(masked.contains("'{\"it'\\''s\": 1}'"), "{masked}");
        let full = export(&req, &scope(), true, Path::new("/c"));
        assert!(full.contains("ana:S3CRET"));
        assert!(full.contains("X-T: S3CRET"));
    }

    #[test]
    fn roundtrip_simple() {
        let req = HttpRequest {
            method: "DELETE".into(),
            url: "http://h/x".into(),
            options: HttpOptions {
                follow_redirects: false,
                verify_tls: false,
                timeout_ms: Some(1500),
                cookies: None,
            },
            ..Default::default()
        };
        let c = export(&req, &Scope::new(), true, Path::new("/"));
        let back = import(&c).unwrap();
        assert_eq!(back.request, req, "{c}");
        assert_eq!(shell_quote("plain-1.2"), "plain-1.2");
        assert_eq!(shell_quote(""), "''");
    }
}
