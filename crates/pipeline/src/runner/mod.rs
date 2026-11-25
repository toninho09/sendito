//! The send pipeline shared by the TUI and `sendito run`:
//! scope → pre-request scripts → resolve → send → extract/assert → post
//! scripts → history.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sendito_core::inherit::{self, ChainItem};
use sendito_core::lua_script;
use sendito_core::model::*;
use sendito_core::resolve::{
    ResolvedGrpc, ResolvedHttp, SignedAuth, mask_request, resolve_grpc, resolve_http, resolve_mcp,
};
use sendito_core::script::{self, AssertResult, ExtractResult, RUNTIME_LAYER, ResponseView};
use sendito_core::vars::{Scope, VarValue};
use sendito_grpc::{Event, Schema};
use sendito_http::{HttpResponse, SendOptions};
use sendito_store::Collection;
use sendito_store::config::Config;
use sendito_store::env::{COLLECTION_ENV, Envs, Keychain, MemoryKeychain, OsKeychain, VarSource};
use sendito_store::history::{self, HistoryEntry};
use tokio::sync::mpsc;

use crate::doc::Doc;

#[derive(Debug, Clone, Default)]
pub struct ScriptResults {
    /// Variables the `pre` rows and the Lua pre scripts set, in order.
    pub pre: Vec<(String, VarValue)>,
    pub extracts: Vec<ExtractResult>,
    /// The assertions, then the `test(...)` results of the Lua post scripts.
    pub asserts: Vec<AssertResult>,
    /// Variables the Lua post scripts set, in order.
    pub post: Vec<(String, VarValue)>,
    /// `print` lines of the Lua scripts, secrets masked.
    pub logs: Vec<String>,
}

impl ScriptResults {
    pub fn failed_asserts(&self) -> usize {
        self.asserts.iter().filter(|a| !a.pass).count()
    }

    /// Runtime variable updates in the order they were made: pre-request
    /// sets, successful extractions, then post-script sets.
    pub fn updates(&self) -> Vec<(String, VarValue)> {
        let mut v = self.pre.clone();
        for e in &self.extracts {
            if let Ok(val) = &e.value {
                v.push((e.var.clone(), val.clone()));
            }
        }
        v.extend(self.post.iter().cloned());
        v
    }
}

/// A request kind the pre phase can wrap in a `Request` and take back.
pub(crate) trait Kind: Clone {
    fn wrap(self) -> Request;
    fn take(r: Request) -> Option<Self>;
}

macro_rules! kind {
    ($($t:ty => $v:ident),*) => {$(
        impl Kind for $t {
            fn wrap(self) -> Request {
                Request::$v(self)
            }
            fn take(r: Request) -> Option<Self> {
                match r {
                    Request::$v(x) => Some(x),
                    _ => None,
                }
            }
        }
    )*};
}

kind!(HttpRequest => Http, GrpcRequest => Grpc, WebSocketRequest => Ws,
      SseRequest => Sse, MqttRequest => Mqtt, McpRequest => Mcp);

/// The result of the pre-request phase.
pub(crate) struct PrePhase<T> {
    /// The request to send: the one given, as the Lua pre scripts changed it.
    pub req: T,
    pub results: ScriptResults,
    /// A Lua pre script failed: the send must stop with this message.
    pub error: Option<String>,
}

/// The pre-request phase of a send: the `pre` rows, then the Lua pre
/// scripts. Both set variables in `scope`; the scripts can also change the
/// request. On a script error the request is the one given.
pub(crate) fn pre_phase<T: Kind>(req: &T, scope: &mut Scope) -> PrePhase<T> {
    let mut wrapped = req.clone().wrap();
    let mut results = ScriptResults {
        pre: script::run_pre(&wrapped.scripts().pre, scope),
        ..Default::default()
    };
    let scripts = wrapped.scripts().lua_pre.clone();
    let error = match lua_script::run_lua_pre(&scripts, &mut wrapped, scope) {
        Ok(out) => {
            results.pre.extend(out.sets);
            results.logs = out.logs;
            None
        }
        Err(e) => Some(e),
    };
    PrePhase {
        req: T::take(wrapped).unwrap_or_else(|| req.clone()),
        results,
        error,
    }
}

/// The response phase of a send: the extracts, the assertions, then the Lua
/// post scripts. Their tests join the assertions, their variables and logs
/// join `results`. An extracted value that contains one of `taint` stays a
/// secret. The caller masks the assertions after this phase.
pub(crate) fn response_phase(
    scripts: &Scripts,
    view: &ResponseView<'_>,
    scope: &mut Scope,
    results: &mut ScriptResults,
    taint: &[String],
) {
    results.extracts = script::run_extract(&scripts.extract, view, scope);
    // Cookie values and OAuth access tokens remain secret even if an
    // extraction rule or a script did not mark them; runtime variables may
    // persist.
    for extract in &mut results.extracts {
        if let Ok(value) = &mut extract.value {
            taint_set(scope, &extract.var, value, taint);
        }
    }
    results.asserts = script::run_asserts(&scripts.asserts, view, scope);
    let mut out = lua_script::run_lua_post(&scripts.lua_post, view, scope);
    for (name, value) in &mut out.sets {
        taint_set(scope, name, value, taint);
    }
    results.asserts.extend(out.tests);
    results.post.extend(out.sets);
    let taint: Vec<String> = taint.iter().filter(|s| !s.is_empty()).cloned().collect();
    results.logs.extend(
        out.logs
            .iter()
            .map(|l| sendito_core::vars::scrub(l, &taint)),
    );
}

/// Marks `value` secret when it contains one of `taint`. The runtime layer
/// gets the change while it still holds this value (a later set wins).
fn taint_set(scope: &mut Scope, name: &str, value: &mut VarValue, taint: &[String]) {
    if value.secret
        || !taint
            .iter()
            .any(|s| !s.is_empty() && value.value.contains(s.as_str()))
    {
        return;
    }
    value.secret = true;
    if scope
        .raw(name)
        .is_some_and(|(current, _)| current.value == value.value)
    {
        scope.set(RUNTIME_LAYER, name, value.clone());
    }
}

#[derive(Clone)]
pub struct RunCtx {
    pub root: PathBuf,
    pub send: SendOptions,
    pub history: Option<(PathBuf, usize)>,
    pub name: String,
    pub path: Option<String>,
    pub env: Option<String>,
    pub schemas: SchemaCache,
    pub grpc_deadline_ms: u64,
    /// `[websocket] timeout_ms`: the default handshake timeout (0 = none).
    pub ws_timeout_ms: u64,
    pub sse_timeout_ms: u64,
    /// `[mqtt] timeout_ms`: the default time to wait for CONNACK (0 = none).
    pub mqtt_timeout_ms: u64,
    /// Mask secret values the server echoes back (the TUI); `sendito run`
    /// writes the body unchanged.
    pub mask_echoed_secrets: bool,
    /// `[http] cookie_jar` config default; the request's `[options] cookies`
    /// overrides it.
    pub cookie_jar: bool,
    pub oauth: Option<OAuthRunInfo>,
}

/// Commands to a running request: gRPC calls and WebSocket sessions share
/// the channel type so the UI can hold one sender per tab.
#[derive(Debug, Clone)]
pub enum Control {
    Grpc(GrpcControl),
    Ws(WsControl),
    Sse(SseControl),
    Mqtt(MqttControl),
}

// ----- WebSocket -----------------------------------------------------

// ----- MCP -----------------------------------------------------------

/// Re-export so the UI can name the event type without depending on the crate.
pub use sendito_mcp::Event as McpEvent;

/// The scope layer that holds values signed at send time (a JWT), so they
/// are masked like other secrets.
pub(super) const SIGNED_LAYER: &str = "signed";

mod graphql;
mod grpc;
mod http;
mod mcp;
mod mqtt;
mod oauth;
mod project;
mod record;
mod scope;
mod sse;
mod ws;
pub use graphql::*;
pub use grpc::*;
pub use http::*;
pub use mcp::*;
pub use mqtt::*;
pub use oauth::*;
pub use project::*;
use record::*;
pub use scope::*;
pub use sse::*;
pub use ws::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_failed_secret_is_unresolved_even_with_a_lower_value() {
        let req = Request::Http(HttpRequest {
            url: "https://h/{{api_key}}/{{base}}".into(),
            ..Default::default()
        });
        let mut low = BTreeMap::new();
        low.insert("api_key".to_string(), VarValue::plain("dev-placeholder"));
        low.insert("base".to_string(), VarValue::plain("x"));
        let scope = Scope::new().with_layer(COLLECTION_ENV, low);
        assert!(unresolved_loaded(&req, &scope, &[]).is_empty());
        assert_eq!(
            unresolved_loaded(&req, &scope, &["api_key".into(), "unused".into()]),
            vec!["api_key".to_string()]
        );
        // Also through another value.
        let req = Request::Http(HttpRequest {
            url: "https://h/{{auth}}".into(),
            ..Default::default()
        });
        let mut low = BTreeMap::new();
        low.insert("api_key".to_string(), VarValue::plain("dev"));
        low.insert("auth".to_string(), VarValue::plain("Bearer {{api_key}}"));
        let scope = Scope::new().with_layer(COLLECTION_ENV, low);
        assert!(!unresolved_loaded(&req, &scope, &["api_key".into()]).is_empty());
        // And through an MCP server's configuration.
        let mut servers = BTreeMap::new();
        servers.insert(
            "remote".to_string(),
            sendito_store::config::McpServer::Http(sendito_store::config::McpHttpServer {
                url: "https://m/mcp".into(),
                headers: BTreeMap::new(),
                verify_tls: true,
                auth: Some(sendito_store::config::McpServerAuth::Bearer {
                    token: "{{auth}}".into(),
                }),
            }),
        );
        let req = Request::Mcp(McpRequest {
            server: "remote".into(),
            tool: "t".into(),
            ..Default::default()
        });
        assert_eq!(
            unresolved_send(&req, &scope, &["api_key".into()], &servers),
            vec!["api_key".to_string()]
        );
    }

    fn secret_scope() -> Scope {
        let mut env = BTreeMap::new();
        env.insert("key".to_string(), VarValue::secret("s3cr3t-key"));
        let mut s = Scope::new().with_layer("env", env);
        s.push_layer(RUNTIME_LAYER, BTreeMap::new());
        s
    }

    #[tokio::test]
    async fn a_failed_pre_script_masks_the_request_shown() {
        let req = HttpRequest {
            url: "http://127.0.0.1:9/x?k={{key}}".into(),
            headers: vec![KeyValue::new("X-Trace", "{{key}}")],
            common: RequestCommon {
                scripts: Scripts {
                    lua_pre: LuaScript::own("error('stop')").into_iter().collect(),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let ctx = RunCtx {
            root: std::env::temp_dir(),
            send: SendOptions::default(),
            history: None,
            name: String::new(),
            path: None,
            env: None,
            schemas: SchemaCache::default(),
            grpc_deadline_ms: 0,
            ws_timeout_ms: 0,
            sse_timeout_ms: 0,
            mqtt_timeout_ms: 0,
            mask_echoed_secrets: true,
            cookie_jar: false,
            oauth: None,
        };
        let o = exec_http(&ctx, &req, secret_scope(), vec![]).await;
        assert!(o.response.is_err());
        assert!(!o.resolved.url.contains("s3cr3t-key"), "{}", o.resolved.url);
        assert!(
            o.resolved
                .headers
                .iter()
                .all(|(_, v)| !v.contains("s3cr3t-key")),
            "{:?}",
            o.resolved.headers
        );
    }

    #[test]
    fn a_post_script_set_of_a_cookie_value_stays_secret() {
        let scripts = Scripts {
            lua_post: LuaScript::own(
                "vars.set('sid', response:header('x-sid'))\nprint('sid ' .. vars.get('sid'))",
            )
            .into_iter()
            .collect(),
            ..Default::default()
        };
        let headers = vec![("X-Sid".to_string(), "cookie-value-1".to_string())];
        let view = ResponseView {
            status: 200,
            headers: &headers,
            body: b"",
            timing: &[],
        };
        let mut scope = secret_scope();
        let mut results = ScriptResults::default();
        response_phase(
            &scripts,
            &view,
            &mut scope,
            &mut results,
            &["cookie-value-1".to_string()],
        );
        assert_eq!(results.post.len(), 1);
        assert!(results.post[0].1.secret);
        assert!(scope.raw("sid").is_some_and(|(v, _)| v.secret));
        assert!(
            !results.logs[0].contains("cookie-value-1"),
            "{:?}",
            results.logs
        );
    }

    #[test]
    fn ws_urls_map_to_their_http_equivalent_for_the_jar() {
        // The cookie jar only accepts http/https; ws/wss keep the same origin.
        assert_eq!(http_equivalent("ws://h:1/a?b=c"), "http://h:1/a?b=c");
        assert_eq!(http_equivalent("wss://h/a"), "https://h/a");
        assert_eq!(
            http_equivalent("https://h/a"),
            "https://h/a",
            "an http URL is left as is"
        );
        assert_eq!(http_equivalent("h/a"), "h/a");
    }

    #[test]
    fn script_results() {
        let r = ScriptResults {
            pre: vec![("a".into(), VarValue::plain("1"))],
            extracts: vec![
                ExtractResult {
                    var: "b".into(),
                    value: Ok(VarValue::plain("2")),
                },
                ExtractResult {
                    var: "c".into(),
                    value: Err("x".into()),
                },
            ],
            asserts: vec![AssertResult {
                label: "x".into(),
                pass: false,
                actual: "y".into(),
            }],
            post: vec![("d".into(), VarValue::plain("4"))],
            logs: vec![],
        };
        let order: Vec<_> = r.updates().into_iter().map(|(n, _)| n).collect();
        assert_eq!(order, ["a", "b", "d"]);
        assert_eq!(r.failed_asserts(), 1);
        assert_eq!(schema_key("u", &SchemaSource::Reflection), "reflect:u");
        let mut summary = GrpcSummary::new();
        for e in [
            Event::Headers {
                metadata: vec![("a".into(), "1".into())],
                at: Duration::ZERO,
            },
            Event::Received {
                json: "{}".into(),
                full: "{}".into(),
                compact: "{}".into(),
                size: 2,
                at: Duration::ZERO,
                wall: std::time::SystemTime::now(),
            },
            Event::Finished {
                code: 0,
                message: String::new(),
                metadata: vec![],
                at: Duration::from_millis(7),
            },
        ] {
            summary.add(&e);
        }
        assert_eq!(summary.body, b"{}");
        assert_eq!(summary.headers.len(), 1);
        assert_eq!(summary.code, 0);
        assert_eq!(summary.size, 2);
        assert_eq!(summary.total_ms, 7);
    }
}
