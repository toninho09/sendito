//! Redirect following: hop method, origin rules and cookies per hop.

use super::*;

pub(super) const MAX_REDIRECTS: usize = 10;

/// The method and body of the next redirect hop (RFC 9110, 15.4): a 303
/// turns the request into a GET (a HEAD stays a HEAD), and so do 301 and 302
/// for a POST, as browsers do; the body is dropped then. 307 and 308 keep the
/// method and the body.
pub(super) fn next_hop_method(
    status: reqwest::StatusCode,
    method: &reqwest::Method,
) -> Option<(reqwest::Method, bool)> {
    use reqwest::{Method, StatusCode};
    match status {
        StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND if *method == Method::POST => {
            Some((Method::GET, true))
        }
        StatusCode::SEE_OTHER if *method == Method::HEAD => Some((Method::HEAD, true)),
        StatusCode::SEE_OTHER => Some((Method::GET, true)),
        StatusCode::MOVED_PERMANENTLY
        | StatusCode::FOUND
        | StatusCode::TEMPORARY_REDIRECT
        | StatusCode::PERMANENT_REDIRECT => Some((method.clone(), false)),
        _ => None,
    }
}

/// The collection cookie jar, seen from the redirect loop: it stores the
/// cookies a redirect response sets and gives the `Cookie` header for the
/// next hop's URL.
pub trait RedirectCookies: Send + Sync {
    fn hop<'a>(
        &'a self,
        from: &'a str,
        set_cookies: Vec<String>,
        to: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, String>> + Send + 'a>>;
}

pub(super) fn same_origin(a: &reqwest::Url, b: &reqwest::Url) -> bool {
    a.scheme() == b.scheme()
        && a.host_str() == b.host_str()
        && a.port_or_known_default() == b.port_or_known_default()
}

pub(super) fn redirect_target(resp: &reqwest::Response) -> Option<reqwest::Url> {
    let location = resp.headers().get(reqwest::header::LOCATION)?;
    resp.url().join(location.to_str().ok()?).ok()
}

/// The headers of a redirect hop. After the chain has left the request's
/// origin, credentials stay behind: auth headers and `Authorization`,
/// `Cookie` and the like. Without a body, the headers that describe it go too.
/// The jar's `Cookie` is replaced by `jar` (the jar's header for this hop).
pub(super) fn hop_headers(
    req: &ResolvedHttp,
    left_origin: bool,
    body_dropped: bool,
    jar: Option<Option<String>>,
) -> Result<HeaderMap, HttpError> {
    const PAYLOAD: [&str; 4] = [
        "content-type",
        "content-length",
        "content-encoding",
        "transfer-encoding",
    ];
    let mut kept: Vec<(String, String)> = req
        .headers
        .iter()
        .enumerate()
        .filter(|(i, (name, _))| {
            let lower = name.to_ascii_lowercase();
            let origin = req.header_origins.get(*i);
            let auth = origin == Some(&HeaderOrigin::Auth);
            let from_jar = origin == Some(&HeaderOrigin::Jar);
            let sensitive = matches!(
                lower.as_str(),
                "authorization" | "cookie" | "cookie2" | "proxy-authorization" | "www-authenticate"
            );
            !(jar.is_some() && from_jar)
                && !(left_origin && (auth || sensitive))
                && !(body_dropped && PAYLOAD.contains(&lower.as_str()))
        })
        .map(|(_, h)| h.clone())
        .collect();
    if let Some(Some(cookie)) = jar {
        kept.push(("Cookie".into(), cookie));
    }
    header_map(&kept)
}

/// Sends one hop of a redirect chain with the time the whole chain has left,
/// so following redirects does not restart the request's timeout.
pub(super) async fn execute_hop(
    client: &reqwest::Client,
    mut request: reqwest::Request,
    deadline: Option<Instant>,
    timeout: Duration,
) -> Result<reqwest::Response, HttpError> {
    let remaining = deadline.map_or(timeout, |d| d.saturating_duration_since(Instant::now()));
    if remaining.is_zero() {
        return Err(HttpError::Timeout(timeout.as_millis()));
    }
    *request.timeout_mut() = Some(remaining);
    client
        .execute(request)
        .await
        .map_err(|e| send_error(e, timeout))
}

/// Follows the redirects of `resp`, at most [`MAX_REDIRECTS`], each hop
/// within the time the chain has left. A 307 or 308 resends the body (a file
/// is read again); a multipart body is not resent, so its redirect response
/// is returned.
pub(super) async fn follow_redirects(
    client: &reqwest::Client,
    req: &ResolvedHttp,
    resp: reqwest::Response,
    deadline: Option<Instant>,
    timeout: Duration,
    cookies: Option<&dyn RedirectCookies>,
) -> Result<reqwest::Response, HttpError> {
    follow_redirects_with_mode(client, req, resp, deadline, timeout, cookies, false).await
}

pub(super) async fn follow_redirects_with_mode(
    client: &reqwest::Client,
    req: &ResolvedHttp,
    mut resp: reqwest::Response,
    deadline: Option<Instant>,
    timeout: Duration,
    cookies: Option<&dyn RedirectCookies>,
    streaming: bool,
) -> Result<reqwest::Response, HttpError> {
    let mut method = reqwest::Method::from_bytes(req.method.as_bytes())
        .map_err(|_| HttpError::Method(req.method.clone()))?;
    let (mut dropped, mut left_origin) = (false, false);
    for hop in 0.. {
        let Some((next, drop_body)) = next_hop_method(resp.status(), &method) else {
            return Ok(resp);
        };
        let Some(target) = redirect_target(&resp) else {
            return Ok(resp);
        };
        dropped |= drop_body;
        if !dropped && matches!(req.body, ResolvedBody::Multipart(_)) {
            return Ok(resp);
        }
        if hop >= MAX_REDIRECTS {
            return Err(HttpError::Send(format!(
                "too many redirects (more than {MAX_REDIRECTS})"
            )));
        }
        left_origin |= !same_origin(&target, resp.url());
        let jar = match cookies {
            Some(c) => {
                let sets: Vec<String> = resp
                    .headers()
                    .get_all(reqwest::header::SET_COOKIE)
                    .iter()
                    .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
                    .collect();
                Some(
                    c.hop(resp.url().as_str(), sets, target.as_str())
                        .await
                        .map_err(HttpError::Send)?,
                )
            }
            None => None,
        };
        method = next;
        let headers = hop_headers(req, left_origin, dropped, jar)?;
        let body = if dropped {
            &ResolvedBody::None
        } else {
            &req.body
        };
        let request = build_request(client, method.clone(), target, headers, body).await?;
        resp = if streaming {
            // The caller bounds opening. No body timeout may survive the hop.
            client
                .execute(request)
                .await
                .map_err(|e| send_error(e, timeout))?
        } else {
            execute_hop(client, request, deadline, timeout).await?
        };
    }
    Ok(resp)
}
