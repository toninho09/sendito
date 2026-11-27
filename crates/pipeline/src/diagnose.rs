//! Why a send failed and what to change. Each runner calls a classifier
//! where it turns an error into text; the TUI and the CLI show the [`Hint`].
//! The classifiers read the error text: the protocol crates and their
//! dependencies flatten most errors to strings before the pipeline sees them.

use crate::runner::RunCtx;

/// A configuration problem and its fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hint {
    /// A few words, for the status line.
    pub short: &'static str,
    /// The sentence with the setting to change, for the response pane and
    /// the reports.
    pub fix: String,
}

/// What the classifiers need to know about the request and the config.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    /// `[options] timeout_ms` of the request; `None` when it sets none.
    pub timeout_ms: Option<u64>,
    /// The configured default timeout in ms; `0` is no timeout.
    pub default_timeout_ms: u64,
    /// The config key of that default, such as `[http] timeout_ms`.
    pub timeout_key: &'static str,
    /// The request's `verify_tls`; `None` when the kind has no such option.
    pub verify_tls: Option<bool>,
    /// `[http.tls] ca` names a PEM file.
    pub custom_ca: bool,
    /// `[http] proxy` is set.
    pub proxy: bool,
    /// The active environment.
    pub env: Option<&'a str>,
    /// The request sends a token of `[oauth.<name>]`.
    pub oauth: Option<&'a str>,
    /// The gRPC deadline in ms; `None` is no deadline.
    pub grpc_deadline_ms: Option<u64>,
    /// The host the request goes to, when its error text does not name it.
    pub host: Option<&'a str>,
}

impl Hint {
    fn new(short: &'static str, fix: impl Into<String>) -> Self {
        Self {
            short,
            fix: fix.into(),
        }
    }
}

impl<'a> Context<'a> {
    /// The `[http]` keys and the environment and proxy of the run.
    pub fn of(ctx: &'a RunCtx) -> Self {
        Self {
            default_timeout_ms: ctx.send.default_timeout.as_millis() as u64,
            timeout_key: "[http] timeout_ms",
            custom_ca: ctx.send.tls.ca.is_some(),
            proxy: ctx.send.proxy.is_some(),
            env: ctx.env.as_deref(),
            ..Default::default()
        }
    }

    fn env(&self) -> String {
        match self.env {
            Some(e) => format!("the {e} environment"),
            None => "the collection environment".to_string(),
        }
    }

    fn effective_timeout_ms(&self) -> u64 {
        self.timeout_ms.unwrap_or(self.default_timeout_ms)
    }
}

/// A short, plain reason for an error: a few words for the status line
/// when the error has no [`Hint`].
pub fn label(msg: &str) -> &'static str {
    let l = msg.to_lowercase();
    if refused(&l) {
        "connection refused"
    } else if dns(&l) {
        "host name not found (DNS)"
    } else if timeout(&l) {
        "timed out"
    } else if l.contains("certificate") || tls(&l) {
        "TLS failure"
    } else if closed(&l) {
        "connection closed by the server"
    } else if file(&l) {
        "file not found"
    } else {
        "request failed"
    }
}

/// The first line of a failure: what happened, in one sentence.
pub fn explain(msg: &str) -> String {
    let l = msg.to_lowercase();
    if refused(&l) {
        match host_of(&l) {
            Some(h) => format!("nothing is listening at {h} (connection refused)."),
            None => "nothing is listening at the address (connection refused).".to_string(),
        }
    } else if dns(&l) {
        "the host name could not be resolved (DNS).".into()
    } else if timeout(&l) {
        "the server did not answer in time (timeout).".into()
    } else if l.contains("certificate") || tls(&l) {
        "the TLS connection failed (certificate or handshake).".into()
    } else if closed(&l) {
        "the server closed the connection.".into()
    } else if file(&l) {
        "a file the request sends was not found.".into()
    } else {
        "see the details below.".into()
    }
}

/// Classifies a transport error of any protocol: TLS, timeout, connection,
/// DNS, proxy, a closed connection, a file or a URL. `None` when the text
/// names none of them.
pub fn transport(msg: &str, cx: &Context<'_>) -> Option<Hint> {
    let l = msg.to_lowercase();
    if l.contains("tls setup failed") {
        return Some(Hint::new(
            "TLS setup failed",
            format!(
                "Check the files in `[http.tls]`: {}",
                after(&l, "tls setup failed: ").unwrap_or("").trim()
            ),
        ));
    }
    if l.contains("certificate") {
        return Some(if cx.verify_tls == Some(false) {
            handshake()
        } else {
            untrusted(cx)
        });
    }
    if timeout(&l) {
        return Some(timeout_hint(cx));
    }
    if tls(&l) {
        return Some(handshake());
    }
    if refused(&l) {
        let mut fix = match host_of(&l).or_else(|| cx.host.map(str::to_string)) {
            Some(h) => format!("Nothing listens at {h}."),
            None => "Nothing listens at the address of the request.".to_string(),
        };
        fix.push_str(&format!(
            " Check the port and the URL variables of {}.",
            cx.env()
        ));
        if cx.proxy {
            fix.push_str(PROXY_NOTE);
        }
        return Some(Hint::new("connection refused", fix));
    }
    if dns(&l) {
        let mut fix = match host_of(&l).or_else(|| cx.host.map(str::to_string)) {
            Some(h) => format!("The host name {h} does not resolve."),
            None => "The host name does not resolve.".to_string(),
        };
        fix.push_str(&format!(
            " Check it in the URL and in the variables of {}.",
            cx.env()
        ));
        if cx.proxy {
            fix.push_str(PROXY_NOTE);
        }
        return Some(Hint::new("host name not found", fix));
    }
    if l.contains("proxy") {
        return Some(Hint::new(
            "proxy error",
            "Check `[http] proxy` in the config and the `HTTPS_PROXY` variable, or remove the proxy.",
        ));
    }
    if closed(&l) {
        return Some(Hint::new(
            "connection closed by the server",
            "The server closed the connection. Check that the scheme matches the port: a TLS scheme on a plaintext port (or the reverse) closes at once.",
        ));
    }
    if file(&l) {
        return Some(Hint::new(
            "file not found",
            format!(
                "The file {} of the request does not exist. Use a path relative to the collection root.",
                path_of(msg)
            ),
        ));
    }
    if l.contains("invalid url")
        || l.contains("invalid mcp server url")
        || l.contains("empty host")
        || l.contains("relative url")
    {
        return Some(Hint::new(
            "invalid URL",
            "The URL is not valid. An unresolved `{{variable}}` stays as typed: select an environment that defines it, or define the variable.",
        ));
    }
    if let Some(rest) = after(&l, "http error: ") {
        let status = rest
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .unwrap_or("");
        return Some(match status {
            "401" | "403" => Hint::new(
                "handshake refused (authentication)",
                format!(
                    "The handshake was refused (HTTP {status}). Add the credentials the server needs: an `Authorization` header in the request, or the cookie jar (`[options] cookies`)."
                ),
            ),
            _ => Hint::new(
                "not a WebSocket endpoint",
                format!(
                    "The URL does not speak WebSocket (HTTP {status}). Use the WebSocket URL of the server, with `ws://` or `wss://`."
                ),
            ),
        });
    }
    if l.contains("invalid websocket url") {
        return Some(Hint::new(
            "invalid WebSocket URL",
            "Use `ws://host/path` or `wss://host/path`.",
        ));
    }
    None
}

const PROXY_NOTE: &str =
    " The request goes through the proxy of `[http] proxy`: check that the proxy runs.";

/// The hint for a response status that points at a configuration problem:
/// `407`, and `401`/`403` when the request sends an OAuth token.
pub fn status(status: u16, cx: &Context<'_>) -> Option<Hint> {
    match status {
        407 => Some(Hint::new(
            "proxy needs credentials",
            "The proxy needs credentials. Put them in the `[http] proxy` URL (`http://user:password@host:port`), or in the `HTTPS_PROXY` variable.",
        )),
        401 | 403 if cx.oauth.is_some() => {
            let name = cx.oauth.unwrap_or_default();
            Some(Hint::new(
                "token refused",
                format!(
                    "The server refuses the token of identity `{name}`. Check `scopes` in `[oauth.{name}]`, or sign in again (`:auth login` in the app, `sendito auth login` on the command line)."
                ),
            ))
        }
        _ => None,
    }
}

/// Classifies an error of the OAuth setup of an HTTP request: the identity
/// is not configured, the sign-in is missing or the token endpoint refused.
pub fn oauth(msg: &str) -> Option<Hint> {
    let l = msg.to_lowercase();
    let (short, fix) = if l.contains("oauth is not configured") {
        (
            "OAuth is not configured",
            "Add `[oauth.<name>]` to the config (the request's `auth` names the identity), or remove the OAuth `auth` of the request.".to_string(),
        )
    } else if l.contains("oauth identity is not defined") {
        (
            "OAuth identity is not defined",
            "Define the identity in `[oauth.<name>]` of the config, or change the `auth` of the request.".to_string(),
        )
    } else if l.contains("require https") {
        (
            "OAuth needs HTTPS",
            "Use an `https://` URL, or remove the OAuth `auth` of the request.".to_string(),
        )
    } else if l.contains("token unavailable") || l.contains("sign in again") {
        (
            "sign in required",
            "Sign in again: `:auth login <identity>` in the app, `sendito auth login <identity>` on the command line.".to_string(),
        )
    } else if l.contains("oauth token request failed") {
        (
            "token endpoint refused the request",
            "The token endpoint refused the request. Check `client_id`, `client_secret` and `token_url` in `[oauth.<name>]`.".to_string(),
        )
    } else {
        return None;
    };
    Some(Hint::new(short, fix))
}

/// The hint for a gRPC status code: deadline, `UNIMPLEMENTED`,
/// `UNAVAILABLE` and `UNAUTHENTICATED` name a setting to change. `NOT_FOUND`
/// is the application's "entity not found" most of the time, so it gets a
/// hint only when its message names the service or the method.
pub fn grpc(code: i32, message: &str, cx: &Context<'_>) -> Option<Hint> {
    match code {
        4 => cx.grpc_deadline_ms.map(|ms| {
            Hint::new(
                "deadline too short",
                format!(
                    "The deadline of {ms} ms is too short for this call. Raise `[options] deadline_ms` in the request or `[grpc] deadline_ms` in the config."
                ),
            )
        }),
        5 => unknown_method(message),
        12 => Some(Hint::new(
            "method not implemented, or wrong scheme",
            "The server does not implement the method, or the scheme of `target` is wrong: `http://` is plaintext and `https://` (or `grpcs://`) is TLS. Check the service and method names, and `[schema] files` when the request uses files.",
        )),
        14 => Some(Hint::new(
            "server not reachable",
            if cx.proxy {
                "The server is not reachable. gRPC does not use `[http] proxy`: the server must be reachable directly. Check the host and port of `target`."
            } else {
                "The server is not reachable now. Check the host and port of `target`."
            },
        )),
        16 => Some(Hint::new(
            "credentials refused",
            "The server refuses the credentials. Check `[auth]` of the request and the `authorization` metadata it sends.",
        )),
        _ => None,
    }
}

/// `NOT_FOUND` with a message that names the service or the method.
fn unknown_method(message: &str) -> Option<Hint> {
    let message = message.trim().to_ascii_lowercase();
    ["unknown service", "unknown method", "service not found", "method not found"]
        .iter()
        .any(|prefix| {
            message.strip_prefix(prefix).is_some_and(|rest| {
                rest.is_empty() || rest.starts_with([' ', ':', '.'])
            })
        })
        .then(|| {
            Hint::new(
                "service or method not found",
                "Check `service` and `method` against the server schema. If the request uses `[schema] files`, check that the `.proto` files match the server.",
            )
        })
}

/// Classifies an error of a gRPC call before the status arrives: schema,
/// reflection, a connection that is not gRPC, then [`transport`].
pub fn grpc_message(msg: &str, cx: &Context<'_>) -> Option<Hint> {
    let l = msg.to_lowercase();
    if l.contains("reflection failed") {
        return Some(Hint::new(
            "reflection failed",
            "The server has no reflection or refuses it. Set `[schema] source = \"files\"` with `files` in the request, or check the `[metadata]` the reflection needs.",
        ));
    }
    if l.contains("no .proto files configured") {
        return Some(Hint::new(
            "no .proto files",
            "Set `files` in `[schema]` of the request.",
        ));
    }
    if l.contains("schema:") {
        return Some(Hint::new(
            "schema problem",
            "Check `files` and `import_paths` in `[schema]` of the request, and the `import` lines of the `.proto` files.",
        ));
    }
    if l.contains("invalid content-type") {
        return Some(Hint::new(
            "not a gRPC port, or wrong scheme",
            "The server does not speak gRPC on this port, or the scheme is wrong. `http://` is plaintext and `https://` is TLS: check `target` and `[options] tls`.",
        ));
    }
    transport(msg, cx)
}

/// Classifies an MQTT error: the CONNACK code of a refusal, then
/// [`transport`]. `version` is the protocol of the request (`3.1.1` or `5`).
pub fn mqtt(msg: &str, cx: &Context<'_>, version: &str, client_id: &str) -> Option<Hint> {
    let l = msg.to_lowercase();
    if let Some(code) = after(&l, "connack refused: ") {
        let env = cx.env();
        let (short, fix) = match code.trim() {
            "refusedprotocolversion" | "unsupportedprotocolversion" => (
                "wrong MQTT version",
                format!(
                    "The broker does not accept MQTT {version}. Set `protocol = \"3.1.1\"` or `\"5\"` in the request."
                ),
            ),
            "badusernamepassword" | "notauthorized" | "badauthenticationmethod" => (
                "broker refuses the credentials",
                format!(
                    "The broker refuses the credentials. Check `username` and `password` of the request and the variables of {env}."
                ),
            ),
            "badclientid" | "clientidentifiernotvalid" => (
                "client id refused",
                format!(
                    "The broker refuses the client id `{client_id}`. Change `client_id` of the request."
                ),
            ),
            "serviceunavailable" | "serverunavailable" | "serverbusy" => (
                "broker takes no connection",
                "The broker is up but takes no connection now. Try again later.".to_string(),
            ),
            other => (
                "connection refused by the broker",
                format!("The broker refused the connection (CONNACK {other})."),
            ),
        };
        return Some(Hint::new(short, fix));
    }
    if l.contains("no connack within") {
        return Some(Hint::new(
            "no CONNACK in time",
            format!(
                "The broker did not answer in time. Raise `[options] timeout_ms` or `{}` in the config, and check the port: 1883 for `mqtt://`, 8883 for `mqtts://`.",
                cx.timeout_key
            ),
        ));
    }
    transport(msg, cx)
}

/// Classifies an MCP error: the server command, the server entry, the
/// HTTP status of the transport, OAuth discovery, then [`transport`].
/// `server` is the resolved `server` of the request.
pub fn mcp(msg: &str, cx: &Context<'_>, server: &str) -> Option<Hint> {
    let l = msg.to_lowercase();
    let configured = !(server.starts_with("http://") || server.starts_with("https://"));
    let key = || format!("[mcp.servers.{server}]");
    if let Some(rest) = after(&l, "cannot start ") {
        let command = rest.split(": ").next().unwrap_or("").trim_matches('"');
        let (short, fix) = if rest.contains("no such file") || rest.contains("os error 2") {
            (
                "command not found",
                format!(
                    "The command `{command}` is not in PATH. Check `command` and `args` of {}, or use an absolute path.",
                    key()
                ),
            )
        } else {
            (
                "command did not start",
                format!(
                    "The command `{command}` did not start. Check `command` and `args` of {}.",
                    key()
                ),
            )
        };
        return Some(Hint::new(short, fix));
    }
    if l.contains("is not defined in [mcp.servers]") {
        return Some(Hint::new(
            "server not configured",
            format!(
                "Add `[mcp.servers.{server}]` to the config, or use an `https://` URL as `server`."
            ),
        ));
    }
    let http_status = after(&l, "http ").and_then(|rest| {
        let digits = rest.split(|c: char| !c.is_ascii_digit()).next()?;
        (digits.len() == 3).then(|| digits.to_string())
    });
    match http_status.as_deref() {
        Some("401") | Some("403") => {
            return Some(Hint::new(
                "credentials refused",
                format!(
                    "The server refuses the credentials. Check `headers` and `auth` of {}.",
                    key()
                ),
            ));
        }
        Some("404") => {
            return Some(Hint::new(
                "wrong server URL",
                format!(
                    "The server has no MCP endpoint at this URL. Check the `url` of {} (the endpoint often ends with `/mcp`).",
                    key()
                ),
            ));
        }
        _ => {}
    }
    if configured
        && [
            "www-authenticate",
            "no authorization_servers",
            "no authorization_endpoint",
            "no token_endpoint",
            "oauth endpoint url",
        ]
        .iter()
        .any(|m| l.contains(m))
    {
        return Some(Hint::new(
            "OAuth discovery failed",
            format!(
                "The server does not publish OAuth metadata. Set `authorize_url` and `token_url` in {}.auth.",
                key()
            ),
        ));
    }
    if l.contains("client_id is required") {
        return Some(Hint::new(
            "OAuth client_id missing",
            format!("Set `client_id` in {}.auth.", key()),
        ));
    }
    if l.contains("json-rpc error -32601") {
        return Some(Hint::new(
            "operation not on the server",
            "The server does not offer this tool, resource or prompt. Refresh the list and check `tool`, `uri` or `prompt`.",
        ));
    }
    if l.contains("invalid json from the server") {
        return Some(Hint::new(
            "not an MCP endpoint",
            "The server did not answer JSON. Check that the `server` URL is the MCP endpoint.",
        ));
    }
    transport(msg, cx)
}

/// The request did not run: variables are unresolved.
pub fn unresolved() -> Hint {
    Hint::new(
        "unresolved variables",
        "Define the variables in the environment file, or select an environment that defines them (`:env` in the app, `--env` on the command line).",
    )
}

/// The URL keeps a `{{variable}}` the scope does not define.
pub fn unresolved_url(url: &str) -> Option<Hint> {
    let start = url.find("{{")? + 2;
    let name = url[start..].split_once("}}").map(|(n, _)| n)?.trim();
    Some(Hint::new(
        "unresolved variable in the URL",
        format!(
            "The URL keeps `{{{{{name}}}}}`. Select an environment that defines it, or define the variable."
        ),
    ))
}

fn refused(l: &str) -> bool {
    l.contains("connection refused") || l.contains("os error 111") || l.contains("os error 61")
}

fn dns(l: &str) -> bool {
    [
        "dns error",
        "failed to lookup",
        "name or service not known",
        "name resolution",
        "no addresses",
        "getaddrinfo",
        "nodename",
    ]
    .iter()
    .any(|m| l.contains(m))
}

fn timeout(l: &str) -> bool {
    l.contains("timed out") || l.contains("timeout")
}

fn tls(l: &str) -> bool {
    l.contains("tls") || l.contains("handshake") || l.contains("fatal alert") || l.contains("ssl")
}

fn closed(l: &str) -> bool {
    [
        "connection reset",
        "broken pipe",
        "connection closed",
        "unexpected eof",
        "connection aborted",
        "incomplete message",
    ]
    .iter()
    .any(|m| l.contains(m))
}

fn file(l: &str) -> bool {
    l.contains("no such file") || l.contains("os error 2") || l.contains("cannot read ")
}

fn timeout_hint(cx: &Context<'_>) -> Hint {
    let ms = cx.effective_timeout_ms();
    let fix = if ms == 0 {
        "The server did not answer in time. Check that the host and the port are correct."
            .to_string()
    } else if cx.timeout_ms.is_some() {
        format!(
            "The timeout of {ms} ms is too short. Set a larger `[options] timeout_ms` in the request, or remove it to use `{}` in the config.",
            cx.timeout_key
        )
    } else {
        format!(
            "The timeout of {ms} ms is too short. Set a larger `{}` in the config, or `[options] timeout_ms` in the request.",
            cx.timeout_key
        )
    };
    Hint::new("timeout too short", fix)
}

fn untrusted(cx: &Context<'_>) -> Hint {
    Hint::new(
        "certificate not trusted",
        if cx.custom_ca {
            "The server certificate is not signed by the CA in `[http.tls] ca`, or is expired or not valid for its name. Check the `ca` file, or set `verify_tls = false` in `[options]` for a test server."
        } else {
            "The server certificate is not trusted, expired or not valid for its name. Add the CA to `[http.tls] ca`, or set `verify_tls = false` in `[options]` for a test server."
        },
    )
}

fn handshake() -> Hint {
    Hint::new(
        "TLS handshake failed",
        "Check that the scheme matches the port (`https://`, `wss://` and `mqtts://` for TLS), and the client certificate in `[http.tls]` when the server asks for one.",
    )
}

/// The text after the first `marker`.
fn after<'a>(l: &'a str, marker: &str) -> Option<&'a str> {
    l.split_once(marker).map(|(_, rest)| rest)
}

/// The host (with its port) that an error names: the URL of a reqwest error
/// (`for url (https://host:port/path)`) or the target of a connect error
/// (`cannot connect to ws://host:port`).
fn host_of(l: &str) -> Option<String> {
    let target = if let Some(rest) = after(l, "for url (") {
        rest.split(')').next()?
    } else {
        after(l, "connect to ")?.split(": ").next()?
    };
    let host = target.rsplit("://").next()?.split('/').next()?;
    (!host.is_empty()).then(|| host.to_string())
}

/// The path of a file an error names (`cannot read <path>: …`), else the
/// file of the request.
fn path_of(msg: &str) -> &str {
    after(msg, "cannot read ")
        .and_then(|rest| rest.split_once(": ").map(|(p, _)| p))
        .unwrap_or("the request sends")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> Context<'static> {
        Context {
            default_timeout_ms: 30_000,
            timeout_key: "[http] timeout_ms",
            verify_tls: Some(true),
            env: Some("dev"),
            ..Default::default()
        }
    }

    fn fix(h: Option<Hint>) -> String {
        h.unwrap_or_else(|| panic!("no hint")).fix
    }

    #[test]
    fn connection_refused_names_the_host_and_the_environment() {
        let e = "error sending request for url (https://localhost:1/): client error (Connect): tcp connect error: Connection refused (os error 61)";
        let h = transport(e, &base()).unwrap();
        assert_eq!(h.short, "connection refused");
        assert_eq!(
            h.fix,
            "Nothing listens at localhost:1. Check the port and the URL variables of the dev environment."
        );
        assert_eq!(label(e), "connection refused");
        assert_eq!(
            explain(e),
            "nothing is listening at localhost:1 (connection refused)."
        );
        // A connect error of WebSocket or gRPC names the target after "connect to".
        let e = "cannot connect to ws://localhost:1: tcp connect error: Connection refused (os error 111)";
        assert!(fix(transport(e, &base())).starts_with("Nothing listens at localhost:1."));
    }

    #[test]
    fn a_set_proxy_is_named_after_a_connection_error() {
        let with_proxy = Context {
            proxy: true,
            ..base()
        };
        let h = transport("I/O: Connection refused (os error 111)", &with_proxy).unwrap();
        assert_eq!(
            h.fix,
            "Nothing listens at the address of the request. Check the port and the URL variables of the dev environment. The request goes through the proxy of `[http] proxy`: check that the proxy runs."
        );
    }

    #[test]
    fn dns_failure_names_the_host() {
        let e = "error sending request for url (https://nope.invalid/): error trying to connect: dns error: failed to lookup address information: nodename nor servname provided, or not known";
        let h = transport(e, &base()).unwrap();
        assert_eq!(h.short, "host name not found");
        assert_eq!(
            h.fix,
            "The host name nope.invalid does not resolve. Check it in the URL and in the variables of the dev environment."
        );
        assert_eq!(label(e), "host name not found (DNS)");
        let e = "cannot connect to ws://nope.invalid:80: name resolution: failed to lookup address information";
        assert!(
            fix(transport(e, &base()))
                .starts_with("The host name nope.invalid:80 does not resolve.")
        );
    }

    #[test]
    fn timeout_names_the_key_that_set_it() {
        let h = transport("request timed out after 100 ms", &base()).unwrap();
        assert_eq!(h.short, "timeout too short");
        assert_eq!(
            h.fix,
            "The timeout of 30000 ms is too short. Set a larger `[http] timeout_ms` in the config, or `[options] timeout_ms` in the request."
        );
        let cx = Context {
            timeout_ms: Some(100),
            ..base()
        };
        assert_eq!(
            fix(transport("request timed out after 100 ms", &cx)),
            "The timeout of 100 ms is too short. Set a larger `[options] timeout_ms` in the request, or remove it to use `[http] timeout_ms` in the config."
        );
        // A handshake or schema load that did not finish in time is a timeout too.
        assert_eq!(
            transport("handshake timed out", &cx).unwrap().short,
            "timeout too short"
        );
        assert_eq!(
            transport(
                "the schema load or the connection took more than 1000 ms ([http] timeout_ms)",
                &cx
            )
            .unwrap()
            .short,
            "timeout too short"
        );
        assert_eq!(
            transport("timeout after 5s", &cx).unwrap().short,
            "timeout too short"
        );
    }

    #[test]
    fn an_untrusted_certificate_points_at_the_ca_or_verify_tls() {
        let e = "error sending request for url (https://localhost:8443/): error trying to connect: invalid peer certificate: UnknownIssuer";
        let h = transport(e, &base()).unwrap();
        assert_eq!(h.short, "certificate not trusted");
        assert!(h.fix.contains("Add the CA to `[http.tls] ca`"), "{}", h.fix);
        assert!(h.fix.contains("`verify_tls = false`"), "{}", h.fix);
        assert_eq!(label(e), "TLS failure");
        // With a custom CA, the CA file is the first thing to check.
        let cx = Context {
            custom_ca: true,
            ..base()
        };
        assert!(fix(transport(e, &cx)).contains("Check the `ca` file"));
        // With verification off, the certificate word is a handshake problem (a client certificate).
        let cx = Context {
            verify_tls: Some(false),
            ..base()
        };
        assert_eq!(transport(e, &cx).unwrap().short, "TLS handshake failed");
    }

    #[test]
    fn tls_setup_and_handshake_failures() {
        let h = transport(
            "TLS setup failed: cannot read ca.pem: No such file or directory (os error 2)",
            &base(),
        )
        .unwrap();
        assert_eq!(h.short, "TLS setup failed");
        assert_eq!(
            h.fix,
            "Check the files in `[http.tls]`: cannot read ca.pem: no such file or directory (os error 2)"
        );
        let h = transport(
            "cannot connect to wss://localhost:1: tls handshake eof",
            &base(),
        )
        .unwrap();
        assert_eq!(h.short, "TLS handshake failed");
        assert!(
            h.fix
                .contains("`https://`, `wss://` and `mqtts://` for TLS")
        );
    }

    #[test]
    fn files_urls_and_closed_connections() {
        let h = transport(
            "cannot read body.json: No such file or directory (os error 2)",
            &base(),
        )
        .unwrap();
        assert_eq!(h.short, "file not found");
        assert!(
            h.fix
                .starts_with("The file body.json of the request does not exist.")
        );
        let h = transport("invalid URL \"https://h/{{x}}\": empty host", &base()).unwrap();
        assert_eq!(h.short, "invalid URL");
        let h = transport("error reading body: connection reset by peer", &base()).unwrap();
        assert_eq!(h.short, "connection closed by the server");
        assert_eq!(
            label("connection reset by peer"),
            "connection closed by the server"
        );
        assert!(transport("cookie jar task failed", &base()).is_none());
    }

    #[test]
    fn a_websocket_handshake_refusal_names_the_status() {
        let h = transport(
            "cannot connect to ws://localhost:8080/socket: HTTP error: 401 Unauthorized",
            &base(),
        )
        .unwrap();
        assert_eq!(h.short, "handshake refused (authentication)");
        assert!(h.fix.contains("(HTTP 401)"));
        let h = transport(
            "cannot connect to ws://localhost:8080: HTTP error: 426 Upgrade Required",
            &base(),
        )
        .unwrap();
        assert_eq!(h.short, "not a WebSocket endpoint");
        assert_eq!(
            transport(
                "invalid WebSocket URL \"localhost:8080\": use ws://host/path or wss://host/path",
                &base()
            )
            .unwrap()
            .short,
            "invalid WebSocket URL"
        );
    }

    #[test]
    fn http_status_hints() {
        assert_eq!(
            status(407, &base()).unwrap().short,
            "proxy needs credentials"
        );
        assert!(status(401, &base()).is_none());
        let cx = Context {
            oauth: Some("acme"),
            ..base()
        };
        let h = status(403, &cx).unwrap();
        assert_eq!(h.short, "token refused");
        assert!(h.fix.contains("`[oauth.acme]`"), "{}", h.fix);
        assert!(status(500, &cx).is_none());
    }

    #[test]
    fn oauth_setup_errors() {
        assert_eq!(
            oauth("OAuth is not configured").unwrap().short,
            "OAuth is not configured"
        );
        assert_eq!(
            oauth("OAuth identity is not defined in [oauth]")
                .unwrap()
                .short,
            "OAuth identity is not defined"
        );
        assert_eq!(
            oauth("OAuth requests require HTTPS except on loopback")
                .unwrap()
                .short,
            "OAuth needs HTTPS"
        );
        assert_eq!(oauth("OAuth token unavailable (no token); run sendito auth login for this identity and environment").unwrap().short, "sign in required");
        assert_eq!(
            oauth("OAuth refresh failed (expired); sign in again")
                .unwrap()
                .short,
            "sign in required"
        );
        assert_eq!(
            oauth("OAuth token request failed: HTTP 400").unwrap().short,
            "token endpoint refused the request"
        );
        assert!(oauth("the server closed the connection").is_none());
    }

    #[test]
    fn grpc_status_codes() {
        assert!(grpc(4, "Deadline Exceeded", &base()).is_none());
        let cx = Context {
            grpc_deadline_ms: Some(2000),
            ..base()
        };
        assert_eq!(
            fix(grpc(4, "Deadline Exceeded", &cx)),
            "The deadline of 2000 ms is too short for this call. Raise `[options] deadline_ms` in the request or `[grpc] deadline_ms` in the config."
        );
        assert_eq!(
            grpc(12, "", &cx).unwrap().short,
            "method not implemented, or wrong scheme"
        );
        assert_eq!(grpc(16, "", &cx).unwrap().short, "credentials refused");
        assert_eq!(
            grpc(14, "", &cx).unwrap().fix,
            "The server is not reachable now. Check the host and port of `target`."
        );
        let cx = Context {
            proxy: true,
            ..base()
        };
        assert!(fix(grpc(14, "", &cx)).contains("gRPC does not use `[http] proxy`"));
        assert!(grpc(7, "", &cx).is_none());
    }

    #[test]
    fn grpc_not_found_only_for_an_unknown_service_or_method() {
        let cx = base();
        let h = grpc(5, "unknown service sendito.test.v1.Echoz", &cx).unwrap();
        assert_eq!(h.short, "service or method not found");
        assert!(
            h.fix
                .contains("Check `service` and `method` against the server schema"),
            "{}",
            h.fix
        );
        assert_eq!(
            grpc(5, "Unknown method: Unarry", &cx).unwrap().short,
            "service or method not found"
        );
        // The application's "entity not found" is a result, not a setting problem.
        assert!(grpc(5, "user 42 not found", &cx).is_none());
        assert!(grpc(5, "payment method not found", &cx).is_none());
        assert!(grpc(5, "", &cx).is_none());
    }

    #[test]
    fn grpc_messages_before_the_status() {
        assert_eq!(
            grpc_message("reflection failed: connection closed", &base())
                .unwrap()
                .short,
            "reflection failed"
        );
        assert_eq!(
            grpc_message("schema: no .proto files configured", &base())
                .unwrap()
                .short,
            "no .proto files"
        );
        assert_eq!(
            grpc_message("schema: protos/a.proto: import not found", &base())
                .unwrap()
                .short,
            "schema problem"
        );
        assert_eq!(grpc_message("cannot connect to http://localhost:1: transport error: invalid content-type: text/html", &base()).unwrap().short, "not a gRPC port, or wrong scheme");
        assert_eq!(grpc_message("cannot connect to http://localhost:1: transport error: tcp connect error: Connection refused (os error 111)", &base()).unwrap().short, "connection refused");
        assert!(grpc_message("cancelled", &base()).is_none());
    }

    #[test]
    fn mqtt_connack_codes() {
        let m = |e: &str| mqtt(e, &base(), "3.1.1", "sensor-1").unwrap();
        let h = m("CONNACK refused: RefusedProtocolVersion");
        assert_eq!(h.short, "wrong MQTT version");
        assert_eq!(
            h.fix,
            "The broker does not accept MQTT 3.1.1. Set `protocol = \"3.1.1\"` or `\"5\"` in the request."
        );
        let h = m("CONNACK refused: BadUserNamePassword");
        assert_eq!(h.short, "broker refuses the credentials");
        assert!(h.fix.ends_with("Check `username` and `password` of the request and the variables of the dev environment."));
        let h = m("CONNACK refused: BadClientId");
        assert_eq!(
            h.fix,
            "The broker refuses the client id `sensor-1`. Change `client_id` of the request."
        );
        assert_eq!(
            m("CONNACK refused: ServerBusy").short,
            "broker takes no connection"
        );
        assert_eq!(
            m("CONNACK refused: Banned").short,
            "connection refused by the broker"
        );
        let broker = Context {
            host: Some("broker.local:1883"),
            timeout_key: "[mqtt] timeout_ms",
            ..base()
        };
        let h = mqtt(
            "the broker sent no CONNACK within 1000 ms",
            &broker,
            "5",
            "c",
        )
        .unwrap();
        assert_eq!(h.short, "no CONNACK in time");
        assert!(h.fix.contains("`[mqtt] timeout_ms`"));
        // The other errors are transport errors; the broker is the host.
        let h = mqtt("I/O: Connection refused (os error 111)", &broker, "5", "c").unwrap();
        assert!(h.fix.starts_with("Nothing listens at broker.local:1883."));
        assert_eq!(
            mqtt("I/O: TLS: handshake eof", &broker, "5", "c")
                .unwrap()
                .short,
            "TLS handshake failed"
        );
    }

    #[test]
    fn mcp_errors() {
        let m = |e: &str| mcp(e, &base(), "local").unwrap();
        let h = m("cannot start \"my-mcp-server\": No such file or directory (os error 2)");
        assert_eq!(h.short, "command not found");
        assert_eq!(
            h.fix,
            "The command `my-mcp-server` is not in PATH. Check `command` and `args` of [mcp.servers.local], or use an absolute path."
        );
        assert_eq!(
            m("cannot start \"x\": Permission denied (os error 13)").short,
            "command did not start"
        );
        let h = m("server \"local\" is not defined in [mcp.servers] of the config");
        assert_eq!(h.short, "server not configured");
        assert_eq!(
            m("transport: HTTP 401: unauthorized").short,
            "credentials refused"
        );
        assert_eq!(
            m("transport: HTTP 404: not found").short,
            "wrong server URL"
        );
        assert_eq!(
            m("the server sent no WWW-Authenticate resource_metadata").short,
            "OAuth discovery failed"
        );
        assert_eq!(
            m("OAuth client_id is required").short,
            "OAuth client_id missing"
        );
        assert_eq!(
            m("JSON-RPC error -32601: Method not found").short,
            "operation not on the server"
        );
        assert_eq!(
            m("invalid JSON from the server: <html>").short,
            "not an MCP endpoint"
        );
        assert_eq!(m("timeout after 5s").short, "timeout too short");
        assert!(mcp("the server closed the connection", &base(), "local").is_none());
        // A URL server has no config entry to check.
        let h = mcp(
            "transport: HTTP 401: unauthorized",
            &base(),
            "https://m.example/mcp",
        )
        .unwrap();
        assert!(h.fix.contains("https://m.example/mcp"), "{}", h.fix);
    }

    #[test]
    fn unresolved_variables() {
        assert_eq!(unresolved().short, "unresolved variables");
        let h = unresolved_url("https://h/{{base}}/x").unwrap();
        assert_eq!(h.short, "unresolved variable in the URL");
        assert_eq!(
            h.fix,
            "The URL keeps `{{base}}`. Select an environment that defines it, or define the variable."
        );
        assert!(unresolved_url("https://h/x").is_none());
    }
}
