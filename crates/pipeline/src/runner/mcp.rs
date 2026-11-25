//! Calls an MCP tool.

use super::*;

/// MCP run settings taken from the project: the `[mcp]` config section, the
/// keychain (OAuth tokens) and the collection name (keychain accounts).
pub struct McpRunInfo {
    pub timeout_ms: u64,
    pub servers: BTreeMap<String, sendito_store::config::McpServer>,
    pub keychain: Arc<dyn Keychain>,
    pub collection: String,
    /// `[http] proxy`, also used by MCP servers over Streamable HTTP.
    pub proxy: Option<sendito_http::ProxySpec>,
    /// `[http] tls` (custom CA, client certificate).
    pub tls: sendito_http::TlsMaterial,
}

impl McpRunInfo {
    pub fn of(p: &Project) -> Self {
        Self {
            timeout_ms: p.config.mcp.timeout_ms,
            servers: p.config.mcp.servers.clone(),
            keychain: p.envs.keychain.clone(),
            collection: p.name.clone(),
            proxy: p.send_options().proxy,
            tls: p.tls.clone(),
        }
    }

    pub(super) fn network(&self) -> sendito_mcp::oauth::Network {
        sendito_mcp::oauth::Network {
            tls: self.tls.clone(),
            proxy: self.proxy.clone(),
        }
    }
}

/// The keychain account an MCP server's tokens live under.
pub fn mcp_token_account(collection: &str, server: &str) -> String {
    format!("{collection}/mcp/{server}")
}

/// Bridges the store's keychain to the MCP crate's token store.
pub(super) struct KeychainStore(pub(super) Arc<dyn Keychain>);

impl sendito_mcp::oauth::TokenStore for KeychainStore {
    fn get(&self, account: &str) -> Result<Option<String>, String> {
        self.0.get(account)
    }
    fn set(&self, account: &str, value: &str) -> Result<(), String> {
        self.0.set(account, value)
    }
    fn delete(&self, account: &str) -> Result<(), String> {
        self.0.delete(account)
    }
}

/// Builds the transport for a server, rendering `{{variables}}` in its
/// command, arguments, environment, URL and headers.
pub fn mcp_transport(
    info: &McpRunInfo,
    server: &str,
    scope: &Scope,
) -> Result<sendito_mcp::Transport, String> {
    let r = |s: &str| scope.render(s).text;
    // An `http(s)://` address connects directly, without a config entry:
    // no static auth, the platform roots and the global proxy apply.
    let rendered = r(server);
    if rendered.starts_with("http://") || rendered.starts_with("https://") {
        return Ok(sendito_mcp::Transport::Http {
            url: rendered,
            headers: Vec::new(),
            verify_tls: true,
            proxy: info.proxy.clone(),
            tls: info.tls.clone(),
            auth: None,
        });
    }
    let cfg = info.servers.get(server).ok_or_else(|| {
        format!("server {server:?} is not defined in [mcp.servers] of the config")
    })?;
    Ok(match cfg {
        sendito_store::config::McpServer::Stdio(s) => sendito_mcp::Transport::Stdio {
            command: r(&s.command),
            args: s.args.iter().map(|a| r(a)).collect(),
            env: s.env.iter().map(|(k, v)| (k.clone(), r(v))).collect(),
        },
        sendito_store::config::McpServer::Http(h) => {
            let mut headers: Vec<(String, String)> =
                h.headers.iter().map(|(k, v)| (k.clone(), r(v))).collect();
            let auth = match &h.auth {
                Some(sendito_store::config::McpServerAuth::Bearer { token }) => {
                    // A configured Authorization header wins, as in HTTP requests.
                    if !headers
                        .iter()
                        .any(|(k, _)| k.eq_ignore_ascii_case("authorization"))
                    {
                        headers.push(("Authorization".into(), format!("Bearer {}", r(token))));
                    }
                    None
                }
                Some(sendito_store::config::McpServerAuth::Oauth { .. })
                    if !headers
                        .iter()
                        .any(|(k, _)| k.eq_ignore_ascii_case("authorization")) =>
                {
                    // The login binds the tokens to the rendered URL, which
                    // is the provider's default binding.
                    let p = sendito_mcp::RefreshingProvider::new(
                        Arc::new(KeychainStore(info.keychain.clone())),
                        mcp_token_account(&info.collection, server),
                        r(&h.url),
                        mcp_oauth_settings(h.auth.as_ref(), scope, info.network())?,
                    );
                    Some(Arc::new(p) as Arc<dyn sendito_mcp::TokenProvider>)
                }
                // No auth, or an explicit Authorization header that wins.
                _ => None,
            };
            sendito_mcp::Transport::Http {
                url: r(&h.url),
                headers,
                verify_tls: h.verify_tls,
                proxy: info.proxy.clone(),
                tls: info.tls.clone(),
                auth,
            }
        }
    })
}

/// Progress of an MCP call, forwarded to the UI as it happens.
#[derive(Debug, Clone)]
pub enum McpUpdate {
    Event(sendito_mcp::Event),
    Done {
        error: Option<String>,
        /// The tool answered `isError: true` (a result, not a failure).
        is_error: bool,
        /// Pretty JSON of the whole `tools/call` result, for the body viewer
        /// and for scripts (JSONPath over `$.content`, `$.structuredContent`).
        result: Option<Vec<u8>>,
        scripts: ScriptResults,
        scope_errors: Vec<String>,
        /// The server needs an OAuth sign-in (`McpError::NeedsLogin`).
        needs_login: bool,
    },
}

pub(super) fn scrub_mcp_event(ev: sendito_mcp::Event, secrets: &[String]) -> sendito_mcp::Event {
    use sendito_core::vars::scrub;
    match ev {
        sendito_mcp::Event::Sent { json, at, wall } => sendito_mcp::Event::Sent {
            json: scrub(&json, secrets),
            at,
            wall,
        },
        sendito_mcp::Event::Received { json, at, wall } => sendito_mcp::Event::Received {
            json: scrub(&json, secrets),
            at,
            wall,
        },
        sendito_mcp::Event::Log { line, at } => sendito_mcp::Event::Log {
            line: scrub(&line, secrets),
            at,
        },
        sendito_mcp::Event::Finished { ok, message, at } => sendito_mcp::Event::Finished {
            ok,
            message: scrub(&message, secrets),
            at,
        },
    }
}

/// Runs one MCP call end to end. `0` is recorded as the status on success,
/// `1` when the tool answers `isError`.
pub async fn exec_mcp(
    ctx: &RunCtx,
    req: &McpRequest,
    mut scope: Scope,
    scope_errors: Vec<String>,
    info: &McpRunInfo,
    updates: mpsc::UnboundedSender<McpUpdate>,
) {
    let mut results = ScriptResults {
        pre: script::run_pre(&req.scripts.pre, &mut scope),
        ..Default::default()
    };
    let resolved = resolve_mcp(req, &scope);
    let template = Request::Mcp(req.clone());
    let target = match mask_request(&template, &scope) {
        Request::Mcp(m) => format!("{}/{}", m.server, m.tool),
        _ => String::new(),
    };
    let secrets = scope.secret_values();
    let started = std::time::Instant::now();
    let fail = |msg: String, results: ScriptResults, needs_login: bool| {
        let msg = sendito_core::vars::scrub(&msg, &secrets);
        let entry = history_entry(
            ctx,
            &template,
            target.clone(),
            None,
            started.elapsed().as_millis() as u64,
            0,
            Some(msg.clone()),
        );
        append_history(ctx, &entry);
        let _ = updates.send(McpUpdate::Done {
            error: Some(msg),
            is_error: false,
            result: None,
            scripts: results,
            scope_errors: scope_errors.clone(),
            needs_login,
        });
    };
    if resolved.server.is_empty() || resolved.tool.is_empty() {
        return fail("choose a server and tool first".into(), results, false);
    }
    let transport = match mcp_transport(info, &resolved.server, &scope) {
        Ok(t) => t,
        Err(e) => return fail(e, results, false),
    };
    let timeout = resolved.timeout_ms.unwrap_or(info.timeout_ms);
    let opts = sendito_mcp::Options {
        timeout: Duration::from_millis(timeout.max(1)),
    };
    let (etx, mut erx) = mpsc::unbounded_channel();
    let call = sendito_mcp::call_tool(&transport, &resolved.tool, &resolved.arguments, &opts, etx);
    let forward = async {
        while let Some(ev) = erx.recv().await {
            let scr = if ctx.mask_echoed_secrets {
                scrub_mcp_event(ev, &secrets)
            } else {
                ev
            };
            let _ = updates.send(McpUpdate::Event(scr));
        }
    };
    let (outcome, _) = tokio::join!(call, forward);
    match outcome {
        Ok(r) => {
            let pretty = serde_json::to_string_pretty(&r.raw).unwrap_or_else(|_| "{}".into());
            let body = pretty.into_bytes();
            let view = ResponseView {
                status: u16::from(r.is_error),
                headers: &[],
                body: &body,
                timing: &[("total", started.elapsed())],
            };
            results.extracts = script::run_extract(&req.scripts.extract, &view, &mut scope);
            results.asserts = script::run_asserts(&req.scripts.asserts, &view, &scope);
            scrub_asserts(&mut results.asserts, &secrets);
            let size = body.len() as u64;
            let entry = history_entry(
                ctx,
                &template,
                target,
                Some(i64::from(r.is_error)),
                started.elapsed().as_millis() as u64,
                size,
                None,
            );
            write_history(ctx, &entry).await;
            let body = if ctx.mask_echoed_secrets {
                sendito_core::vars::scrub_bytes(&body, &secrets).unwrap_or(body)
            } else {
                body
            };
            let _ = updates.send(McpUpdate::Done {
                error: None,
                is_error: r.is_error,
                result: Some(body),
                scripts: results,
                scope_errors,
                needs_login: false,
            });
        }
        Err(e) => {
            let needs_login = matches!(e, sendito_mcp::McpError::NeedsLogin(_));
            fail(e.to_string(), results, needs_login)
        }
    }
}
