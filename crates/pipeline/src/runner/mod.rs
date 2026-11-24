//! The send pipeline shared by the TUI and `sendito run`:
//! scope → pre-request script → resolve → send → extract/assert → history.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use sendito_core::model::*;
use sendito_core::resolve::{
    ResolvedGrpc, ResolvedHttp, mask_request, resolve_grpc, resolve_http, resolve_mcp,
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
    pub pre: Vec<(String, VarValue)>,
    pub extracts: Vec<ExtractResult>,
    pub asserts: Vec<AssertResult>,
}

impl ScriptResults {
    pub fn failed_asserts(&self) -> usize {
        self.asserts.iter().filter(|a| !a.pass).count()
    }

    /// Runtime variable updates (pre-request sets and successful extractions).
    pub fn updates(&self) -> Vec<(String, VarValue)> {
        let mut v = self.pre.clone();
        for e in &self.extracts {
            if let Ok(val) = &e.value {
                v.push((e.var.clone(), val.clone()));
            }
        }
        v
    }
}

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
}

// ----- WebSocket -----------------------------------------------------

// ----- MCP -----------------------------------------------------------

/// Re-export so the UI can name the event type without depending on the crate.
pub use sendito_mcp::Event as McpEvent;

mod grpc;
mod http;
mod mcp;
mod oauth;
mod project;
mod record;
mod scope;
mod ws;
pub use grpc::*;
pub use http::*;
pub use mcp::*;
pub use oauth::*;
pub use project::*;
use record::*;
pub use scope::*;
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
        };
        assert_eq!(r.updates().len(), 2);
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
