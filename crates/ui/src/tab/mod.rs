//! An open request tab: editor widgets for every field, conversion to and
//! from the request model, and response state.

use std::collections::VecDeque;

use sendito_core::model::*;
use sendito_pipeline::grpc::ServiceInfo;

use crate::editor::{Highlight, TextEditor};
use crate::form::{Cell, ColKind, Column, Row, Table};
use crate::viewer::{Doc, Viewer};

mod editors;
mod fields;
mod response;
pub use editors::*;
pub use fields::*;
pub use response::*;

/// Summary of the last response for the status line and response header.
#[derive(Debug, Clone, Default)]
pub struct Summary {
    /// e.g. `200 OK` or `OK (0)` for gRPC; empty while waiting.
    pub status: String,
    pub code: Option<u16>,
    pub ok: bool,
    pub time_ms: Option<f64>,
    pub size: Option<u64>,
    /// e.g. `gzip 1.2 KB`: the body arrived with this Content-Encoding and was
    /// decoded; `size` is the decoded size.
    pub decoded: Option<String>,
    pub error: Option<String>,
}

/// An in-flight request.
pub struct Running {
    pub task: tokio::task::JoinHandle<()>,
    pub control: Option<tokio::sync::mpsc::UnboundedSender<crate::runner::Control>>,
    pub started: std::time::Instant,
    /// The call/session accepts more input (client/bidi streaming, open WS).
    pub streaming_input: bool,
}

/// Watch (repeat) state of a tab: the request is sent again on this interval
/// while the tab is active and idle. Stopped by the watch key again, cancelling,
/// closing the tab or changing the environment.
#[derive(Debug, Clone)]
pub struct Watch {
    pub interval: std::time::Duration,
    /// When the next repeat is due.
    pub next: std::time::Instant,
    /// One `(ok, ms)` per send while watching, oldest first.
    pub samples: Vec<(bool, f64)>,
}

/// How many watch samples are kept for the sparkline.
pub const WATCH_SAMPLES: usize = 120;

impl Watch {
    pub fn new(interval: std::time::Duration) -> Self {
        Self {
            interval,
            next: std::time::Instant::now() + interval,
            samples: Vec::new(),
        }
    }

    pub fn record(&mut self, ok: bool, ms: f64) {
        self.samples.push((ok, ms));
        let n = self.samples.len();
        if n > WATCH_SAMPLES {
            self.samples.drain(..n - WATCH_SAMPLES);
        }
    }
}

pub struct Tab {
    pub id: u64,
    pub path: Option<String>,
    pub name: String,
    /// The request as last saved; `None` for a tab that was never saved.
    saved: Option<Request>,
    /// `saved` differs from `model`: kept up to date by `refresh_model` and
    /// `set_saved`, so drawing does not compare whole requests every frame.
    is_dirty: bool,
    pub ed: Editors,
    pub req_tab: usize,
    pub resp_tab: usize,
    pub field: usize,
    pub response: Option<Response>,
    pub running: Option<Running>,
    pub watch: Option<Watch>,
    pub services: Option<Vec<ServiceInfo>>,
    pub mcp_tools: Vec<sendito_pipeline::mcp::Tool>,
    /// The server `mcp_tools` was loaded from; the list is stale for any other.
    pub mcp_tools_server: String,
    /// Why listing `mcp_tools_server`'s tools failed; completion does not
    /// retry a dead server by itself.
    pub mcp_tools_error: Option<String>,
    pub schema_error: Option<String>,
    pub graphql_schema: Option<sendito_pipeline::http::graphql::GraphqlSchema>,
    pub graphql_source: String,
    /// Cached model built from the editors (refreshed after each change).
    pub model: Request,
    /// Where an unsaved tab came from, shown in the request header
    /// (e.g. "history entry of 12:46:51").
    pub origin: Option<String>,
    /// The saved request a history copy came from; its history entries and
    /// "save to tree" default to that folder.
    pub origin_path: Option<String>,
    /// The folder whose settings (`folder.toml`) this tab edits, `""` for
    /// the collection root; `None` for a request tab.
    pub folder: Option<String>,
    /// jq expressions applied in this tab, oldest first (kept in memory only).
    pub jq_history: Vec<String>,
    /// The tab is pinned: it stays before the tabs that are not pinned.
    pub pinned: bool,
}

/// The largest body (in bytes) that the jq filter runs on at each key press.
pub const LIVE_JQ_MAX: usize = 1024 * 1024;

/// How many jq expressions a tab remembers.
pub const JQ_HISTORY_MAX: usize = 50;

impl Tab {
    pub fn new(
        id: u64,
        path: Option<String>,
        name: String,
        req: &Request,
        saved: Option<Request>,
    ) -> Self {
        let ed = Editors::from_request(req);
        let is_dirty = saved.as_ref() != Some(req);
        Self {
            id,
            path,
            name,
            saved,
            is_dirty,
            ed,
            req_tab: 0,
            resp_tab: 0,
            field: 0,
            response: None,
            running: None,
            watch: None,
            services: None,
            mcp_tools: Vec::new(),
            mcp_tools_server: String::new(),
            mcp_tools_error: None,
            schema_error: None,
            graphql_schema: None,
            graphql_source: String::new(),
            model: req.clone(),
            origin: None,
            origin_path: None,
            folder: None,
            jq_history: Vec::new(),
            pinned: false,
        }
    }

    /// A tab that edits the settings of the folder `rel`. `saved` is what
    /// its `folder.toml` holds.
    pub fn new_folder(
        id: u64,
        rel: &str,
        name: String,
        settings: &FolderSettings,
        saved: Option<&FolderSettings>,
    ) -> Self {
        let req = folder_request(settings);
        let mut tab = Self::new(id, None, name, &req, saved.map(folder_request));
        tab.ed = Editors::from_folder(settings);
        tab.folder = Some(rel.to_string());
        tab.refresh_model();
        tab
    }

    /// The settings a folder tab holds now.
    pub fn folder_settings(&self) -> FolderSettings {
        request_folder(&self.model)
    }

    /// Adds a jq expression to the tab's history (not twice in a row).
    pub fn remember_jq(&mut self, expr: &str) {
        let expr = expr.trim();
        if expr.is_empty() || self.jq_history.last().is_some_and(|l| l == expr) {
            return;
        }
        self.jq_history.push(expr.to_string());
        if self.jq_history.len() > JQ_HISTORY_MAX {
            self.jq_history.remove(0);
        }
    }

    pub fn refresh_model(&mut self) {
        self.model = self.ed.to_request();
        self.is_dirty = self.saved.as_ref() != Some(&self.model);
    }

    pub fn saved(&self) -> Option<&Request> {
        self.saved.as_ref()
    }

    pub fn set_saved(&mut self, saved: Option<Request>) {
        self.saved = saved;
        self.is_dirty = self.saved.as_ref() != Some(&self.model);
    }

    pub fn dirty(&self) -> bool {
        self.is_dirty
    }

    /// The name shown for the tab (on the tab, in the header and in dialogs):
    /// stable while the request is edited; a new request is "untitled N".
    pub fn title(&self) -> String {
        self.name.clone()
    }

    pub fn badge(&self) -> String {
        if self.folder.is_some() {
            return "DIR".into();
        }
        self.model.badge().to_string()
    }

    pub fn fields(&self) -> Vec<Field> {
        self.ed.fields(self.req_tab)
    }

    pub fn current_field(&self) -> Option<Field> {
        self.fields().get(self.field).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::viewer::ViewMode;

    fn grpc_msg(json: &str) -> sendito_pipeline::grpc::Event {
        sendito_pipeline::grpc::Event::Received {
            json: json.into(),
            full: json.into(),
            compact: json.into(),
            size: json.len(),
            at: std::time::Duration::from_millis(5),
            wall: std::time::SystemTime::UNIX_EPOCH,
        }
    }

    #[test]
    fn stream_messages_follow_live_and_keep_the_filter() {
        let mut r = Response::new();
        for text in ["one", "two", "three"] {
            r.push_grpc(grpc_msg(&format!("{{\"text\":\"{text}\"}}")), 100);
        }
        r.push_ws(
            sendito_pipeline::ws::Event::Received {
                kind: sendito_pipeline::ws::FrameKind::Ping,
                preview: String::new(),
                size: 0,
                at: std::time::Duration::ZERO,
                wall: std::time::SystemTime::UNIX_EPOCH,
            },
            100,
        );
        assert_eq!(r.stream_messages().len(), 3);
        assert!(r.body.doc.raw.text.contains("three"));
        assert_eq!(r.body.mode, ViewMode::Pretty);
        assert_eq!(r.stream_sel, None);
        // A filter and its bar stay on the message the body shows.
        r.query = Some(QueryBar::new(QueryMode::Jq, ".text", 0));
        assert!(r.apply_jq(".text").is_ok());
        assert!(r.select_message(-1));
        assert_eq!(r.stream_sel, Some(1));
        assert_eq!(r.body.doc.raw.text.trim(), "\"two\"");
        assert!(r.query.is_some());
        // A new message does not move a paused view.
        r.push_grpc(grpc_msg("{\"text\":\"four\"}"), 100);
        assert_eq!(r.body.doc.raw.text.trim(), "\"two\"");
        // Going to the last message turns live mode on again.
        assert!(r.select_message(1));
        assert!(r.select_message(1));
        assert_eq!(r.stream_sel, None);
        assert!(!r.select_message(1));
        assert_eq!(r.body.doc.raw.text.trim(), "\"four\"");
        // A message the filter fails on shows unfiltered, the error on the bar.
        r.push_grpc(grpc_msg("[1]"), 100);
        assert_eq!(r.body.doc.raw.text, "[1]");
        assert!(r.query.as_ref().is_some_and(|q| q.error.is_some()));
        assert_eq!(r.jq.as_deref(), Some(".text"));
    }

    #[test]
    fn stream_keeps_the_newest_messages_and_counts_all() {
        let mut r = Response::new();
        for i in 0..5 {
            r.push_grpc(grpc_msg(&format!("{{\"n\":{i}}}")), 3);
        }
        assert_eq!(r.stream.received, 5);
        assert_eq!(r.stream.dropped, 2);
        assert_eq!(r.stream.kept(), 3);
        let kept: Vec<String> = r
            .stream_messages()
            .iter()
            .map(|m| m.text.to_string())
            .collect();
        assert_eq!(kept, ["{\"n\":2}", "{\"n\":3}", "{\"n\":4}"]);
        // A paused selection stays on its message when an old one is dropped.
        assert!(r.select_message(-1));
        assert_eq!(r.stream_sel, Some(1));
        r.push_grpc(grpc_msg("{\"n\":5}"), 3);
        assert_eq!(r.stream_sel, Some(0));
        assert!(r.body.doc.raw.text.contains('3'));
    }

    #[test]
    fn control_events_do_not_grow_the_stream_list() {
        let mut r = Response::new();
        let sent = || sendito_pipeline::grpc::Event::Sent {
            json: "{}".into(),
            at: std::time::Duration::ZERO,
            wall: std::time::SystemTime::UNIX_EPOCH,
        };
        let ping = || sendito_pipeline::ws::Event::Received {
            kind: sendito_pipeline::ws::FrameKind::Ping,
            preview: String::new(),
            size: 0,
            at: std::time::Duration::ZERO,
            wall: std::time::SystemTime::UNIX_EPOCH,
        };
        // `Sent` events and ping frames are not messages: the list stays empty.
        for _ in 0..1000 {
            r.push_grpc(sent(), 100);
            r.push_ws(ping(), 100);
        }
        assert!(r.grpc_events.is_empty());
        assert!(r.ws_events.is_empty());
        assert_eq!(r.stream.received, 0);
        // Headers and the close event are kept: the headers view needs them.
        r.push_grpc(
            sendito_pipeline::grpc::Event::Headers {
                metadata: vec![("a".into(), "1".into())],
                at: std::time::Duration::ZERO,
            },
            100,
        );
        assert_eq!(r.grpc_events.len(), 1);
    }

    #[test]
    fn jq_filter_state() {
        let body = br#"{"items":[{"id":1},{"id":2}],"count":2}"#;
        let mut r = Response::new();
        r.set_body(Doc::from_bytes(body.to_vec(), Some("application/json")));
        // A filter replaces the shown document and remembers the expression.
        assert!(r.apply_jq(".count").unwrap());
        assert_eq!(r.jq.as_deref(), Some(".count"));
        assert_eq!(r.body.doc.line(ViewMode::Pretty, 0), "2");
        // A new expression re-filters the original body, not the output.
        assert!(r.apply_jq(".items[0].id").unwrap());
        assert_eq!(r.body.doc.line(ViewMode::Pretty, 0), "1");
        // Clearing restores the original bytes and drops the state.
        assert!(!r.apply_jq("").unwrap());
        assert!(r.jq.is_none() && r.jq_src.is_none());
        assert_eq!(r.body.doc.bytes(), &body[..]);
        // An empty filter with none applied is a no-op.
        assert!(!r.apply_jq("").unwrap());
        // A replaced body drops a pending filter.
        assert!(r.apply_jq(".count").unwrap());
        r.set_body(Doc::text("x"));
        assert!(r.jq.is_none() && r.jq_src.is_none());
        // A bad expression changes nothing.
        assert!(r.apply_jq("nope").is_err());
        assert!(r.jq.is_none());
    }

    fn full() -> Request {
        Request::Http(HttpRequest {
            method: "POST".into(),
            url: "{{base}}/x".into(),
            params: vec![KeyValue::new("a", "1"), KeyValue::new("b", "2").disabled()],
            headers: vec![KeyValue::new("X", "y")],
            body: Body::Multipart {
                parts: vec![
                    MultipartPart {
                        name: "t".into(),
                        kind: PartKind::Text { value: "v".into() },
                        content_type: String::new(),
                        enabled: true,
                    },
                    MultipartPart {
                        name: "f".into(),
                        kind: PartKind::File {
                            path: "a.png".into(),
                            filename: Some("b.png".into()),
                        },
                        content_type: "image/png".into(),
                        enabled: false,
                    },
                ],
            },
            auth: Auth::ApiKey {
                key: "k".into(),
                value: "v".into(),
                location: ApiKeyLocation::Query,
            },
            options: HttpOptions {
                timeout_ms: Some(10),
                follow_redirects: false,
                verify_tls: false,
                cookies: None,
            },
            common: RequestCommon {
                scripts: Scripts {
                    pre: vec![SetVar {
                        var: "a".into(),
                        value: "b".into(),
                    }],
                    extract: vec![Extract {
                        var: "t".into(),
                        source: Source::Header,
                        expr: "x".into(),
                        secret: true,
                    }],
                    asserts: vec![Assertion {
                        source: Source::Status,
                        expr: String::new(),
                        op: AssertOp::Lt,
                        value: "300".into(),
                    }],
                    lua_pre: LuaScript::own("request:set_header(\"X\", \"1\")\nprint(1)")
                        .into_iter()
                        .collect(),
                    lua_post: LuaScript::own("test(\"ok\", response.status < 300);")
                        .into_iter()
                        .collect(),
                },
                description: "# Create\n\nMakes an *x*.".into(),
            },
        })
    }

    #[test]
    fn roundtrip_through_editors() {
        let bodies = [
            Body::None,
            Body::Json {
                text: "{\n  \"a\": 1\n}".into(),
            },
            Body::Raw {
                content_type: "text/xml".into(),
                text: "<a/>".into(),
            },
            Body::Form {
                fields: vec![KeyValue::new("a", "b")],
            },
            Body::File {
                path: "f.bin".into(),
                content_type: String::new(),
            },
        ];
        let mut reqs = vec![full()];
        for b in bodies {
            for auth in [
                Auth::Inherit,
                Auth::None,
                Auth::Basic {
                    username: "u".into(),
                    password: "p".into(),
                },
                Auth::Bearer { token: "t".into() },
                Auth::OAuth {
                    name: "identity".into(),
                },
                Auth::Digest {
                    username: "d".into(),
                    password: "{{dp}}".into(),
                },
                Auth::AwsSigV4 {
                    access_key: "ak".into(),
                    secret_key: "{{sk}}".into(),
                    session_token: "tok".into(),
                    region: "eu-west-1".into(),
                    service: "s3".into(),
                },
                Auth::Jwt {
                    algorithm: JwtAlgorithm::Rs256,
                    secret: String::new(),
                    private_key: "keys/rsa.pem".into(),
                    claims: "{\"sub\": \"x\"}".into(),
                    expires_in_s: Some(60),
                },
            ] {
                reqs.push(Request::Http(HttpRequest {
                    body: b.clone(),
                    auth,
                    ..Default::default()
                }));
            }
        }
        reqs.push(Request::Grpc(GrpcRequest {
            target: "h:1".into(),
            service: "s.S".into(),
            method: "M".into(),
            message: "{}".into(),
            metadata: vec![KeyValue::new("k", "v")],
            tls: true,
            verify_tls: false,
            deadline_ms: Some(5),
            schema: SchemaSource::Files {
                files: vec!["a.proto".into(), "b.proto".into()],
                import_paths: vec!["p".into()],
            },
            ..Default::default()
        }));
        reqs.push(Request::new_grpc());
        reqs.push(Request::Ws(WebSocketRequest {
            url: "wss://{{h}}/chat".into(),
            headers: vec![KeyValue::new("origin", "https://x")],
            subprotocols: vec!["chat.v2".into()],
            messages: "[\"a\", {\"binary\": \"AAEC\"}]".into(),
            auth: Auth::Bearer {
                token: "{{t}}".into(),
            },
            options: WsOptions {
                timeout_ms: Some(5),
                deadline_ms: Some(50),
                verify_tls: false,
                tls: true,
                cookies: Some(false),
            },
            ..Default::default()
        }));
        reqs.push(Request::new_ws());
        for r in reqs {
            let e = Editors::from_request(&r);
            assert_eq!(e.to_request(), r);
        }
        // A folder's settings, variables included.
        let folder = FolderSettings {
            headers: vec![KeyValue::new("X-A", "1")],
            auth: Auth::Jwt {
                algorithm: JwtAlgorithm::Hs256,
                secret: "{{s}}".into(),
                private_key: String::new(),
                claims: "{}".into(),
                expires_in_s: None,
            },
            vars: vec![KeyValue::new("tenant", "acme").disabled()],
            description: "Tenant calls".into(),
            ..Default::default()
        };
        let e = Editors::from_folder(&folder);
        assert_eq!(e.kind, Kind::Folder);
        assert_eq!(request_folder(&e.to_request()), folder);
        assert_eq!(e.fields(2), vec![Field::Vars]);
        // Docs is the last tab; a written description opens rendered.
        assert_eq!(e.fields(4), vec![Field::DocsView, Field::DocsPreview]);
        let mut blank = Editors::from_request(&Request::new_mcp());
        assert_eq!(blank.fields(4), vec![Field::DocsView, Field::Description]);
        assert!(blank.cycle(Field::DocsView, 1));
        assert_eq!(blank.fields(4), vec![Field::DocsView, Field::DocsPreview]);
        assert_eq!(blank.fields(3), vec![Field::Timeout]);
        assert_eq!(
            e.fields(1),
            vec![
                Field::AuthKind,
                Field::JwtAlgorithm,
                Field::JwtSecret,
                Field::JwtExpires,
                Field::JwtClaims
            ]
        );
    }

    #[test]
    fn http_only_auth_kinds_are_not_offered_to_grpc() {
        let e = Editors::from_request(&Request::new_grpc());
        let options = e.choice_options(Field::AuthKind);
        assert!(options.iter().any(|o| o.starts_with("JWT")));
        assert!(!options.iter().any(|o| o == "Digest" || o == "OAuth 2"));
    }

    #[test]
    fn fields_and_choices() {
        let mut e = Editors::from_request(&full());
        assert_eq!(e.fields(2), vec![Field::BodyKind, Field::Parts]);
        assert_eq!(
            e.fields(3),
            vec![
                Field::AuthKind,
                Field::AuthKey,
                Field::AuthValue,
                Field::AuthIn
            ]
        );
        assert!(e.cycle(Field::BodyKind, 1));
        assert_eq!(e.body_kind, BodyKind::File);
        e.set_choice(Field::BodyKind, 1);
        assert_eq!(e.body_kind, BodyKind::Json);
        assert_eq!(e.choice_label(Field::BodyKind), "JSON");
        assert_eq!(e.choice_options(Field::AuthKind).len(), 9);
        assert!(e.cycle(Field::FollowRedirects, 1));
        assert_eq!(e.choice_label(Field::FollowRedirects), "On");
        assert!(!e.cycle(Field::Params, 1));
        e.set_choice(Field::AuthIn, 0);
        let auto = e.auto_headers();
        assert!(auto.iter().any(|(n, _)| n == "Content-Type"));
        assert!(auto.iter().any(|(n, _)| n == "k"));
        e.set_vim(true);
        assert!(e.text_mut(Field::BodyText).is_some());
        assert!(e.table_mut(Field::Params).is_some());
        assert!(e.text_mut(Field::Params).is_none());
        for f in [Field::Timeout, Field::BodyKind, Field::Tls, Field::Cookies] {
            assert!(!f.label().is_empty());
            assert!(!f.help(Kind::Http).is_empty());
        }
        // Tables and editors fill their area: no label, no help line.
        assert!(Field::Params.label().is_empty());
        assert!(Field::BodyText.help(Kind::Http).is_empty());
        // The cookie jar option cycles: default → on → off → default.
        assert_eq!(e.choice_label(Field::Cookies), "Config default");
        assert!(e.cycle(Field::Cookies, 1));
        assert_eq!(e.choice_label(Field::Cookies), "On");
        assert!(e.cycle(Field::Cookies, 1));
        assert_eq!(e.choice_label(Field::Cookies), "Off");
        assert!(e.cycle(Field::Cookies, 1));
        assert_eq!(e.choice_label(Field::Cookies), "Config default");
        e.cookies = Some(true);
        let Request::Http(h) = e.to_request() else {
            panic!("not http")
        };
        assert_eq!(h.options.cookies, Some(true));
        let mut tab = Tab::new(1, None, "x".into(), &full(), Some(full()));
        tab.refresh_model();
        assert!(!tab.dirty());
        tab.ed.url.set_text("changed");
        tab.refresh_model();
        assert!(tab.dirty());
        assert_eq!(tab.badge(), "POST");
        assert_eq!(tab.current_field(), Some(Field::Params));
        let mut r = Response::new();
        r.viewer_mut(3);
    }
}
