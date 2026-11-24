//! Snapshot tests of every pane and overlay at 120×40 and 200×60.
//! Responses are fabricated so the output is deterministic.

mod common;

use std::time::Duration;

use common::{Harness, Must};
use sendito_core::resolve::{ResolvedBody, ResolvedHttp};
use sendito_http::{HttpResponse, Timing};
use sendito_ui::app::{AppMsg, Focus};
use sendito_ui::runner::{HttpOutcome, ScriptResults};
use sendito_ui::viewer::Doc;

const SIZES: [(u16, u16); 2] = [(120, 40), (200, 60)];

fn snap(name: &str, h: &mut Harness) {
    let (w, ht) = (
        h.term.backend().buffer().area.width,
        h.term.backend().buffer().area.height,
    );
    let tmp = h.dir.path().display().to_string();
    let screen = h.screen().replace(&tmp, "<tmp>");
    insta::with_settings!({ snapshot_path => "snapshots", prepend_module_to_snapshot => false }, {
        insta::assert_snapshot!(format!("{name}_{w}x{ht}"), screen);
    });
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

fn each(f: impl Fn(&mut Harness)) {
    for (w, ht) in SIZES {
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
        snap("tree_expanded", h);
        h.key("/");
        h.type_text("user");
        snap("tree_filter", h);
    });
}

#[test]
fn request_tabs_http() {
    each(|h| {
        open(h, "list users");
        snap("req_params", h);
        h.key("alt+2");
        snap("req_headers", h);
        h.key("alt+4");
        snap("req_auth_bearer", h);
        h.key("alt+6");
        snap("req_options", h);
        open(h, "create user");
        h.key("alt+3");
        snap("req_body_json", h);
        h.key("alt+5");
        snap("req_scripts", h);
    });
}

#[test]
fn request_body_kinds_and_auth() {
    each(|h| {
        open(h, "delete user");
        let t = h.app.tabs.last_mut().must();
        t.ed.set_body_kind(sendito_core::BodyKind::Form);
        t.ed.form.push_texts(true, &["username", "ana"]);
        t.ed.form.push_texts(false, &["debug", "1"]);
        t.req_tab = 2;
        t.refresh_model();
        snap("req_body_form", h);
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
        snap("req_body_multipart", h);
        let t = h.app.tabs.last_mut().must();
        t.ed.auth_kind = sendito_core::AuthKind::ApiKey;
        t.req_tab = 3;
        t.refresh_model();
        snap("req_auth_apikey", h);
        let t = h.app.tabs.last_mut().must();
        t.ed.auth_kind = sendito_core::AuthKind::Basic;
        t.refresh_model();
        snap("req_auth_basic", h);
    });
}

#[test]
fn grpc_request() {
    each(|h| {
        open(h, "echo unary");
        h.app.focus = Focus::Bar(1);
        snap("grpc_message", h);
        h.key("alt+5");
        snap("grpc_options", h);
    });
}

#[test]
fn mcp_request() {
    each(|h| {
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
        snap("mcp_tool", h);
        h.key("alt+2");
        snap("mcp_args", h);
    });
}

#[test]
fn response_views() {
    each(|h| {
        open(h, "list users");
        fake_response(h, BODY, 200);
        h.key("ctrl+w j");
        snap("resp_pretty", h);
        h.key("down enter");
        snap("resp_folded", h);
        h.key("r");
        snap("resp_raw", h);
        h.key("x");
        snap("resp_hex", h);
        h.key("p alt+2");
        snap("resp_headers", h);
        h.key("alt+3");
        snap("resp_timing", h);
        h.key("alt+4");
        snap("resp_messages", h);
        fake_response(h, "{\"error\":\"not found\"}", 404);
        h.key("alt+1");
        snap("resp_404", h);
    });
}

#[test]
fn response_jq_filter() {
    each(|h| {
        open(h, "list users");
        fake_response(h, BODY, 200);
        h.key("ctrl+w j");
        h.key("j");
        snap("resp_jq_prompt", h);
        h.type_text(".items[] | .name");
        h.key("enter");
        snap("resp_jq_filter", h);
        h.key("j");
        h.key("ctrl+a backspace enter");
        snap("resp_jq_cleared", h);
    });
}

#[test]
fn overlays() {
    each(|h| {
        h.key("ctrl+p");
        snap("palette", h);
        h.type_text(":en");
        snap("palette_commands", h);
        h.key("esc f1");
        snap("help", h);
        h.key("esc ctrl+w ,");
        snap("settings", h);
        h.key("esc f4");
        snap("env_picker", h);
        h.key("esc ctrl+w V");
        snap("env_editor", h);
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
        h.key("esc ctrl+w S");
        snap("mcp_servers", h);
        h.key("esc ctrl+w i");
        snap("import", h);
        h.key("esc");
        open(h, "get order");
        h.key("f5");
        snap("confirm_unresolved", h);
        h.key("esc ctrl+w h n");
        snap("prompt_new_request", h);
        h.key("esc");
        h.app.focus = Focus::Bar(0);
        h.key("enter");
        snap("picker_method", h);
    });
}

#[test]
fn vim_mode_and_history() {
    each(|h| {
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
            },
        ]);
        h.app.focus = Focus::Sidebar;
        snap("history", h);
    });
}

#[test]
fn nerd_icons() {
    each(|h| {
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
    use sendito_grpc::{CallKind, MethodInfo, ServiceInfo};
    each(|h| {
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
        snap("grpc_field_completion", h);
        h.type_text("in");
        snap("grpc_field_completion_filtered", h);
        h.key("esc");
        // The method field completes from the same schema.
        h.app.focus = Focus::Bar(0);
        h.app.tabs[h.app.active]
            .ed
            .grpc_method_text
            .set_text("sendito.test.v1.Echo/");
        h.app.tabs[h.app.active].ed.grpc_method_text.move_to_end();
        h.key("ctrl+space");
        snap("grpc_method_completion", h);
    });
}
