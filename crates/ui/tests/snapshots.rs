//! Snapshot tests of every pane and overlay.
//! Responses are fabricated so the output is deterministic.
//!
//! A snapshot keeps only the part of the screen that its test is about (see
//! [`Area`]), so a change to one pane does not change the snapshots of the
//! others. The layout tests keep the full screen at 120×40 and 200×60; the
//! other tests use 120×40 only.

mod common;

use std::time::Duration;

use common::{Harness, Must};
use ratatui::layout::Rect;
use sendito_core::resolve::{ResolvedBody, ResolvedHttp};
use sendito_pipeline::http::{HttpResponse, Timing};
use sendito_ui::app::{AppMsg, Focus, Hit, OverlayHit};
use sendito_ui::runner::{HttpOutcome, ScriptResults};
use sendito_ui::viewer::Doc;

/// The sizes of the layout tests.
const SIZES: [(u16, u16); 2] = [(120, 40), (200, 60)];
/// The size of the tests of one pane or dialog.
const SMALL: [(u16, u16); 1] = [(120, 40)];

/// The part of the screen that a snapshot keeps.
#[derive(Debug, Clone, Copy)]
enum Area {
    Full,
    Sidebar,
    /// The request and the response panes.
    Main,
    Request,
    Response,
    /// The topmost dialog.
    Dialog,
    /// The rows under the panes.
    Footer,
}

fn area_rect(h: &Harness, area: Area) -> Rect {
    let full = h.term.backend().buffer().area;
    let a = &h.app.areas;
    match area {
        Area::Full => full,
        Area::Sidebar => a.sidebar,
        Area::Main => a.main,
        Area::Request => a.request,
        Area::Response => a.response,
        Area::Dialog => h
            .app
            .hits
            .iter()
            .rev()
            .find(|(_, hit)| *hit == Hit::Overlay(OverlayHit::Dialog))
            .map(|(r, _)| *r)
            .must(),
        Area::Footer => {
            let top = a.main.bottom().max(a.sidebar.bottom());
            Rect::new(0, top, full.width, full.height.saturating_sub(top))
        }
    }
}

/// Draws the screen and compares `area` of it with the snapshot `name`. The
/// file name ends with the screen size.
fn snap_in(area: Area, name: &str, h: &mut Harness) {
    h.draw();
    let full = h.term.backend().buffer().area;
    let r = area_rect(h, area);
    let buf = h.term.backend().buffer();
    let mut screen = String::new();
    for y in r.top()..r.bottom() {
        let mut line = String::new();
        for x in r.left()..r.right() {
            line.push_str(buf[(x, y)].symbol());
        }
        screen.push_str(line.trim_end());
        screen.push('\n');
    }
    let tmp = h.dir.path().display().to_string();
    let screen = screen.replace(&tmp, "<tmp>");
    insta::with_settings!({ snapshot_path => "snapshots", prepend_module_to_snapshot => false }, {
        insta::assert_snapshot!(format!("{name}_{}x{}", full.width, full.height), screen);
    });
}

/// A snapshot of the full screen.
fn snap(name: &str, h: &mut Harness) {
    snap_in(Area::Full, name, h);
}

fn fake_response(h: &mut Harness, body: &str, status: u16) {
    let id = h.app.tabs[h.app.active].id;
    let headers = vec![
        ("content-type".to_string(), "application/json".to_string()),
        ("x-request-id".to_string(), "req-1".to_string()),
    ];
    let resp = HttpResponse {
        status,
        reason: if status == 200 {
            "OK".into()
        } else {
            "Not Found".into()
        },
        version: "HTTP/1.1".into(),
        url: "http://127.0.0.1:9/users?page=1".into(),
        remote_addr: None,
        headers,
        body: Vec::new(),
        truncated: false,
        decoded: None,
        timing: Timing {
            dns: Some(Duration::from_micros(1200)),
            connect: Some(Duration::from_micros(800)),
            tls: None,
            ttfb: Duration::from_millis(12),
            download: Duration::from_micros(300),
            total: Duration::from_micros(14300),
        },
    };
    let resolved = ResolvedHttp {
        method: "GET".into(),
        url: "http://127.0.0.1:9/users?page=1".into(),
        headers: vec![
            ("Accept".into(), "application/json".into()),
            ("Authorization".into(), "Bearer s3cr3t-token".into()),
        ],
        header_origins: vec![],
        body: ResolvedBody::None,
        timeout_ms: None,
        follow_redirects: true,
        verify_tls: true,
        unresolved: vec![],
        notes: vec![],
        signed: None,
    };
    let scripts = ScriptResults {
        pre: vec![],
        extracts: vec![sendito_core::script::ExtractResult {
            var: "token".into(),
            value: Ok(sendito_core::VarValue::secret("abc")),
        }],
        asserts: vec![
            sendito_core::script::AssertResult {
                label: "status == 200".into(),
                pass: status == 200,
                actual: status.to_string(),
            },
            sendito_core::script::AssertResult {
                label: "$.count > 1".into(),
                pass: true,
                actual: "2".into(),
            },
        ],
        ..Default::default()
    };
    let outcome = HttpOutcome {
        resolved,
        response: Ok(resp),
        doc: Doc::from_bytes(body.as_bytes().to_vec(), Some("application/json")),
        body_size: body.len() as u64,
        scripts,
        scope_errors: vec![],
    };
    h.app.tabs[h.app.active].running = None;
    h.app.on_msg(AppMsg::Http {
        tab: id,
        outcome: Box::new(outcome),
    });
    h.draw();
}

const BODY: &str = r#"{"items":[{"id":1,"name":"Ana","email":"ana@example.com","roles":["admin","dev"],"active":true},{"id":2,"name":"Bia","email":"bia@example.com","roles":[],"active":false,"manager":null}],"count":2,"next":"/users?page=2"}"#;

fn open(h: &mut Harness, name: &str) {
    h.key("ctrl+w h");
    h.app.tree.set_filter(name);
    h.key("enter");
    h.app.tree.set_filter("");
    h.app.focus = Focus::Bar(1);
    h.draw();
}

/// Runs `f` at the sizes of the layout tests.
fn each(f: impl Fn(&mut Harness)) {
    each_at(&SIZES, f);
}

/// Runs `f` at the size of the tests of one pane or dialog.
fn each_small(f: impl Fn(&mut Harness)) {
    each_at(&SMALL, f);
}

fn each_at(sizes: &[(u16, u16)], f: impl Fn(&mut Harness)) {
    for &(w, ht) in sizes {
        let mut h = Harness::new(w, ht);
        h.app.set_env(Some("local".into()));
        h.app.status = None;
        f(&mut h);
    }
}

#[test]
fn welcome_and_tree() {
    each(|h| {
        snap("welcome", h);
        h.key("ctrl+w h down enter");
        snap_in(Area::Sidebar, "tree_expanded", h);
        h.key("/");
        h.type_text("user");
        snap_in(Area::Sidebar, "tree_filter", h);
    });
}

/// Favorites in the sidebar and on the start screen, and a pinned tab.
#[test]
fn favorites_and_pinned_tab() {
    each_small(|h| {
        h.app.tree.toggle_favorite("health.toml");
        h.app.tree.toggle_favorite("Users/list users.toml");
        snap("welcome_favorites", h);
        open(h, "health");
        open(h, "get order");
        h.key("ctrl+w P");
        h.app.status = None;
        h.key("ctrl+w h");
        snap("tree_favorites_pinned_tab", h);
    });
}

#[test]
fn request_tabs_http() {
    each_small(|h| {
        open(h, "list users");
        snap_in(Area::Request, "req_params", h);
        h.key("alt+2");
        snap_in(Area::Request, "req_headers", h);
        h.key("alt+4");
        snap_in(Area::Request, "req_auth_bearer", h);
        h.key("alt+6");
        snap_in(Area::Request, "req_options", h);
        open(h, "create user");
        h.key("alt+3");
        snap_in(Area::Request, "req_body_json", h);
        h.key("alt+5");
        snap_in(Area::Request, "req_scripts", h);
    });
}

#[test]
fn request_docs_preview() {
    each_small(|h| {
        open(h, "create user");
        let t = h.app.tabs.last_mut().must();
        t.ed.description.set_text(
            "# Create user\n\nAdds a **user** to the `users` table.\n\n- name is required\n- email must be unique\n\n> Needs the admin role.\n\n```json\n{\"name\": \"ana\"}\n```\n\nSee [the guide](https://example.com/guide).",
        );
        t.ed.docs_preview = true;
        t.req_tab = 6;
        t.refresh_model();
        snap_in(Area::Request, "req_docs_preview", h);
    });
}

#[test]
fn request_body_kinds_and_auth() {
    each_small(|h| {
        open(h, "delete user");
        let t = h.app.tabs.last_mut().must();
        t.ed.set_body_kind(sendito_core::BodyKind::Form);
        t.ed.form.push_texts(true, &["username", "ana"]);
        t.ed.form.push_texts(false, &["debug", "1"]);
        t.req_tab = 2;
        t.refresh_model();
        snap_in(Area::Request, "req_body_form", h);
        let t = h.app.tabs.last_mut().must();
        t.ed.set_body_kind(sendito_core::BodyKind::Multipart);
        t.ed.parts.push(
            true,
            vec![
                sendito_ui::form::Cell::Text(sendito_ui::editor::TextEditor::single("avatar")),
                sendito_ui::form::Cell::Choice(1),
                sendito_ui::form::Cell::Text(sendito_ui::editor::TextEditor::single(
                    "files/me.png",
                )),
                sendito_ui::form::Cell::Text(sendito_ui::editor::TextEditor::single("")),
                sendito_ui::form::Cell::Text(sendito_ui::editor::TextEditor::single("image/png")),
            ],
        );
        t.refresh_model();
        snap_in(Area::Request, "req_body_multipart", h);
        let t = h.app.tabs.last_mut().must();
        t.ed.auth_kind = sendito_core::AuthKind::ApiKey;
        t.req_tab = 3;
        t.refresh_model();
        snap_in(Area::Request, "req_auth_apikey", h);
        let t = h.app.tabs.last_mut().must();
        t.ed.auth_kind = sendito_core::AuthKind::Basic;
        t.refresh_model();
        snap_in(Area::Request, "req_auth_basic", h);
    });
}

/// Three messages of a server stream, as the runner reports them.
fn fake_stream(h: &mut Harness) {
    let id = h.app.tabs[h.app.active].id;
    for (i, text) in ["one", "two", "three"].iter().enumerate() {
        let json = format!("{{\"text\":\"{text}\",\"n\":{i}}}");
        h.app.on_msg(AppMsg::Grpc {
            tab: id,
            update: sendito_ui::runner::GrpcUpdate::Event(
                sendito_pipeline::grpc::Event::Received {
                    full: json.clone(),
                    compact: json.clone(),
                    size: json.len(),
                    json,
                    at: Duration::from_millis(100 * (i as u64 + 1)),
                    wall: std::time::SystemTime::UNIX_EPOCH,
                },
            ),
        });
    }
}

#[test]
fn request_body_graphql() {
    each_small(|h| {
        open(h, "create user");
        let t = h.app.tab_mut().must();
        t.ed.set_body_kind(sendito_core::BodyKind::GraphQL);
        t.ed.body_text
            .set_text("query Lookup($id: ID!) {\n  hero(id: $id) {\n    name\n  }\n}");
        t.ed.graphql_variables.set_text("{\"id\": \"42\"}");
        t.ed.graphql_operation.set_text("Lookup");
        t.req_tab = 2;
        t.field = 2;
        t.refresh_model();
        h.app.focus = Focus::ReqBody;
        snap_in(Area::Request, "req_body_graphql", h);
        let t = h.app.tab_mut().must();
        t.ed.set_choice(sendito_ui::tab::Field::GraphqlSection, 1);
        snap_in(Area::Request, "req_body_graphql_variables", h);
    });
}

#[test]
fn graphql_completion() {
    each_small(|h| {
        open(h, "create user");
        let env = h.app.active_env.clone();
        let t = h.app.tab_mut().must();
        t.ed.method = "POST".into();
        t.ed.url.set_text("https://example.test/graphql");
        t.ed.set_body_kind(sendito_core::BodyKind::GraphQL);
        t.ed.body_text.set_text("query Lookup {\n  hero(");
        t.ed.body_text.move_to_end();
        t.req_tab = 2;
        t.field = 2;
        t.graphql_schema = Some(
            sendito_pipeline::http::graphql::GraphqlSchema::from_json(include_bytes!(
                "fixtures/graphql-schema.json"
            ))
            .must(),
        );
        t.graphql_source = format!("{env:?}:graphql:POST https://example.test/graphql");
        t.refresh_model();
        h.app.focus = Focus::ReqBody;
        h.key("ctrl+space");
        assert_eq!(h.app.complete.as_ref().must().items.len(), 2);
        snap_in(Area::Main, "graphql_completion", h);
    });
}

#[test]
fn sse_session() {
    each_small(|h| {
        common::sse::demo(h);
        snap_in(Area::Request, "sse_options", h);
        h.key("ctrl+w j");
        snap_in(Area::Response, "sse_events", h);
        h.key("alt+2");
        snap_in(Area::Response, "sse_headers", h);
        h.key("alt+3");
        snap_in(Area::Response, "sse_timing", h);
        h.key("alt+4");
        snap_in(Area::Response, "sse_log", h);
    });
}

#[test]
fn mqtt_session() {
    each_small(|h| {
        common::mqtt::demo(h);
        snap_in(Area::Request, "mqtt_topics", h);
        h.key("ctrl+w j");
        snap_in(Area::Response, "mqtt_messages", h);
        h.key("alt+2");
        snap_in(Area::Response, "mqtt_connection", h);
        h.key("alt+3");
        snap_in(Area::Response, "mqtt_timing", h);
        h.key("alt+4");
        snap_in(Area::Response, "mqtt_log", h);
        // The one-line fields of the Will and Options tabs have labels.
        h.app.tab_mut().must().req_tab = 2;
        snap_in(Area::Request, "mqtt_will", h);
        h.app.tab_mut().must().req_tab = 5;
        snap_in(Area::Request, "mqtt_options", h);
    });
}

#[test]
fn response_stream() {
    each_small(|h| {
        open(h, "echo unary");
        fake_stream(h);
        h.key("ctrl+w j");
        snap_in(Area::Response, "resp_stream_list", h);
        h.key("[");
        snap_in(Area::Response, "resp_stream_paused", h);
    });
    // A pane too narrow for the list shows the position in its border.
    let mut h = Harness::new(96, 30);
    h.app.set_env(Some("local".into()));
    h.app.status = None;
    open(&mut h, "echo unary");
    fake_stream(&mut h);
    h.key("ctrl+w j [");
    snap_in(Area::Response, "resp_stream_narrow", &mut h);
}

#[test]
fn grpc_request() {
    each_small(|h| {
        open(h, "echo unary");
        h.app.focus = Focus::Bar(1);
        snap_in(Area::Main, "grpc_message", h);
        h.key("alt+5");
        snap_in(Area::Main, "grpc_options", h);
    });
}

#[test]
fn mcp_request() {
    each_small(|h| {
        h.app.run_command("newmcp");
        // A configured server, so the bar shows where the call goes.
        h.app.project.config.mcp.servers.insert(
            "example".into(),
            sendito_store::config::McpServer::Http(sendito_store::config::McpHttpServer {
                url: "http://127.0.0.1:9/mcp".into(),
                headers: Default::default(),
                verify_tls: true,
                auth: None,
            }),
        );
        let t = &mut h.app.tabs[h.app.active];
        t.ed.mcp_server.set_text("example");
        t.ed.mcp_tool.set_text("search");
        t.ed.message.set_text("{\"query\":\"hello\"}");
        t.refresh_model();
        snap_in(Area::Main, "mcp_tool", h);
        h.key("alt+2");
        snap_in(Area::Main, "mcp_args", h);
    });
}

#[test]
fn response_views() {
    each_small(|h| {
        open(h, "list users");
        fake_response(h, BODY, 200);
        h.key("ctrl+w j");
        snap_in(Area::Response, "resp_pretty", h);
        h.key("down enter");
        snap_in(Area::Response, "resp_folded", h);
        h.key("r");
        snap_in(Area::Response, "resp_raw", h);
        h.key("x");
        snap_in(Area::Response, "resp_hex", h);
        h.key("p alt+2");
        snap_in(Area::Response, "resp_headers", h);
        h.key("alt+3");
        snap_in(Area::Response, "resp_timing", h);
        h.key("alt+4");
        snap_in(Area::Response, "resp_messages", h);
        fake_response(h, "{\"error\":\"not found\"}", 404);
        h.key("alt+1");
        snap_in(Area::Response, "resp_404", h);
    });
}

#[test]
fn response_jq_filter() {
    each_small(|h| {
        open(h, "list users");
        fake_response(h, BODY, 200);
        h.key("ctrl+w j");
        h.key("j");
        snap_in(Area::Response, "resp_query_bar", h);
        h.type_text(".items[] | .name");
        h.key("enter");
        snap_in(Area::Response, "resp_jq_filter", h);
        h.key("j");
        h.type_text(" |");
        snap_in(Area::Response, "resp_query_error", h);
        h.key("esc");
        h.key("j");
        h.key("ctrl+a backspace enter");
        snap_in(Area::Response, "resp_jq_cleared", h);
    });
}

#[test]
fn overlays() {
    each(|h| {
        // The palette lists every key and command. The filter keeps this
        // screen on rows that a new key does not move; KEYMAP.md checks the
        // full list.
        h.key("ctrl+p");
        h.type_text("get order");
        snap_in(Area::Dialog, "palette", h);
        h.key("esc ctrl+p");
        h.type_text(":en");
        snap_in(Area::Dialog, "palette_commands", h);
        h.key("esc f1");
        snap_in(Area::Dialog, "help", h);
        h.key("esc ctrl+w ,");
        snap_in(Area::Dialog, "settings", h);
        h.key("esc f4");
        snap_in(Area::Dialog, "env_picker", h);
        h.key("esc ctrl+w V");
        snap_in(Area::Dialog, "env_editor", h);
        // The MCP servers editor shows the project file's own servers.
        let mut servers = std::collections::BTreeMap::new();
        servers.insert(
            "echo".to_string(),
            sendito_store::config::McpServer::Http(sendito_store::config::McpHttpServer {
                url: "http://127.0.0.1:9/mcp".into(),
                headers: Default::default(),
                verify_tls: true,
                auth: None,
            }),
        );
        servers.insert(
            "files".to_string(),
            sendito_store::config::McpServer::Stdio(sendito_store::config::McpStdioServer {
                command: "npx".into(),
                args: vec!["-y".into(), "@mcp/server-fs".into()],
                env: Default::default(),
            }),
        );
        sendito_store::config::set_mcp_servers(
            &sendito_store::config::project_path(h.dir.path()),
            &servers,
        )
        .unwrap();
        // The editor shows the Settings scope's file; the servers are in the
        // project one.
        h.app.config_scope = sendito_ui::overlay::ConfigScope::Project;
        h.key("esc ctrl+w S");
        snap_in(Area::Dialog, "mcp_servers", h);
        // The form of the selected server.
        h.key("enter");
        snap_in(Area::Dialog, "mcp_server_form", h);
        // An OAuth identity of the password grant.
        let ids = std::collections::BTreeMap::from([(
            "billing".to_string(),
            sendito_store::config::OAuthIdentity {
                grant: sendito_store::config::OAuthGrant::Password,
                token_url: "https://auth.example.test/token".into(),
                client_id: "sendito".into(),
                client_secret: Some("{{billing_secret}}".into()),
                username: Some("{{api_user}}".into()),
                password: Some("hunter2".into()),
                scopes: vec!["invoices:read".into()],
                ..Default::default()
            },
        )]);
        sendito_store::config::set_table(
            &sendito_store::config::project_path(h.dir.path()),
            &["oauth"],
            &ids,
        )
        .unwrap();
        h.key("esc esc");
        h.app.run_command("oauth");
        h.key("enter");
        snap_in(Area::Dialog, "oauth_form", h);
        h.key("esc esc");
        h.app.run_command("keymap");
        snap_in(Area::Dialog, "keymap_editor", h);
        h.key("esc ctrl+w i");
        snap_in(Area::Dialog, "import", h);
        h.key("esc");
        open(h, "get order");
        h.key("f5");
        snap_in(Area::Dialog, "confirm_unresolved", h);
        h.key("esc ctrl+w h n");
        snap_in(Area::Dialog, "prompt_new_request", h);
        h.key("esc");
        h.app.focus = Focus::Bar(0);
        h.key("enter");
        snap_in(Area::Main, "picker_method", h);
    });
}

#[test]
fn vim_mode_and_history() {
    each_small(|h| {
        h.app.vim = true;
        open(h, "list users");
        for t in &mut h.app.tabs {
            t.ed.set_vim(true);
        }
        snap("vim_normal", h);
        h.key("A");
        snap("vim_insert", h);
        h.key("esc");
        h.app.sidebar_mode = sendito_ui::app::SidebarMode::History;
        h.app.history = Some(vec![
            sendito_store::history::HistoryEntry {
                ts: "2026-11-21T10:15:30Z".into(),
                name: "list users".into(),
                path: Some("Users/list users.toml".into()),
                env: Some("local".into()),
                badge: "GET".into(),
                target: "http://127.0.0.1:9/users".into(),
                template: "{{base_url}}/users".into(),
                request: String::new(),
                status: Some(200),
                duration_ms: 14,
                size: 291,
                error: None,
                response: None,
            },
            sendito_store::history::HistoryEntry {
                ts: "2026-11-21T10:14:02Z".into(),
                name: "echo unary".into(),
                path: None,
                env: Some("local".into()),
                badge: "GRPC".into(),
                target: "http://127.0.0.1:9/sendito.test.v1.Echo/Unary".into(),
                template: "{{grpc_host}}/sendito.test.v1.Echo/Unary".into(),
                request: String::new(),
                status: Some(14),
                duration_ms: 3,
                size: 0,
                error: Some("UNAVAILABLE".into()),
                response: None,
            },
        ]);
        h.app.focus = Focus::Sidebar;
        snap_in(Area::Sidebar, "history", h);
    });
}

#[test]
fn nerd_icons() {
    each_small(|h| {
        h.app.icons = sendito_ui::icons::Icons::nerd();
        open(h, "list users");
        fake_response(h, "{\"ok\": true}", 200);
        snap("nerd_main", h);
    });
}

/// A completion popup over the gRPC message editor, from a fabricated schema
/// (no reflection round-trip, so the frame is deterministic).
#[test]
fn completion_popup() {
    use sendito_core::complete::FieldNode;
    use sendito_pipeline::grpc::{CallKind, MethodInfo, ServiceInfo};
    each_small(|h| {
        open(h, "echo unary");
        let mut inner = FieldNode::new("inner", "sendito.test.v1.Inner", "{}");
        inner.children.push(FieldNode::new("count", "int32", "0"));
        let mut fields = FieldNode::new("sendito.test.v1.EchoRequest", "", "{}");
        fields
            .children
            .push(FieldNode::new("text", "string", "\"\""));
        fields.children.push(FieldNode::new("i32", "int32", "0"));
        fields.children.push(inner);
        fields
            .children
            .push(FieldNode::new("tags", "repeated string", "[]"));
        let svc = ServiceInfo {
            name: "sendito.test.v1.Echo".into(),
            methods: vec![MethodInfo {
                name: "Unary".into(),
                kind: CallKind::Unary,
                input: "sendito.test.v1.EchoRequest".into(),
                output: "sendito.test.v1.EchoResponse".into(),
                input_fields: fields,
            }],
        };
        let t = &mut h.app.tabs[h.app.active];
        t.services = Some(vec![svc]);
        t.ed.message.set_text("{\n  \n}");
        t.ed.message.cursor = sendito_ui::editor::Pos { row: 1, col: 2 };
        t.refresh_model();
        h.app.focus = Focus::ReqBody;
        h.app.tabs[h.app.active].field = 0;
        h.type_text("\"");
        snap_in(Area::Main, "grpc_field_completion", h);
        h.type_text("in");
        snap_in(Area::Main, "grpc_field_completion_filtered", h);
        h.key("esc");
        // The method field completes from the same schema.
        h.app.focus = Focus::Bar(0);
        h.app.tabs[h.app.active]
            .ed
            .grpc_method_text
            .set_text("sendito.test.v1.Echo/");
        h.app.tabs[h.app.active].ed.grpc_method_text.move_to_end();
        h.key("ctrl+space");
        snap_in(Area::Main, "grpc_method_completion", h);
    });
}

#[test]
fn footer_and_ctrl_w_menu() {
    use sendito_ui::app::StatusKind;
    each(|h| {
        open(h, "list users");
        fake_response(h, BODY, 200);
        h.key("ctrl+w");
        snap("whichkey_ctrl_w", h);
        h.key("esc");
        h.app.set_status("saved users/list.http", StatusKind::Ok);
        snap_in(Area::Footer, "footer_message_ok", h);
        h.app.set_status("connection refused", StatusKind::Error);
        snap_in(Area::Footer, "footer_message_error", h);
        h.app.status = None;
        h.app.project.config.ui.hints = "full".into();
        snap_in(Area::Footer, "footer_hints_full", h);
        h.app.project.config.ui.hints = "off".into();
        snap_in(Area::Footer, "footer_hints_off", h);
    });
}
