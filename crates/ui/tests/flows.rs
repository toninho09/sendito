//! Scripted key/mouse sequences for the UX flows, asserting real outcomes
//! (files on disk, what the servers received, what the screen shows).

mod common;

use common::{Harness, Must};
use sendito_testkit::{grpc_echo, http_echo, ws_echo};

struct Servers {
    http: http_echo::HttpEcho,
    grpc: grpc_echo::GrpcEcho,
    ws: ws_echo::WsEcho,
    _rt: tokio::runtime::Runtime,
}

fn servers() -> Servers {
    let rt = tokio::runtime::Runtime::new().must();
    let (http, grpc, ws) = rt.block_on(async {
        (
            http_echo::start().await.must(),
            grpc_echo::start().await.must(),
            ws_echo::start().await.must(),
        )
    });
    Servers {
        http,
        grpc,
        ws,
        _rt: rt,
    }
}

fn harness(s: &Servers, w: u16, h: u16) -> Harness {
    let (hu, gt) = (s.http.url(), s.grpc.target());
    let mut h = Harness::with(w, h, |d| common::demo_collection(d, &hu, &gt));
    h.app.set_env(Some("local".into()));
    h
}

fn open(h: &mut Harness, name: &str) {
    h.key("ctrl+w h");
    h.app.tree.set_filter(name);
    h.key("enter");
    h.app.tree.set_filter("");
    h.draw();
}

/// An `http(s)://` URL in the server field connects without a config entry.
#[test]
fn flow_mcp_direct_url_server() {
    let rt = tokio::runtime::Runtime::new().must();
    let echo = rt.block_on(sendito_testkit::mcp_echo::start()).must();
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
    });
    h.app.run_command("newmcp");
    // Type the URL straight into the server input (no [mcp.servers] entry).
    h.app.focus = sendito_ui::app::Focus::Bar(1);
    h.type_text(&format!("{}/mcp", echo.url()));
    h.draw();
    // The tool chip discovers tools from that URL.
    h.app.focus = sendito_ui::app::Focus::Bar(0);
    h.key("enter");
    h.wait_for("Choose an MCP tool", 10);
    h.type_text("add");
    h.key("enter");
    assert_eq!(h.app.tabs[h.app.active].ed.mcp_tool.text(), "add");
    h.key("f5");
    h.wait_idle(10);
    assert!(
        echo.captured()
            .iter()
            .any(|c| c.method_of_body() == "tools/call")
    );
}

/// A server that answers 401 with a Bearer challenge offers a sign-in, also
/// when the config has no auth table for it. Cancel leaves the config as is.
#[test]
fn flow_mcp_server_asking_for_a_sign_in_offers_it() {
    use sendito_ui::overlay::{ConfirmAction, McpAfterLogin, Overlay};
    let rt = tokio::runtime::Runtime::new().must();
    let srv = rt.block_on(sendito_testkit::mcp_auth::start()).must();
    let config = format!(
        "[collection]\nname = \"demo\"\n[mcp.servers.secure]\nurl = \"{}\"\n",
        srv.mcp_url()
    );
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        common::write(d, ".sendito/config.toml", &config);
    });
    h.app.run_command("newmcp");
    h.app.focus = sendito_ui::app::Focus::Bar(1);
    h.type_text("secure");
    h.app.run_command("mcptools");
    let sc = h.wait_for("asks you to sign in", 10);
    assert!(sc.contains("Sign in"), "{sc}");
    assert!(matches!(
        &h.app.overlay,
        Some(Overlay::Confirm(c)) if matches!(
            &c.action,
            ConfirmAction::McpLogin { server, after: McpAfterLogin::Tools { open_picker: true }, .. }
                if server == "secure"
        )
    ));
    h.key("esc");
    assert!(h.app.overlay.is_none());
    let text = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert_eq!(text, config, "cancel writes nothing");
}

/// The servers editor shows and writes the auth of an HTTP server.
#[test]
fn flow_mcp_editor_shows_and_writes_auth() {
    use sendito_ui::overlay::Overlay;
    let mut h = Harness::with(160, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        common::write(
            d,
            ".sendito/config.toml",
            "[collection]\nname = \"demo\"\n\
             [mcp.servers.a]\nurl = \"https://a.test/mcp\"\n\
             [mcp.servers.a.auth]\ntype = \"oauth\"\nscopes = [\"read\"]\n\
             [mcp.servers.b]\nurl = \"https://b.test/mcp\"\n",
        );
    });
    h.key("ctrl+w S");
    let sc = h.screen();
    assert!(sc.contains("Auth") && sc.contains("oauth"), "{sc}");
    // Row b: Auth to bearer and a token variable.
    match &mut h.app.overlay {
        Some(Overlay::Mcp(m)) => {
            m.table.rows[1].cells[3] = sendito_ui::form::Cell::Choice(1);
            m.table.rows[1].cells[4] =
                sendito_ui::form::Cell::Text(sendito_ui::editor::TextEditor::single("{{tok}}"));
        }
        _ => panic!("the MCP editor did not open"),
    }
    h.key("ctrl+s");
    let text = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    // a keeps its OAuth fields; b gets the bearer token.
    assert!(
        text.contains("type = \"oauth\"") && text.contains("\"read\""),
        "{text}"
    );
    assert!(
        text.contains("type = \"bearer\"") && text.contains("{{tok}}"),
        "{text}"
    );
}

#[test]
fn flow_mcp_discover_and_call() {
    let rt = tokio::runtime::Runtime::new().must();
    let echo = rt.block_on(sendito_testkit::mcp_echo::start()).must();
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        common::write(
            d,
            ".sendito/config.toml",
            &format!(
                "[collection]\nname = \"demo\"\n[mcp.servers.echo]\nurl = \"{}/mcp\"\n",
                echo.url()
            ),
        );
    });
    h.app.run_command("newmcp");
    // The server input takes a config name; F2 lists the configured servers.
    h.key("f2");
    h.key("enter");
    assert_eq!(h.app.tabs[h.app.active].ed.mcp_server.text(), "echo");
    // The bar shows where the call goes: the configured server's URL, when
    // the strip is wide enough (120 columns: the test harness).
    h.key("right");
    assert!(h.screen().contains("/mcp") || h.screen().contains("127.0.0.1"));
    h.app.run_command("mcptools");
    h.wait_for("Choose an MCP tool", 5);
    h.key("enter");
    let tab = &h.app.tabs[h.app.active];
    assert_eq!(tab.ed.mcp_tool.text(), "add");
    let args: serde_json::Value = serde_json::from_str(&tab.ed.message.text()).must();
    assert_eq!(args, serde_json::json!({"a": 0, "b": 0}));
    h.key("f5");
    h.wait_idle(5);
    assert!(h.screen().contains("OK"));
    assert!(
        echo.captured()
            .iter()
            .any(|c| c.method_of_body() == "tools/call")
    );

    // Secrets echoed by a tool must not reach the UI or history.
    h.app.set_env(Some("local".into()));
    let t = &mut h.app.tabs[h.app.active];
    t.ed.message.set_text(r#"{"a":"{{token}}"}"#);
    t.refresh_model();
    h.key("f5");
    h.wait_idle(5);
    let response = h.app.tabs[h.app.active].response.as_ref().must();
    assert!(!response.body.doc.raw.text.contains("s3cr3t-token"));
    assert!(!response.messages.doc.raw.text.contains("s3cr3t-token"));
    let history = std::fs::read_to_string(h.dir.path().join(".sendito/history.jsonl")).must();
    assert!(!history.contains("s3cr3t-token"));
}

#[test]
fn mcp_discovery_can_be_cancelled() {
    let rt = tokio::runtime::Runtime::new().must();
    let listener = rt
        .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
        .must();
    let addr = listener.local_addr().must();
    rt.spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let _socket = socket;
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            });
        }
    });
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        common::write(
            d,
            ".sendito/config.toml",
            &format!("[mcp.servers.slow]\nurl = \"http://{addr}/mcp\"\n"),
        );
    });
    h.app.run_command("newmcp");
    h.app.tabs[h.app.active].ed.mcp_server.set_text("slow");
    h.app.run_command("mcptools");
    assert!(h.app.is_busy());
    h.key("ctrl+w x");
    assert!(!h.app.is_busy());
    assert!(h.screen().contains("discovery cancelled"));
}

#[test]
fn mcp_invalid_arguments_save_and_reopen() {
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
    });
    h.app.run_command("newmcp");
    h.app.run_command("saveas");
    // Name the file at the collection root.
    h.type_text("half-typed");
    h.key("enter");
    let path = h.dir.path().join("half-typed.toml");
    assert!(path.is_file(), "the save-as dialog must create the file");
    // A half-typed object must save and reopen: it is a valid draft, only the
    // send rejects it.
    h.app.tabs[h.app.active].ed.message.set_text("{\"a\": ");
    h.app.tabs[h.app.active].refresh_model();
    h.key("ctrl+s");
    assert!(path.is_file());
    let text = std::fs::read_to_string(&path).must();
    assert!(
        text.contains("arguments"),
        "arguments must be written: {text}"
    );
    h.key("ctrl+w c");
    h.key("ctrl+w h");
    h.app.tree.set_filter("half-typed");
    h.key("enter");
    h.app.tree.set_filter("");
    assert!(
        h.app.tabs[h.app.active]
            .ed
            .message
            .text()
            .contains("\"a\":"),
        "reopening must restore the half-typed arguments"
    );
}

#[test]
fn mcp_success_status_is_not_an_http_error_code() {
    let rt = tokio::runtime::Runtime::new().must();
    let echo = rt.block_on(sendito_testkit::mcp_echo::start()).must();
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        common::write(
            d,
            ".sendito/config.toml",
            &format!("[mcp.servers.echo]\nurl = \"{}/mcp\"\n", echo.url()),
        );
    });
    h.app.run_command("newmcp");
    let t = &mut h.app.tabs[h.app.active];
    t.ed.mcp_server.set_text("echo");
    t.ed.mcp_tool.set_text("add");
    t.ed.message.set_text("{}");
    t.refresh_model();
    h.key("f5");
    h.wait_idle(5);
    let summary = h.app.tabs[h.app.active]
        .response
        .as_ref()
        .must()
        .summary
        .clone();
    assert_eq!(summary.status, "OK");
    assert_eq!(summary.code, None, "MCP must not carry an HTTP status code");
    assert!(summary.ok);
}

#[test]
fn mcp_up_from_tabs_focuses_the_tool_input() {
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
    });
    h.app.run_command("newmcp");
    h.app.tabs[h.app.active].ed.mcp_server.set_text("echo");
    h.app.tabs[h.app.active].refresh_model();
    h.key("alt+1");
    // Up from the tabs lands on the endpoint input every kind draws (the
    // tool for MCP), never on a slot it does not draw.
    h.key("up");
    assert_eq!(h.app.focus, sendito_ui::app::Focus::Bar(1));
}

#[test]
fn flow_create_folder_and_request() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    h.key("ctrl+w h");
    h.key("N");
    assert!(h.screen().contains("New folder"));
    h.type_text("Payments");
    // Choose the collection root as the parent.
    h.key("tab");
    while h.app.overlay.as_ref().is_some_and(
        |o| matches!(o, sendito_ui::overlay::Overlay::Prompt(p) if p.choices[0].2 != 0),
    ) {
        h.key("left");
    }
    // Space on a choice row opens its list; choosing returns to the name.
    h.key("space");
    assert!(h.app.overlay.as_ref().is_some_and(
        |o| matches!(o, sendito_ui::overlay::Overlay::Prompt(p) if p.open == Some(0)),
    ));
    h.key("enter");
    h.key("enter");
    assert!(h.dir.path().join("Payments").is_dir());
    assert!(h.screen().contains("Payments/"));
    h.key("n");
    let sc = h.screen();
    assert!(
        sc.contains("New request") && sc.contains("/Payments"),
        "{sc}"
    );
    h.type_text("charge card");
    h.key("enter");
    let file = h.dir.path().join("Payments/charge card.toml");
    assert!(file.is_file());
    // The new request opens with the URL focused.
    h.type_text("{{base_url}}/charges");
    assert!(h.screen().contains("●"));
    h.key("ctrl+s");
    let text = std::fs::read_to_string(&file).must();
    assert!(text.contains("url = \"{{base_url}}/charges\""), "{text}");
    assert!(h.app.tabs.iter().all(|t| !t.dirty()));
    assert!(!h.screen().contains("unsaved"));
    // Rename and duplicate from the tree.
    h.key("ctrl+w h");
    h.app.tree.select_rel("Payments/charge card.toml");
    h.key("d");
    assert!(
        h.dir
            .path()
            .join("Payments/charge card copy.toml")
            .is_file()
    );
    h.key("r");
    h.key("ctrl+a");
    h.type_text("refund");
    h.key("enter");
    assert!(h.dir.path().join("Payments/refund.toml").is_file());
    h.key("delete");
    assert!(h.screen().contains("Delete the request"));
    h.key("left enter");
    assert!(!h.dir.path().join("Payments/refund.toml").exists());
}

/// A large tree can be browsed with the mouse wheel without snapping back
/// to the selection; moving the selection still scrolls it into view.
#[test]
fn flow_tree_wheel_scrolls_freely() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    for i in 0..60 {
        common::write(
            h.dir.path(),
            &format!("Big/route {i:02}.toml"),
            &format!("kind = \"http\"\nmethod = \"GET\"\nurl = \"{{{{base_url}}}}/r{i}\"\n"),
        );
    }
    h.app.rescan();
    h.key("ctrl+w h");
    h.app.tree.select_rel("Big");
    h.draw();
    h.key("right"); // expand
    for _ in 0..55 {
        h.key("down");
    }
    // Keyboard navigation keeps the selection on screen.
    let name = h
        .app
        .tree
        .selected_rel()
        .must()
        .rsplit('/')
        .next()
        .must()
        .trim_end_matches(".toml")
        .to_string();
    assert!(h.screen().contains(&name), "{name} not visible");
    // The wheel browses freely: scrolling back to the top stays at the top.
    let tr = h.app.areas.tree;
    let (wx, wy) = (tr.x + 2, tr.y + 2);
    for _ in 0..30 {
        h.scroll(wx, wy, false);
    }
    let sc = h.screen();
    assert!(
        sc.contains("route 00") && sc.contains("route 01"),
        "wheel scroll snapped back to the selection\n{sc}"
    );
    // Moving the selection again scrolls it back into view.
    h.key("down");
    let name = h
        .app
        .tree
        .selected_rel()
        .must()
        .rsplit('/')
        .next()
        .must()
        .trim_end_matches(".toml")
        .to_string();
    assert!(h.screen().contains(&name), "{name} not visible");
}

/// The history sidebar behaves the same way: free wheel browsing, selection
/// kept visible while navigating with the keyboard.
#[test]
fn flow_history_wheel_scrolls_freely() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    let entries: Vec<_> = (0..80)
        .map(|i| sendito_store::history::HistoryEntry {
            ts: format!("2026-01-{:02}T10:00:00Z", 1 + i % 28),
            name: format!("hist{i:02}"),
            path: None,
            env: None,
            badge: "GET".into(),
            target: format!("http://x/r{i}"),
            template: String::new(),
            request: String::new(),
            status: Some(200),
            duration_ms: 1,
            size: 1,
            error: None,
        })
        .collect();
    h.key("f7");
    h.key("ctrl+w h");
    h.app.history = Some(entries);
    h.draw();
    for _ in 0..75 {
        h.key("j");
    }
    // Keyboard navigation keeps the selection on screen.
    let sel = |h: &Harness| {
        h.app
            .history
            .as_ref()
            .map(|v| v[h.app.history_sel].name.clone())
            .must()
    };
    let name = sel(&h);
    assert!(h.screen().contains(&name), "{name} not visible");
    // The wheel browses freely back to the top.
    let sr = h.app.areas.sidebar;
    let (wx, wy) = (sr.x + 2, sr.y + 2);
    for _ in 0..30 {
        h.scroll(wx, wy, false);
    }
    let sc = h.screen();
    assert!(sc.contains("hist00"), "wheel scroll snapped back\n{sc}");
    // Moving the selection again scrolls it back into view.
    h.key("j");
    let name = sel(&h);
    assert!(h.screen().contains(&name), "{name} not visible");
}

/// Every kind draws the same URL strip: a picker chip (part 0) and one
/// endpoint input (part 1) — WebSocket has no chip, only the input.
#[test]
fn flow_bar_has_the_same_parts_for_every_kind() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    use sendito_ui::app::Focus;

    // HTTP: method chip + URL input.
    open(&mut h, "list users");
    h.app.focus = Focus::Bar(1);
    h.key("ctrl+a");
    h.type_text("http://x/1");
    assert_eq!(h.app.tabs[h.app.active].ed.url.text(), "http://x/1");

    // gRPC: method chip + target input.
    open(&mut h, "echo unary");
    h.app.focus = Focus::Bar(0);
    h.key("enter");
    assert!(h.wait_for("Choose a gRPC method", 5).contains("Echo/Unary"));
    h.key("esc");
    h.app.focus = Focus::Bar(1);
    h.key("ctrl+a");
    h.type_text("127.0.0.1:1");
    assert_eq!(h.app.tabs[h.app.active].ed.target.text(), "127.0.0.1:1");

    // WebSocket: static badge + URL input (part 0 does not exist).
    h.app.run_command("newws");
    h.app.focus = Focus::Bar(1);
    h.type_text("ws://x/1");
    assert_eq!(h.app.tabs[h.app.active].ed.url.text(), "ws://x/1");
    // No chip part: shift+tab from the input reaches the sidebar directly.
    h.key("shift+tab");
    assert_eq!(h.app.focus, Focus::Sidebar);

    // MCP: server input + tool chip, like gRPC's target + method chip.
    h.app.run_command("newmcp");
    h.app.focus = Focus::Bar(1);
    h.type_text("search");
    assert_eq!(h.app.tabs[h.app.active].ed.mcp_server.text(), "search");
    h.app.focus = Focus::Bar(0);
    h.type_text("tool");
    assert_eq!(h.app.tabs[h.app.active].ed.mcp_tool.text(), "tool");
}

#[test]
fn flow_send_get_and_post_json() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    open(&mut h, "health");
    h.key("f5");
    h.wait_idle(10);
    let sc = h.screen();
    assert!(sc.contains("200 OK"), "{sc}");
    assert_eq!(
        s.http.take().last().map(|c| c.target()),
        Some("/status/200".to_string())
    );

    open(&mut h, "create user");
    // Body tab, then the editor: replace the JSON.
    h.key("alt+3");
    h.key("enter down");
    h.key("ctrl+a");
    h.type_text("{\"name\": \"Zé\"}");
    h.key("ctrl+enter");
    h.wait_idle(10);
    let caps = s.http.take();
    let c = caps.last().must();
    assert_eq!(c.method(), "POST");
    assert_eq!(c.body(), "{\"name\": \"Zé\"}".as_bytes());
    assert_eq!(
        c.header("content-type").as_deref(),
        Some("application/json")
    );
    let sc = h.screen();
    assert!(sc.contains("200 OK"), "{sc}");
    // The request has an assertion; the Messages tab shows it passed.
    h.key("ctrl+w j alt+4");
    assert!(h.screen().contains("PASS  status == 200"));
    // Response search and view modes.
    h.key("alt+1");
    h.key("/");
    h.type_text("server-session");
    h.key("enter");
    {
        let sc = h.screen();
        assert!(sc.contains("match 1/1"), "{sc}");
    }
    h.key("x");
    let sc = h.screen();
    assert!(sc.contains("0000") && sc.contains("│"), "{sc}");
    h.key("p");
}

#[test]
fn flow_response_jq_filter() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    open(&mut h, "create user");
    h.key("f5");
    h.wait_idle(10);
    // The echo body: method, target, headers, session, …
    let dbg = h.screen();
    assert!(dbg.contains("\"method\""), "{dbg}");
    h.key("ctrl+w j");
    // j opens the query bar; the body follows the expression as it is typed.
    h.key("j");
    assert!(h.screen().contains("jq ▸"));
    h.type_text(".session");
    {
        let sc = h.screen();
        assert!(sc.contains("server-session-1"), "{sc}");
        assert!(sc.contains("1 result"), "{sc}");
        assert!(!sc.contains("\"method\""), "{sc}");
    }
    // Enter keeps the filter and the bar.
    h.key("enter");
    assert!(h.screen().contains("jq ▸ .session"));
    // :jq re-filters the original body, not the filtered output.
    h.app.run_command("jq .method");
    {
        let sc = h.screen();
        assert!(sc.contains("jq ▸ .method"), "{sc}");
        assert!(sc.contains("\"POST\""), "{sc}");
        assert!(!sc.contains("server-session-1"), "{sc}");
    }
    // A bad expression shows its error; the body keeps the last valid
    // result (here `.a`) and Enter does not leave the bar.
    h.key("j");
    h.key("ctrl+a");
    h.type_text(".a[");
    h.key("enter");
    {
        let sc = h.screen();
        assert!(sc.contains("✗"), "{sc}");
        assert!(sc.contains("null"), "{sc}");
    }
    // Esc goes back to the filter that applies.
    h.key("esc");
    {
        let sc = h.screen();
        assert!(sc.contains("jq ▸ .method"), "{sc}");
        assert!(!sc.contains("✗"), "{sc}");
    }
    // An empty expression clears the filter and closes the bar.
    h.key("j");
    h.key("ctrl+a");
    h.key("backspace");
    assert!(h.screen().contains("\"method\""));
    h.key("enter");
    {
        let sc = h.screen();
        assert!(!sc.contains("jq ▸"), "{sc}");
        assert!(sc.contains("\"method\""), "{sc}");
    }
    // Up walks the expressions used before.
    h.key("j");
    h.key("up");
    assert!(h.screen().contains("jq ▸ .method"));
    h.key("up");
    assert!(h.screen().contains("jq ▸ .session"));
    h.key("esc");
    // A new response replaces the body and drops the filter.
    h.app.run_command("jq .target");
    assert!(h.screen().contains("jq ▸ .target"));
    h.key("f5");
    h.wait_idle(10);
    let sc = h.screen();
    assert!(!sc.contains("jq ▸"), "{sc}");
    assert!(sc.contains("\"method\""), "{sc}");
}

#[test]
fn flow_response_jq_complete_and_path() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    open(&mut h, "create user");
    h.key("f5");
    h.wait_idle(10);
    h.key("ctrl+w j");
    // Tab completes a key of the body; one match goes in at once.
    h.key("j");
    h.type_text(".sess");
    h.key("tab");
    {
        let sc = h.screen();
        assert!(sc.contains("jq ▸ .session"), "{sc}");
        assert!(sc.contains("server-session-1"), "{sc}");
    }
    // More matches open the list, with the type of each value.
    h.key("ctrl+a");
    h.type_text(".");
    h.key("tab");
    {
        let sc = h.screen();
        assert!(sc.contains("method") && sc.contains("string"), "{sc}");
    }
    h.key("esc esc");
    assert!(!h.screen().contains("jq ▸"));
    // The path of the line at the cursor shows in the bottom border; f
    // filters the body to it and Y copies it.
    h.key("down");
    let path = {
        let sc = h.screen();
        let row = sc
            .lines()
            .find(|l| l.contains("Ln 2/"))
            .unwrap_or_default()
            .to_string();
        let start = row.find(" .").map(|i| i + 1).unwrap_or(0);
        row[start..]
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    };
    assert!(path.starts_with('.') && path.len() > 1, "{path}");
    h.key("Y");
    assert!(h.screen().contains(&format!("copied {path}")));
    h.key("f");
    assert!(h.screen().contains(&format!("jq ▸ {path}")));
}

#[test]
fn flow_multipart_upload() {
    let s = servers();
    let mut h = harness(&s, 120, 34);
    std::fs::write(h.dir.path().join("upload.txt"), "file contents\n").must();
    h.key("ctrl+n");
    h.type_text("{{base_url}}/upload");
    h.key("alt+3");
    h.key("enter");
    assert!(h.screen().contains("Body type"));
    h.type_text("multi");
    h.key("enter");
    assert!(h.screen().contains("Multipart"));
    h.key("down");
    // Part 1: text field.
    h.type_text("note");
    h.key("enter enter");
    h.type_text("hello");
    // Part 2: a file (moving down onto the add row starts at its first column).
    h.key("down");
    h.type_text("doc");
    h.key("enter space enter");
    h.type_text("upload.txt");
    h.key("f5");
    h.wait_idle(10);
    let caps = s.http.take();
    let c = caps.last().expect("request received");
    assert_eq!(c.method(), "GET");
    let body = String::from_utf8_lossy(&c.body()).into_owned();
    assert!(
        c.header("content-type")
            .unwrap_or_default()
            .starts_with("multipart/form-data; boundary=")
    );
    assert!(body.contains("name=\"note\"\r\n\r\nhello\r\n"), "{body}");
    assert!(body.contains("name=\"doc\"; filename=\"upload.txt\"\r\nContent-Type: text/plain\r\n\r\nfile contents\n"), "{body}");
}

#[test]
fn flow_switch_env() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    assert!(h.screen().contains("local ▾"));
    h.key("f4");
    h.type_text("stag");
    h.key("enter");
    assert!(h.screen().contains("staging ▾"));
    // Undefined variables are flagged before sending.
    open(&mut h, "get order");
    h.key("f5");
    let sc = h.screen();
    assert!(sc.contains("Undefined variables"), "{sc}");
    assert!(sc.contains("{{order_id}}"));
    h.key("esc");
    assert!(h.app.tabs[h.app.active].running.is_none());
    // Palette command.
    h.key("ctrl+p");
    h.type_text(":env local");
    h.key("enter");
    assert!(h.screen().contains("local ▾"));
    // Env editor shows sources, masks secrets.
    h.key("ctrl+w V");
    let sc = h.screen();
    assert!(
        sc.contains("Environments") && sc.contains("Source says where a value lives"),
        "{sc}"
    );
    assert!(!sc.contains("s3cr3t-token"));
    h.key("esc");
}

#[test]
fn flow_unary_grpc_reflection() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "echo unary");
    h.key("f5");
    h.wait_idle(10);
    let sc = h.wait_for("OK (0)", 5);
    assert!(sc.contains("\"text\": \"hello\""), "{sc}");
    // Method picker from reflection, template generation.
    h.key("ctrl+w k");
    h.app.focus = sendito_ui::app::Focus::Bar(0);
    h.key("enter");
    let sc = h.wait_for("Choose a gRPC method", 5);
    assert!(sc.contains("sendito.test.v1.Echo/Bidi"), "{sc}");
    h.type_text("ServerStream");
    h.key("enter");
    assert_eq!(h.app.tabs[h.app.active].ed.method(), "ServerStream");
}

#[test]
fn flow_bidi_stream() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    h.key("ctrl+p");
    h.type_text(":newgrpc");
    h.key("enter");
    h.type_text(&s.grpc.target());
    h.key("tab enter");
    let _ = h.wait_for("Choose a gRPC method", 5);
    h.type_text("Echo/Bidi");
    h.key("enter");
    // Template appears; replace with our first message.
    h.wait_for("\"text\": \"\"", 5);
    h.key("alt+1 enter");
    h.key("ctrl+a");
    h.type_text("{\"text\": \"one\"}");
    h.key("f5");
    h.wait_for("bidi stream open", 5);
    h.key("ctrl+a");
    h.type_text("{\"text\": \"two\"}");
    h.key("f5");
    h.key("ctrl+w d");
    h.wait_idle(10);
    h.key("ctrl+w j alt+4");
    let sc = h.wait_for("OK (0)", 5);
    assert!(sc.contains("received"), "{sc}");
    // The body lists every message and shows the last one.
    h.key("alt+1");
    {
        let sc = h.screen();
        assert!(sc.contains("2 messages · live"), "{sc}");
        assert!(sc.contains("\"two\""), "{sc}");
    }
    // [ shows the previous message and pauses live mode; ] goes back.
    h.key("[");
    {
        let sc = h.screen();
        assert!(sc.contains("2 messages · 1/2"), "{sc}");
        assert!(sc.contains("\"text\": \"one\""), "{sc}");
    }
    // A jq filter applies to the message shown and stays on the others.
    h.key("j");
    h.type_text(".text");
    h.key("enter");
    assert!(h.screen().contains("jq ▸ .text"));
    h.key("]");
    {
        let sc = h.screen();
        assert!(sc.contains("2 messages · live"), "{sc}");
        assert!(sc.contains("jq ▸ .text"), "{sc}");
    }
    let caps = s.grpc.take();
    let bidi = caps.iter().find(|c| c.method == "Bidi").expect("bidi call");
    assert_eq!(
        bidi.messages,
        vec![
            "{\"text\":\"one\"}".to_string(),
            "{\"text\":\"two\"}".to_string()
        ]
    );
}

#[test]
fn flow_postman_import() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/acme.postman_collection.json");
    h.key("ctrl+w i");
    h.paste(&fixture.display().to_string());
    h.key("f5");
    let sc = h.screen();
    assert!(sc.contains("Postman import"), "{sc}");
    assert!(sc.contains("requests:  32"), "{sc}");
    assert!(sc.contains("Needs action"), "{sc}");
    h.key("esc");
    assert!(
        h.dir
            .path()
            .join("Acme API/Users/Admin/Ban user.toml")
            .is_file()
    );
    let coll = std::fs::read_to_string(h.dir.path().join(".sendito/collection.toml")).must();
    assert!(coll.contains("base = \"https://api.acme.test\""));
}

#[test]
fn flow_http_oauth_invalid_endpoint_reports_error() {
    let mut h = Harness::new(120, 40);
    h.app.project.config.oauth.insert(
        "service".into(),
        sendito_store::config::OAuthIdentity {
            authorize_url: "http://example.test/auth".into(),
            token_url: "http://example.test/token".into(),
            client_id: "client".into(),
            ..Default::default()
        },
    );
    h.app.run_command("auth login service");
    h.wait_idle(5);
    let screen = h.screen();
    assert!(screen.contains("OAuth login failed"), "{screen}");
}

#[test]
fn flow_cookie_jar_list_and_delete() {
    let mut h = Harness::new(120, 40);
    h.app.set_env(Some("local".into()));
    sendito_store::cookies::store_response(
        h.dir.path(),
        "local",
        "https://example.test/",
        &["session=private-value; Path=/".into()],
    )
    .must();
    h.app.run_command("cookies");
    let screen = h.wait_for("Cookie jar", 5);
    assert!(screen.contains("session"), "{screen}");
    assert!(screen.contains("Clear all cookies"), "{screen}");
    assert!(!screen.contains("private-value"));
    // Enter on the cookie asks first; Delete removes it.
    h.key("enter");
    assert!(matches!(
        h.app.overlay,
        Some(sendito_ui::overlay::Overlay::Confirm(_))
    ));
    h.key("left");
    h.key("enter");
    h.wait_for("cookie deleted", 5);
    assert!(
        sendito_store::cookies::describe(h.dir.path(), "local")
            .must()
            .is_empty()
    );
}

#[test]
fn flow_bruno_and_insomnia_import() {
    let mut h = Harness::new(120, 40);
    let fixtures = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures");
    for (name, title, expected) in [
        ("bruno", "Bruno import", "Bruno demo/Users/List.toml"),
        (
            "insomnia.json",
            "Insomnia import",
            "Insomnia demo/Users/Create.toml",
        ),
    ] {
        h.key("ctrl+w i");
        h.paste(&fixtures.join(name).display().to_string());
        h.key("f5");
        let screen = h.screen();
        assert!(screen.contains(title), "{screen}");
        assert!(screen.contains("requests:  1"), "{screen}");
        h.key("esc");
        assert!(h.dir.path().join(expected).is_file());
    }
}

#[test]
fn flow_curl_paste_and_export() {
    let s = servers();
    let mut h = harness(&s, 120, 34);
    h.key("ctrl+n");
    let tabs = h.app.tabs.len();
    h.paste("curl -X POST 'https://api.example.com/items?x=1' -H 'X-Token: abc' --data-raw '{\"a\":1}' -H 'Content-Type: application/json'");
    // The empty tab is filled instead of a new one being opened.
    assert_eq!(h.app.tabs.len(), tabs);
    let t = &h.app.tabs[h.app.active];
    assert_eq!(t.ed.method, "POST");
    assert_eq!(t.ed.url.text(), "https://api.example.com/items?x=1");
    h.key("ctrl+w u");
    let sc = h.screen();
    assert!(sc.contains("curl") && sc.contains("X-Token: abc"), "{sc}");
    assert!(h.app.clipboard.text.contains("--data-raw"));
    h.key("esc");
    // Secrets stay masked in exports.
    open(&mut h, "list users");
    h.key("ctrl+w u");
    let sc = h.screen();
    assert!(sc.contains("Bearer {{token}}"), "{sc}");
    assert!(!sc.contains("s3cr3t-token"));
}

#[test]
fn flow_history_resend() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "list users");
    h.key("f5");
    h.wait_idle(10);
    s.http.take();
    // The echo server sends the Authorization header back: the body view masks it.
    let sc = h.screen();
    assert!(
        sc.contains("Bearer ••••••") && !sc.contains("s3cr3t-token"),
        "{sc}"
    );
    h.key("f7");
    for _ in 0..200 {
        if h.app.history.as_ref().is_some_and(|v| !v.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.pump();
    }
    let sc = h.screen();
    assert!(sc.contains("History") && sc.contains("200"), "{sc}");
    let history = std::fs::read_to_string(h.dir.path().join(".sendito/history.jsonl")).must();
    assert!(
        !history.contains("s3cr3t-token"),
        "secret leaked into history"
    );
    assert!(history.contains("{{token}}"));
    h.key("f5");
    h.wait_idle(10);
    let caps = s.http.take();
    assert_eq!(caps.len(), 1);
    assert_eq!(
        caps[0].header("authorization").as_deref(),
        Some("Bearer s3cr3t-token")
    );
    let sc = h.screen();
    assert!(sc.contains("from history ("), "{sc}");
    // The entry keeps the template, and the resent tab has no unsaved edits.
    let t = &h.app.tabs[h.app.active];
    assert_eq!(t.ed.url.text(), "{{base_url}}/users");
    assert!(!t.dirty());
    // Save the newest history entry back to the tree.
    h.key("f7");
    h.app.load_history();
    for _ in 0..200 {
        if h.app.history.as_ref().is_some_and(|v| v.len() >= 2) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.pump();
    }
    // The resent copy came from Users/list users.toml, so Users/ is the default.
    let folder = "Users";
    h.key("s");
    h.key("ctrl+a");
    h.type_text("from history");
    h.key("enter");
    let target = h.dir.path().join(folder).join("from history.toml");
    assert!(target.is_file(), "{}", h.screen());
    let saved = std::fs::read_to_string(&target).must();
    assert!(saved.contains("{{base_url}}/users"), "{saved}");
}

#[test]
fn flow_vim_mode_editing() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    h.key("ctrl+w ,");
    // Vim mode is the fourth setting (after Theme, Icons and Key hints).
    h.key("down down down enter esc");
    assert!(h.app.vim);
    open(&mut h, "delete user");
    let sc = h.screen();
    assert!(sc.contains(" NORMAL "), "{sc}");
    // Append to the URL in vim: A, type, Esc.
    h.key("A");
    assert!(h.screen().contains(" INSERT "));
    h.type_text("/v2");
    h.key("esc");
    assert_eq!(
        h.app.tabs[h.app.active].ed.url.text(),
        "{{base_url}}/users/42/v2"
    );
    // Word motion and delete: a {{variable}} is one word.
    h.type_text("0dw");
    assert!(h.app.tabs[h.app.active].ed.url.text().starts_with("/users"));
    h.type_text("u");
    assert_eq!(
        h.app.tabs[h.app.active].ed.url.text(),
        "{{base_url}}/users/42/v2"
    );
    // :w saves.
    h.type_text(":w");
    h.key("enter");
    let text = std::fs::read_to_string(h.dir.path().join("Users/delete user.toml")).must();
    assert!(text.contains("/users/42/v2"));
    // hjkl in the tree.
    h.key("ctrl+w h");
    h.app.tree.selected = 0;
    h.type_text("jj");
    assert_eq!(h.app.tree.selected, 2);
    h.type_text("gg");
    assert_eq!(h.app.tree.selected, 0);
}

#[test]
fn flow_mouse_drag_in_tree() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    let from = h.find("health").must();
    let to = h.find("Orders/").must();
    h.drag(from, to);
    assert!(
        h.dir.path().join("Orders/health.toml").is_file(),
        "{}",
        h.screen()
    );
    assert!(!h.dir.path().join("health.toml").exists());
    // Double-click opens a request; clicking tabs and buttons works.
    let (x, y) = h.find("Orders/").must();
    h.click(x, y);
    let pos = h.find("health").must();
    h.double_click(pos.0 + 1, pos.1);
    let sc = h.screen();
    assert!(sc.contains("Orders › health"), "{sc}");
    let send = h.find(" Send").must();
    h.click(send.0 + 1, send.1);
    h.wait_idle(10);
    assert!(h.screen().contains("200 OK"));
    // Resize the sidebar by dragging its border.
    let before = h.app.layout.sidebar_width;
    let bx = h.app.areas.sidebar.x + h.app.areas.sidebar.width - 1;
    h.drag((bx, 10), (bx + 6, 10));
    assert!(h.app.layout.sidebar_width > before);
    // Wheel scrolls the response.
    let r = h.app.areas.resp_body;
    h.scroll(r.x + 2, r.y + 1, true);
}

#[test]
fn flow_untouched_palette_highlights_a_harmless_first_row() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    h.key("ctrl+p");
    // The first row is the key list: Enter on an untouched palette opens help.
    h.key("enter");
    assert!(matches!(
        h.app.overlay,
        Some(sendito_ui::overlay::Overlay::Help { .. })
    ));
    h.key("esc");
    assert!(h.app.overlay.is_none());
}

/// Clicking outside a dirty env editor asks to save instead of closing
/// silently; a clean editor still closes on the same click.
#[test]
fn flow_env_editor_outside_click_asks_when_dirty() {
    use sendito_ui::overlay::{ConfirmAction, Overlay};
    let s = servers();
    let mut h = harness(&s, 100, 30);

    // Clean editor: a click truly outside (below the modal) closes it.
    h.key("ctrl+w V");
    assert!(matches!(h.app.overlay, Some(Overlay::Env(_))));
    h.click(1, 28);
    assert!(h.app.overlay.is_none());

    // Dirty editor: the click parks it behind a save confirmation.
    h.key("ctrl+w V");
    h.key("ctrl+d");
    h.click(1, 28);
    assert!(matches!(
        h.app.overlay,
        Some(Overlay::Confirm(ref c)) if matches!(c.action, ConfirmAction::EnvClose)
    ));
    assert!(h.screen().contains("Unsaved environment changes"));
    // Discard closes for real.
    h.key("right enter");
    assert!(h.app.overlay.is_none());
}

/// A secret typed into a new env row is masked on screen right away,
/// not only after a save reloads the table.
#[test]
fn flow_env_editor_masks_new_secrets() {
    use sendito_ui::overlay::Overlay;
    let s = servers();
    let mut h = harness(&s, 100, 30);
    h.key("ctrl+w V");
    // Walk down to the trailing "add" row.
    for _ in 0..32 {
        let at_add = match &h.app.overlay {
            Some(Overlay::Env(e)) => e.table.sel_row >= e.table.rows.len(),
            _ => false,
        };
        if at_add {
            break;
        }
        h.key("down");
    }
    h.type_text("tok");
    h.key("right");
    h.key("space"); // source: secrets
    h.key("right right");
    h.type_text("hunter2");
    let screen = h.screen();
    assert!(!screen.contains("hunter2"));
    assert!(screen.contains('•'));
}

/// Clicks on the env dialog's own chrome (title, legend) and on a row's ✕
/// act inside the dialog; only the X, Esc or a truly outside click leave it.
#[test]
fn flow_env_editor_chrome_clicks_stay_inside() {
    use sendito_ui::overlay::Overlay;
    let s = servers();
    let mut h = harness(&s, 100, 30);
    h.app.set_env(Some("local".into()));
    h.key("ctrl+w V");
    let state = |h: &Harness| match &h.app.overlay {
        Some(Overlay::Env(e)) => Some((e.table.rows.len(), e.dirty)),
        _ => None,
    };
    let (rows, dirty) = state(&h).unwrap();
    assert!(rows >= 2 && !dirty);

    // A click on the dialog's title does nothing.
    let (tx, ty) = h.find("Environments").unwrap();
    h.click(tx, ty);
    assert_eq!(state(&h), Some((rows, false)));

    // The delete icon on a row deletes it instead of leaving the dialog.
    let (_, vy) = h.find("grpc").unwrap();
    let line = h.screen().split('\n').nth(vy as usize).unwrap().to_string();
    let close = sendito_ui::icons::Icons::by_name("unicode").close;
    let dx = line
        .char_indices()
        .rev()
        .find(|(_, c)| c.to_string() == close)
        .map(|(i, _)| line[..i].chars().count())
        .unwrap() as u16;
    h.click(dx, vy);
    assert_eq!(state(&h), Some((rows - 1, true)));

    // A click truly outside (below the modal) still asks before discarding.
    h.click(1, 28);
    assert!(matches!(h.app.overlay, Some(Overlay::Confirm(_))));
}

#[test]
fn flow_ctrl_s_confirms_a_dialog_from_a_choice_row() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    h.key("ctrl+w h");
    h.key("N");
    h.type_text("Billing");
    h.key("tab");
    assert!(h.screen().contains("Create (Enter)"));
    h.key("ctrl+s");
    assert!(h.app.overlay.is_none());
    assert!(h.screen().contains("Billing/"));
}

/// User themes load from the collection, `:theme reload` reads them again,
/// and the redesigned frame shows focus and dialogs with color.
#[test]
fn flow_themes_and_style() {
    use ratatui::style::Color;
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        let themes = d.join(".sendito/themes");
        std::fs::create_dir_all(&themes).must();
        std::fs::write(
            themes.join("harbor.toml"),
            "extends = \"dusk\"\n[colors]\naccent = \"#336699\"\nborder_focus = \"#336699\"\n",
        )
        .must();
        std::fs::write(d.join(".sendito/config.toml"), "[ui]\ntheme = \"harbor\"\n").must();
    });
    let rgb = |c: Color| sendito_ui::theme::to_rgb(c);
    assert_eq!(h.app.project.config.ui.theme, "harbor");
    assert_eq!(
        rgb(h.app.themes.theme_full("harbor").accent),
        rgb(Color::Rgb(0x33, 0x66, 0x99))
    );
    assert_eq!(h.app.theme.accent, h.app.themes.theme("harbor").accent);
    assert!(h.app.themes.names().contains(&"harbor".to_string()));
    // The focused pane's border takes the focus color.
    h.key("ctrl+w h");
    h.draw();
    let side = h.app.areas.sidebar;
    let cell = h.term.backend().buffer()[(side.x, side.y + 2)].clone();
    assert_eq!(cell.fg, h.app.theme.border_focus);
    // Edit the file and reload: the new color applies at once.
    std::fs::write(
        h.dir.path().join(".sendito/themes/harbor.toml"),
        "extends = \"daylight\"\n[colors]\naccent = \"#004400\"\n",
    )
    .must();
    h.key("ctrl+p");
    h.type_text(":theme reload");
    h.key("enter");
    assert!(h.app.theme.light);
    assert_eq!(
        rgb(h.app.themes.theme_full("harbor").accent),
        rgb(Color::Rgb(0, 0x44, 0))
    );
    assert_eq!(h.app.theme.accent, h.app.themes.theme("harbor").accent);
    // A dialog fades the screen behind it instead of clearing it.
    h.key("ctrl+p");
    h.draw();
    let behind = h.term.backend().buffer()[(side.x + 3, side.y + 1)].clone();
    assert_eq!(behind.bg, h.app.theme.backdrop);
    assert_ne!(behind.symbol(), " ");
    h.key("esc");
    // :theme switches to a built-in theme.
    h.key("ctrl+p");
    h.type_text(":theme ember");
    h.key("enter");
    assert_eq!(h.app.project.config.ui.theme, "ember");
    assert_eq!(&*h.app.theme.name, "ember");
}

#[test]
fn flow_help_two_columns() {
    use sendito_ui::overlay::Overlay;
    let cursor = |h: &Harness| match &h.app.overlay {
        Some(Overlay::Help { viewer, .. }) => viewer.cursor_line(),
        _ => usize::MAX,
    };
    // One column: the line is below the first page.
    let mut h = Harness::new(120, 40);
    h.key("f1");
    assert!(h.find("Move down").is_none());
    // Wide: the right column continues the left one, and a click there
    // selects the line it shows.
    let mut h = Harness::new(200, 60);
    h.key("f1");
    let (x, y) = h.find("Move down").must();
    assert!(x > 100, "{x}");
    h.click(x, y);
    let wide = cursor(&h);
    assert_ne!(wide, usize::MAX);
    assert!(wide > 40, "{wide}");
    let line = h.screen().lines().nth(y as usize).unwrap_or("").to_string();
    assert!(line.contains("Move down"));
    // PgDn moves by both columns at once.
    h.key("pagedown");
    assert!(cursor(&h) > wide);
}

#[test]
fn flow_watch_repeats_and_stops() {
    let s = servers();
    let mut h = harness(&s, 120, 34);
    open(&mut h, "health");
    h.app.run_command("watch 1");
    assert!(h.app.tabs[h.app.active].watch.is_some());
    // The first send goes out by hand; the watch repeats it.
    h.key("f5");
    h.wait_idle(10);
    if let Some(w) = h.app.tabs[h.app.active].watch.as_mut() {
        w.next = std::time::Instant::now() - std::time::Duration::from_millis(10);
    }
    assert!(h.app.tick(), "the tick should fire the watched request");
    h.wait_idle(10);
    assert!(s.http.take().len() >= 2);
    let sc = h.screen();
    assert!(sc.contains("↻ 1 s"), "{sc}");
    // The Timing tab shows one mark per send (scrolled to its end).
    h.key("ctrl+w j alt+3");
    h.key("end");
    let sc = h.screen();
    assert!(sc.contains("Watch"), "{sc}");
    assert!(sc.contains("2 sends"), "{sc}");
    // A watch remains active and sends while another tab is selected.
    let watched = h.app.active;
    h.key("ctrl+n");
    let active = h.app.active;
    assert_ne!(active, watched);
    h.app.tabs[watched].watch.as_mut().must().next =
        std::time::Instant::now() - std::time::Duration::from_millis(10);
    assert!(h.app.tick());
    assert_eq!(h.app.active, active);
    h.wait_idle(10);
    assert_eq!(s.http.take().len(), 1);
    assert_eq!(h.app.tabs[watched].watch.as_ref().must().samples.len(), 3);
    let history = std::fs::read_to_string(h.dir.path().join(".sendito/history.jsonl")).must();
    assert_eq!(
        history.lines().count(),
        1,
        "watch repeats must not flood history"
    );
    h.key("ctrl+w p");
    assert_eq!(h.app.active, watched);
    // Changing the environment stops the watch.
    h.app.set_env(Some("local".into()));
    assert!(h.app.tabs[h.app.active].watch.is_none());
    // The watch key toggles it.
    h.key("ctrl+w m");
    assert!(h.app.tabs[h.app.active].watch.is_some());
    h.key("ctrl+w m");
    assert!(h.app.tabs[h.app.active].watch.is_none());
}

/// Field names of the message come from the gRPC schema; accepting one inserts
/// a valid pair and leaves the cursor inside it.
#[test]
fn flow_grpc_message_completion() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "echo unary");
    // Load the schema.
    h.key("ctrl+w k");
    h.app.focus = sendito_ui::app::Focus::Bar(0);
    h.key("enter");
    let _ = h.wait_for("Choose a gRPC method", 5);
    h.key("esc");
    // An empty message body with the cursor on its own line.
    {
        let t = &mut h.app.tabs[h.app.active];
        t.ed.message.set_text("{\n  \n}");
        t.ed.message.cursor = sendito_ui::editor::Pos { row: 1, col: 2 };
        t.refresh_model();
    }
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.app.tabs[h.app.active].field = 0;
    h.type_text("\"tex");
    assert!(h.app.complete.is_some(), "popup opens on a key position");
    assert!(!h.app.complete.as_ref().must().items.is_empty());
    h.key("enter");
    // The cursor rests inside the new value.
    h.type_text("hi");
    let text = h.app.tabs[h.app.active].ed.message.text();
    let v: serde_json::Value = serde_json::from_str(&text).must();
    assert_eq!(v, serde_json::json!({"text": "hi"}));
    h.key("f5");
    h.wait_idle(10);
    let caps = s.grpc.take();
    let call = caps.iter().find(|c| c.method == "Unary").must();
    assert_eq!(call.messages, vec!["{\"text\":\"hi\"}".to_string()]);
}

/// Completing inside an existing key replaces the whole key and keeps its
/// value, so the JSON stays valid.
#[test]
fn flow_completion_inside_an_existing_key() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "echo unary");
    h.key("ctrl+w k");
    h.app.focus = sendito_ui::app::Focus::Bar(0);
    h.key("enter");
    let _ = h.wait_for("Choose a gRPC method", 5);
    h.key("esc");
    {
        let t = &mut h.app.tabs[h.app.active];
        t.ed.message.set_text("{\"tet\": \"x\"}");
        t.ed.message.cursor = sendito_ui::editor::Pos { row: 0, col: 4 };
        t.refresh_model();
    }
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.app.tabs[h.app.active].field = 0;
    h.type_text("x");
    assert!(h.app.complete.is_some(), "popup opens inside the key");
    h.key("enter");
    let text = h.app.tabs[h.app.active].ed.message.text();
    let v: serde_json::Value = serde_json::from_str(&text).must();
    assert_eq!(v, serde_json::json!({"text": "x"}), "{text}");
}

/// The method chip is editable and the schema completes it inline.
#[test]
fn flow_grpc_method_completion() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "echo unary");
    h.key("ctrl+w k");
    h.app.focus = sendito_ui::app::Focus::Bar(0);
    h.key("enter");
    let _ = h.wait_for("Choose a gRPC method", 5);
    h.key("esc");
    h.key("ctrl+a");
    h.type_text("sendito.test.v1.Echo/Ser");
    assert!(h.app.complete.is_some(), "popup opens on the method field");
    h.key("tab");
    let t = &h.app.tabs[h.app.active];
    assert_eq!(t.ed.service(), "sendito.test.v1.Echo");
    assert_eq!(t.ed.method(), "ServerStream");
    // The caret rests at the end of the accepted name.
    let mt = &t.ed.grpc_method_text;
    assert_eq!(
        (mt.cursor.row, mt.cursor.col),
        (0, mt.text().chars().count())
    );
}

/// Tools and their argument keys are completed from `tools/list`.
#[test]
fn flow_mcp_completion() {
    let rt = tokio::runtime::Runtime::new().must();
    let echo = rt.block_on(sendito_testkit::mcp_echo::start()).must();
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        common::write(
            d,
            ".sendito/config.toml",
            &format!(
                "[collection]\nname = \"demo\"\n[mcp.servers.echo]\nurl = \"{}/mcp\"\n",
                echo.url()
            ),
        );
    });
    h.app.run_command("newmcp");
    // The server input takes a config name; F2 lists the configured servers.
    h.key("f2");
    h.key("enter");
    assert_eq!(h.app.tabs[h.app.active].ed.mcp_server.text(), "echo");
    // The tool field pulls the tool list in the background, then filters it.
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.app.tabs[h.app.active].field = 1;
    h.type_text("ad");
    let start = std::time::Instant::now();
    while h.app.complete.as_ref().is_none_or(|c| c.items.is_empty()) {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "tool list did not arrive; tools={:?}",
            h.app.tabs[h.app.active].mcp_tools.len()
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.pump();
    }
    h.key("enter");
    assert_eq!(h.app.tabs[h.app.active].ed.mcp_tool.text(), "add");
    // Argument keys come from the tool's inputSchema.
    {
        let t = &mut h.app.tabs[h.app.active];
        t.req_tab = 1;
        t.field = 0;
        t.ed.message.set_text("{\n  \n}");
        t.ed.message.cursor = sendito_ui::editor::Pos { row: 1, col: 2 };
        t.refresh_model();
    }
    h.type_text("\"b");
    assert!(h.app.complete.is_some(), "popup opens on the args editor");
    h.key("enter");
    let args: serde_json::Value =
        serde_json::from_str(&h.app.tabs[h.app.active].ed.message.text()).must();
    assert_eq!(args, serde_json::json!({ "b": 0 }));
}

/// Esc and a click elsewhere close the popup without touching the text.
#[test]
fn flow_completion_closes() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "echo unary");
    h.key("ctrl+w k");
    h.app.focus = sendito_ui::app::Focus::Bar(0);
    h.key("enter");
    let _ = h.wait_for("Choose a gRPC method", 5);
    h.key("esc");
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.app.tabs[h.app.active].field = 0;
    h.key("ctrl+a");
    h.type_text("{");
    assert!(h.app.complete.is_some());
    h.key("esc");
    assert!(h.app.complete.is_none(), "Esc closes it");
    // A cursor move closes it too (its replacement span would be stale).
    h.type_text("{");
    assert!(h.app.complete.is_some());
    h.key("left");
    assert!(h.app.complete.is_none(), "a cursor move closes it");
    let before = h.app.tabs[h.app.active].ed.message.text();
    h.type_text("{");
    assert!(h.app.complete.is_some());
    h.click(1, 1);
    assert!(h.app.complete.is_none(), "a click elsewhere closes it");
    assert_eq!(h.app.tabs[h.app.active].ed.message.text(), before + "{");
}

/// Without a loaded schema the popup opens with a spinner and fills once the
/// background load finishes.
#[test]
fn flow_completion_fetches_schema() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "echo unary");
    {
        let t = &mut h.app.tabs[h.app.active];
        assert!(t.services.is_none());
        t.ed.message.set_text("{\n  \n}");
        t.ed.message.cursor = sendito_ui::editor::Pos { row: 1, col: 2 };
        t.refresh_model();
    }
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.app.tabs[h.app.active].field = 0;
    h.type_text("\"");
    assert_eq!(h.app.complete.as_ref().must().loading, Some("schema"));
    // The list fills when the schema arrives, still filtered by nothing.
    let start = std::time::Instant::now();
    while h.app.complete.as_ref().is_none_or(|c| c.items.is_empty()) {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "schema did not fill the popup"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.pump();
    }
    let c = h.app.complete.as_ref().must();
    assert!(c.loading.is_none());
    assert!(c.items.iter().any(|i| i.name == "text"));
    assert!(h.screen().contains("insert"), "the hint row describes it");
}

/// A schema that fails to load is reported in the popup and status, and is not
/// retried in the background.
#[test]
fn flow_completion_schema_failure() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "echo unary");
    // A dead port: reflection fails fast (connection refused).
    {
        let t = &mut h.app.tabs[h.app.active];
        t.ed.target.set_text("127.0.0.1:9");
        t.ed.message.set_text("{\n  \n}");
        t.ed.message.cursor = sendito_ui::editor::Pos { row: 1, col: 2 };
        t.refresh_model();
    }
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.app.tabs[h.app.active].field = 0;
    h.type_text("\"");
    assert_eq!(h.app.complete.as_ref().must().loading, Some("schema"));
    let start = std::time::Instant::now();
    while h.app.complete.as_ref().is_none_or(|c| c.failed.is_none()) {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "schema failure did not reach the popup"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.pump();
    }
    let c = h.app.complete.as_ref().must();
    assert_eq!(c.failed, Some("schema"));
    assert!(c.loading.is_none());
    assert!(h.app.tabs[h.app.active].schema_error.is_some());
    assert!(h.screen().contains("schema failed"));
    assert!(!h.app.is_busy(), "no load is left running");
    // Asking for the list again shows the failure; it does not refetch.
    h.key("esc");
    h.key("ctrl+space");
    let c = h.app.complete.as_ref().must();
    assert_eq!(c.failed, Some("schema"));
    assert!(c.loading.is_none());
    assert!(!h.app.is_busy(), "the dead server is not retried");
}

/// A tools/list that fails is reported once and not retried in the background.
#[test]
fn flow_completion_tools_failure() {
    let rt = tokio::runtime::Runtime::new().must();
    let echo = rt.block_on(sendito_testkit::mcp_echo::start()).must();
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        common::write(
            d,
            ".sendito/config.toml",
            &format!(
                "[collection]\nname = \"demo\"\n[mcp.servers.echo]\nurl = \"{}/mcp\"\n[mcp.servers.dead]\nurl = \"http://127.0.0.1:9/mcp\"\n",
                echo.url()
            ),
        );
    });
    h.app.run_command("newmcp");
    {
        let t = &mut h.app.tabs[h.app.active];
        t.ed.mcp_server.set_text("dead");
        t.refresh_model();
    }
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.app.tabs[h.app.active].field = 1;
    h.type_text("a");
    assert_eq!(h.app.complete.as_ref().must().loading, Some("tools"));
    let start = std::time::Instant::now();
    while h.app.complete.as_ref().is_none_or(|c| c.failed.is_none()) {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "tools failure did not reach the popup"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        h.pump();
    }
    let c = h.app.complete.as_ref().must();
    assert_eq!(c.failed, Some("tools"));
    assert!(c.loading.is_none());
    let t = &h.app.tabs[h.app.active];
    assert_eq!(t.mcp_tools_server, "dead");
    assert!(t.mcp_tools_error.is_some());
    assert!(h.screen().contains("tools failed"));
    assert!(!h.app.is_busy(), "no discovery is left running");
    // Asking for the list again shows the failure; it does not refetch.
    h.key("esc");
    h.key("ctrl+space");
    let c = h.app.complete.as_ref().must();
    assert_eq!(c.failed, Some("tools"));
    assert!(c.loading.is_none());
    assert!(!h.app.is_busy(), "the dead server is not retried");
}

/// Moving focus, switching tabs or opening a dialog closes the popup, so its
/// keys stop applying elsewhere.
#[test]
fn flow_completion_closes_on_navigation() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "echo unary");
    h.key("ctrl+w k");
    h.app.focus = sendito_ui::app::Focus::Bar(0);
    h.key("enter");
    let _ = h.wait_for("Choose a gRPC method", 5);
    h.key("esc");
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.app.tabs[h.app.active].field = 0;
    h.key("ctrl+a");
    h.type_text("{");
    assert!(h.app.complete.is_some());
    // Focus elsewhere: the popup goes with it.
    h.key("ctrl+w j");
    assert!(h.app.complete.is_none(), "focus change closes it");
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.key("ctrl+space");
    assert!(h.app.complete.is_some());
    // Another tab: same.
    h.key("ctrl+w n");
    assert!(h.app.complete.is_none(), "tab switch closes it");
    // The global chords keep working while it is open.
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.key("ctrl+space");
    assert!(h.app.complete.is_some());
    h.key("ctrl+p");
    assert!(h.app.complete.is_none(), "the palette closes it");
    assert!(
        h.app.overlay.is_some(),
        "Ctrl+P opened the palette, not the list"
    );
}

#[test]
fn flow_websocket_session() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    // A WebSocket tab from the palette, then the URL and the opening message.
    h.key("ctrl+p");
    h.type_text(":newws");
    h.key("enter");
    assert_eq!(h.app.tabs[h.app.active].ed.kind, sendito_ui::tab::Kind::Ws);
    h.type_text(&s.ws.url("/echo"));
    h.key("f5");
    h.wait_for("101 Switching Protocols", 10);
    // The editor holds the message document; sending pushes one more frame.
    h.key("alt+2 ctrl+w k enter");
    h.key("ctrl+a");
    h.type_text("\"from the editor\"");
    h.key("f5");
    // The status line confirms the frame; the server capture proves the payload.
    h.wait_for("frame 1 sent", 10);
    h.key("ctrl+w d");
    h.wait_idle(10);
    let caps = s.ws.take();
    let cap = caps.first().expect("a session reached the server");
    assert!(cap.head().starts_with("GET /echo HTTP/1.1"));
    let sent: Vec<&Vec<u8>> = cap
        .frames
        .iter()
        .filter(|(out, k, _)| !out && k == "text")
        .map(|(_, _, p)| p)
        .collect();
    // The editor content is the frame, sent verbatim (quotes included).
    assert_eq!(sent, vec![&b"\"from the editor\"".to_vec()]);
    // The close handshake reached the server with 1000.
    let closed = cap
        .frames
        .iter()
        .any(|(out, k, p)| !out && k == "close" && String::from_utf8_lossy(p).starts_with("1000"));
    assert!(closed, "{:?}", cap.frames);
}

/// Closing a tab with an open session asks first: Cancel keeps it,
/// Close ends the session and the tab.
#[test]
fn flow_close_tab_with_open_session_asks() {
    use sendito_ui::overlay::{ConfirmAction, Overlay};
    let s = servers();
    let mut h = harness(&s, 120, 40);
    h.key("ctrl+p");
    h.type_text(":newws");
    h.key("enter");
    h.type_text(&s.ws.url("/echo"));
    h.key("f5");
    h.wait_for("101 Switching Protocols", 10);
    let n = h.app.tabs.len();

    h.key("ctrl+w c");
    assert!(matches!(
        h.app.overlay,
        Some(Overlay::Confirm(ref c)) if matches!(c.action, ConfirmAction::CloseTabRunning(_))
    ));
    assert_eq!(h.app.tabs.len(), n, "the tab is still open");

    // Cancel keeps the tab and the open session.
    h.key("right enter");
    assert!(h.app.overlay.is_none());
    assert_eq!(h.app.tabs.len(), n);
    assert!(
        h.app.tabs[h.app.active]
            .running
            .as_ref()
            .is_some_and(|r| r.streaming_input)
    );

    // Close confirms, then the unsaved edits still get their own prompt.
    h.key("ctrl+w c");
    h.key("enter");
    assert!(matches!(
        h.app.overlay,
        Some(Overlay::Confirm(ref c)) if matches!(c.action, ConfirmAction::CloseTab(_))
    ));
    h.key("right enter"); // Discard
    assert_eq!(h.app.tabs.len(), n - 1);
}

/// An in-flight unary request also asks before its tab closes.
#[test]
fn flow_close_tab_with_inflight_request_asks() {
    use sendito_ui::overlay::{ConfirmAction, Overlay};
    let s = servers();
    let mut h = harness(&s, 120, 40);
    open(&mut h, "list users");
    h.app.focus = sendito_ui::app::Focus::Bar(1);
    h.key("ctrl+a");
    h.type_text(&format!("{}/delay/5000", s.http.url()));
    h.key("f5");
    h.wait_for("sending", 5);

    h.key("ctrl+w c");
    assert!(matches!(
        h.app.overlay,
        Some(Overlay::Confirm(ref c)) if matches!(c.action, ConfirmAction::CloseTabRunning(_))
    ));
    let sc = h.screen();
    assert!(sc.contains("a request in flight"), "{sc}");

    // Cancel keeps the tab and the request running.
    h.key("right enter");
    assert!(h.app.overlay.is_none());
    assert!(h.app.tabs[h.app.active].running.is_some());
}

/// The Cancel button next to Send stops a running request; with nothing
/// running it is disabled and does nothing.
#[test]
fn flow_cancel_button_stops_running_request() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    open(&mut h, "list users");
    h.app.focus = sendito_ui::app::Focus::Bar(1);
    h.key("ctrl+a");
    h.type_text(&format!("{}/delay/5000", s.http.url()));
    h.key("f5");
    h.wait_for("sending", 5);
    assert!(h.app.tabs[h.app.active].running.is_some());

    let (cx, cy) = h.find("Cancel").unwrap();
    h.click(cx + 1, cy);
    h.wait_for("Cancelled", 5);
    assert!(h.app.tabs[h.app.active].running.is_none());

    // Disabled: a click neither cancels nor reports "nothing is running".
    let (cx, cy) = h.find("Cancel").unwrap();
    h.click(cx + 1, cy);
    assert!(!h.screen().contains("nothing is running"));
}

/// Quitting with a live session asks too, even with no unsaved edits.
#[test]
fn flow_quit_with_live_session_asks() {
    use sendito_ui::overlay::Overlay;
    let s = servers();
    let mut h = harness(&s, 120, 40);
    h.key("ctrl+p");
    h.type_text(":newws");
    h.key("enter");
    h.type_text(&s.ws.url("/echo"));
    h.key("f5");
    h.wait_for("101 Switching Protocols", 10);

    h.key("ctrl+q");
    assert!(matches!(h.app.overlay, Some(Overlay::Confirm(_))));
    let sc = h.screen();
    assert!(sc.contains("still running"), "{sc}");
    assert!(!h.app.quit);

    // Cancel stays; Quit quits.
    h.key("right enter");
    assert!(!h.app.quit);
    h.key("ctrl+q");
    h.key("enter");
    assert!(h.app.quit);
}

#[test]
fn flow_websocket_snapshot_state() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    h.key("ctrl+p");
    h.type_text(":newws");
    h.key("enter");
    h.type_text(&s.ws.url("/echo"));
    h.key("f5");
    h.wait_for("101 Switching Protocols", 10);
    // The Messages tab is the second request sub-tab, focused on the document.
    h.key("alt+2");
    h.key("ctrl+w k");
    h.key("enter");
    let sc = h.screen();
    assert!(sc.contains("Messages"), "{sc}");
    assert_eq!(
        h.app.tab().and_then(|t| t.current_field()),
        Some(sendito_ui::tab::Field::WsMessages)
    );
    assert!(sc.contains("WS"), "{sc}");
}

/// Servers can be added, renamed and removed from the interface: the project
/// config keeps its other content, and settings the table does not show
/// (auth, headers, env) survive a save that leaves the entry alone.
#[test]
fn flow_mcp_editor_adds_and_keeps_servers() {
    use sendito_ui::overlay::Overlay;
    let s = servers();
    let mut h = harness(&s, 120, 40);
    common::write(
        h.dir.path(),
        ".sendito/config.toml",
        "[collection]\nname = \"demo\"\n\n\
         [mcp.servers.remote]\nurl = \"https://example.test/mcp\"\nverify_tls = false\n\
         [mcp.servers.remote.headers]\nx-key = \"{{token}}\"\n\
         [mcp.servers.remote.auth]\ntype = \"bearer\"\ntoken = \"{{token}}\"\n\n\
         [mcp.servers.files]\ncommand = \"npx\"\nargs = [\"-y\", \"@mcp/server-fs\", \".\"]\n",
    );
    // Re-read the config the file now holds.
    h.app.project = sendito_ui::runner::Project::open_with(
        h.dir.path(),
        sendito_ui::runner::default_keychain(),
        None,
    );

    h.key("ctrl+w S");
    assert!(matches!(h.app.overlay, Some(Overlay::Mcp(_))));
    let sc = h.screen();
    assert!(sc.contains("MCP servers"), "{sc}");
    assert!(sc.contains("remote") && sc.contains("files"), "{sc}");
    // stdio rows read back as the command line they start.
    assert!(sc.contains("npx -y @mcp/server-fs ."), "{sc}");

    // Walk to the trailing row and add an HTTP server.
    for _ in 0..10 {
        let at_add = match &h.app.overlay {
            Some(Overlay::Mcp(m)) => m.table.sel_row >= m.table.rows.len(),
            _ => false,
        };
        if at_add {
            break;
        }
        h.key("down");
    }
    h.type_text("ledger");
    h.key("tab");
    assert_eq!(
        h.app.overlay.as_ref().map(|o| match o {
            Overlay::Mcp(m) => m.table.rows.last().map(|r| r.cells[1].choice()),
            _ => None,
        }),
        Some(Some(0)),
        "a new row starts as http"
    );
    h.key("tab");
    h.type_text("http://127.0.0.1:9/mcp");
    // Ctrl+S writes the table back.
    h.key("ctrl+s");
    let text = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert!(text.contains("[mcp.servers.ledger]"), "{text}");
    assert!(text.contains("http://127.0.0.1:9/mcp"), "{text}");
    // The untouched remote entry kept its verification, headers and auth.
    assert!(text.contains("verify_tls = false"), "{text}");
    assert!(text.contains("x-key"), "{text}");
    assert!(text.contains("type = \"bearer\""), "{text}");
    // So did the stdio entry's split args.
    assert!(text.contains("\"@mcp/server-fs\""), "{text}");
    assert_eq!(h.app.project.config.mcp.servers.len(), 3);
}

/// The editor shows and writes only the project file's servers: a server
/// from the global config (which may hold a literal token) never reaches the
/// versioned project file, and it stays usable after a save.
#[test]
fn flow_mcp_editor_never_copies_global_servers() {
    use sendito_ui::overlay::Overlay;
    let s = servers();
    let mut h = harness(&s, 120, 40);
    let global = h.dir.path().join("global.toml");
    std::fs::write(
        &global,
        "[mcp.servers.github]\ncommand = \"gh-mcp\"\nenv = { GITHUB_TOKEN = \"ghp_literal\" }\n",
    )
    .must();
    common::write(
        h.dir.path(),
        ".sendito/config.toml",
        "[collection]\nname = \"demo\"\n\n[mcp.servers.files]\ncommand = \"npx\"\n",
    );
    h.app.project = sendito_ui::runner::Project::open_with(
        h.dir.path(),
        sendito_ui::runner::default_keychain(),
        Some(global),
    );
    assert_eq!(h.app.project.config.mcp.servers.len(), 2);
    h.key("ctrl+w S");
    let sc = h.screen();
    assert!(sc.contains("files") && !sc.contains("github"), "{sc}");
    h.key("ctrl+s");
    assert!(!matches!(h.app.overlay, Some(Overlay::Mcp(ref m)) if m.error.is_some()));
    let text = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert!(
        !text.contains("github") && !text.contains("ghp_literal"),
        "{text}"
    );
    assert_eq!(h.app.project.config.mcp.servers.len(), 2);
}

/// `Esc` on a dirty editor asks before discarding; Discard drops the edit and
/// leaves the file alone.
#[test]
fn flow_mcp_editor_asks_before_discarding() {
    use sendito_ui::overlay::{ConfirmAction, Overlay};
    let s = servers();
    let mut h = harness(&s, 120, 40);
    h.key("ctrl+w S");
    h.type_text("only");
    h.key("esc");
    assert!(matches!(
        h.app.overlay,
        Some(Overlay::Confirm(ref c)) if matches!(c.action, ConfirmAction::McpClose)
    ));
    assert!(h.screen().contains("Unsaved MCP server changes"));
    // Cancel keeps the editor and the typed row.
    h.key("right right enter");
    assert!(matches!(h.app.overlay, Some(Overlay::Mcp(_))));
    // Discard closes without writing.
    h.key("esc right enter");
    assert!(h.app.overlay.is_none());
    let text = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert!(!text.contains("[mcp.servers"), "{text}");
}

/// `:mcpadd` asks for the URL (or command) and writes the server at once.
#[test]
fn flow_mcp_add_command_writes_the_project_config() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    h.app.run_command("mcpadd ledger");
    h.type_text("http://127.0.0.1:9/mcp");
    h.key("enter");
    assert!(h.app.overlay.is_none(), "the prompt closes on Enter");
    let text = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert!(text.contains("[mcp.servers.ledger]"), "{text}");
    assert!(text.contains("http://127.0.0.1:9/mcp"), "{text}");
    assert_eq!(
        h.app
            .project
            .config
            .mcp
            .servers
            .get("ledger")
            .map(|s| match s {
                sendito_store::config::McpServer::Http(h) => h.url.as_str(),
                _ => "",
            }),
        Some("http://127.0.0.1:9/mcp")
    );

    // The name is required; without it the command says how to use it.
    h.app.run_command("mcpadd");
    assert!(h.app.overlay.is_none());
    assert!(h.screen().contains("usage: :mcpadd"), "{}", h.screen());
}

/// Clicks inside a dialog act inside it: its own frame and text never dismiss
/// it, and the fields take the caret (the dialog is modal, not a backdrop).
#[test]
fn flow_dialogs_ignore_clicks_on_their_own_chrome() {
    use sendito_ui::app::{Hit, OverlayHit};
    let mut h = harness(&servers(), 120, 40);
    for cmd in ["settings", "help", "import"] {
        h.app.run_command(cmd);
        h.draw();
        let r = h
            .app
            .hits
            .iter()
            .rev()
            .find(|(_, hit)| matches!(hit, Hit::Overlay(OverlayHit::Dialog)))
            .map(|(r, _)| *r)
            .must();
        h.click(r.x + 2, r.y);
        assert!(
            h.app.overlay.is_some(),
            "{cmd}: clicking its title must not close it"
        );
        h.key("esc");
    }
    // A report's text is inert too; only its buttons act.
    h.key("ctrl+w h");
    h.app.tree.set_filter("health");
    h.key("enter");
    h.app.tree.set_filter("");
    h.app.run_command("curl");
    h.draw();
    let (x, y) = h.find("curl ").must();
    h.click(x + 2, y);
    assert!(matches!(
        h.app.overlay,
        Some(sendito_ui::overlay::Overlay::Report(_))
    ));
}

/// The palette's search field takes the caret instead of closing the palette.
#[test]
fn flow_palette_input_takes_the_mouse() {
    use sendito_ui::app::{Hit, OverlayHit};
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    h.app.open_palette("health");
    h.draw();
    let input = h
        .app
        .hits
        .iter()
        .rev()
        .find(|(_, hit)| matches!(hit, Hit::Overlay(OverlayHit::Input)))
        .map(|(r, _)| *r)
        .must();
    h.click(input.x + 2, input.y);
    assert!(matches!(h.app.overlay, Some(Overlay::Palette(_))));
    // Clicking a row still runs it.
    let (x, y) = h.find("health").must();
    h.click(x + 2, y);
    assert!(
        h.screen().contains("health"),
        "the row opened the request: {}",
        h.screen()
    );
}

/// An open prompt option list is clickable; choosing an entry sets the value
/// and closes the list (keyboard Space/Enter stay the same).
#[test]
fn flow_prompt_dropdown_option_takes_the_mouse() {
    use sendito_ui::app::{Hit, OverlayHit};
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("mcpadd demo");
    h.key("tab space");
    h.draw();
    let opt = h
        .app
        .hits
        .iter()
        .rev()
        .find(|(_, hit)| matches!(hit, Hit::Overlay(OverlayHit::PromptOption(1))))
        .map(|(r, _)| *r)
        .must();
    h.click(opt.x + 1, opt.y);
    match &h.app.overlay {
        Some(Overlay::Prompt(p)) => {
            assert_eq!(p.choices[0].2, 1, "stdio was chosen");
            assert!(p.open.is_none(), "the list closes");
        }
        _ => panic!("clicking an option must keep the prompt open"),
    }
}

/// A settings row click selects it; the value chip changes it, so a click on
/// the row no longer rewrites the config.
#[test]
fn flow_settings_click_selects_and_the_value_changes() {
    use sendito_ui::app::{Hit, OverlayHit};
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("settings");
    h.draw();
    let theme = |h: &Harness| h.app.project.config.ui.theme.clone();
    let before = theme(&h);
    let row = h
        .app
        .hits
        .iter()
        .rev()
        .find(|(_, hit)| matches!(hit, Hit::Overlay(OverlayHit::Item(1))))
        .map(|(r, _)| *r)
        .must();
    h.click(row.x + 2, row.y);
    assert_eq!(theme(&h), before, "a click only selects the row");
    let value = h
        .app
        .hits
        .iter()
        .rev()
        .find(|(_, hit)| matches!(hit, Hit::Overlay(OverlayHit::Value(1))))
        .map(|(r, _)| *r)
        .must();
    h.click(value.x + 1, value.y);
    assert_ne!(theme(&h), before, "the value chip changes the setting");
}

/// Clicking the trailing "add" row starts it, like typing in it does.
#[test]
fn flow_table_add_row_takes_the_mouse() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("envs");
    h.draw();
    let before = match &h.app.overlay {
        Some(Overlay::Env(e)) => e.table.rows.len(),
        _ => panic!("the env editor did not open"),
    };
    let (x, y) = h.find("add variable").must();
    h.click(x + 1, y);
    h.type_text("token");
    h.draw();
    match &h.app.overlay {
        Some(Overlay::Env(e)) => {
            assert_eq!(e.table.rows.len(), before + 1);
            assert_eq!(e.table.rows.last().must().cells[0].text(), "token");
            assert!(e.dirty);
        }
        _ => panic!("the env editor closed"),
    }
}

/// Ctrl+N in the MCP editor adds a row, and the button does the same.
#[test]
fn flow_mcp_editor_adds_rows() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("mcpservers edit");
    h.draw();
    h.key("ctrl+n");
    h.type_text("echo");
    h.draw();
    match &h.app.overlay {
        Some(Overlay::Mcp(m)) => {
            assert_eq!(m.table.rows.len(), 1, "Ctrl+N created the row");
            assert_eq!(m.table.rows[0].cells[0].text(), "echo");
        }
        _ => panic!("the MCP editor closed"),
    }
    let (x, y) = h.find("Add row (Ctrl+N)").must();
    h.click(x + 1, y);
    h.draw();
    match &h.app.overlay {
        Some(Overlay::Mcp(m)) => assert_eq!(m.table.rows.len(), 2, "the button added a row"),
        _ => panic!("the MCP editor closed"),
    }
}

/// Cancelling keeps the editor: Esc on the save prompt returns to the env
/// editor, and cancelling Ctrl+N's prompt does too.
#[test]
fn flow_cancelling_a_prompt_keeps_the_editor() {
    use sendito_ui::overlay::{ConfirmAction, Overlay};
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("envs");
    h.type_text("edit");
    h.key("esc");
    assert!(matches!(
        h.app.overlay,
        Some(Overlay::Confirm(ref c)) if matches!(c.action, ConfirmAction::EnvClose)
    ));
    h.key("esc");
    assert!(
        matches!(h.app.overlay, Some(Overlay::Env(_))),
        "Esc on the prompt means cancel, not close"
    );
    // Ctrl+N with unsaved edits is refused: creating an environment loads
    // it into the editor, which would drop them.
    h.key("ctrl+n");
    match &h.app.overlay {
        Some(Overlay::Env(e)) => assert!(e.dirty && e.error.is_some()),
        _ => panic!("Ctrl+N left the dirty editor"),
    }
    assert!(h.app.parked_env.is_none());
}

/// On a clean editor, Ctrl+N parks it, and cancelling the name prompt (by
/// Esc) goes back to it.
#[test]
fn flow_new_env_prompt_cancel_returns_to_the_editor() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("envs");
    h.key("ctrl+n");
    assert!(matches!(h.app.overlay, Some(Overlay::Prompt(_))));
    h.key("esc");
    assert!(matches!(h.app.overlay, Some(Overlay::Env(_))));
    assert!(h.app.parked_env.is_none());
}

/// Ctrl+D on the trailing row (nothing to delete) is not a change: Esc then
/// closes without asking to save.
#[test]
fn flow_ctrl_d_on_the_add_row_is_not_a_change() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("envs");
    h.draw();
    for _ in 0..30 {
        h.key("down");
    }
    h.key("ctrl+d");
    h.draw();
    assert!(matches!(&h.app.overlay, Some(Overlay::Env(e)) if !e.dirty));
    h.key("esc");
    assert!(h.app.overlay.is_none(), "a clean editor closes at once");
}

/// Copying from a report flips the button to "Copy again", by key or click.
#[test]
fn flow_report_copy_button_flips_the_label() {
    use sendito_ui::app::{Hit, OverlayHit};
    let mut h = harness(&servers(), 120, 40);
    h.app.show_report("Report", "one\ntwo\nthree");
    h.draw();
    assert!(!h.screen().contains("Copy again"));
    h.key("y");
    assert!(h.screen().contains("Copy again"), "{}", h.screen());
    // A fresh report: the button does the same.
    h.app.show_report("Report", "one\ntwo\nthree");
    h.draw();
    let btn = h
        .app
        .hits
        .iter()
        .rev()
        .find(|(_, hit)| matches!(hit, Hit::Overlay(OverlayHit::Button(0))))
        .map(|(r, _)| *r)
        .must();
    h.click(btn.x + 1, btn.y);
    assert!(h.screen().contains("Copy again"), "{}", h.screen());
}

/// The wheel acts on the dialog only when the pointer is over it, and it
/// scrolls the settings and the env table too.
#[test]
fn flow_wheel_over_the_dialog() {
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("settings");
    h.draw();
    let sel = |h: &Harness| match &h.app.overlay {
        Some(sendito_ui::overlay::Overlay::Settings { sel }) => *sel,
        _ => usize::MAX,
    };
    // Outside the dialog (sidebar) does nothing.
    h.scroll(2, 20, true);
    assert_eq!(sel(&h), 1);
    // Over the dialog moves the selection.
    let (x, y) = h.find("Appearance").must();
    h.scroll(x, y, true);
    assert_eq!(sel(&h), 2, "the wheel moved the settings selection");

    // The env table follows the wheel as well (keyboard arrows already did).
    let mut h = harness(&servers(), 120, 40);
    h.app.set_env(Some("local".into()));
    h.app.run_command("envs");
    h.draw();
    let (x, y) = h.find("Variable").must();
    h.scroll(x + 2, y + 1, true);
    match &h.app.overlay {
        Some(sendito_ui::overlay::Overlay::Env(e)) => assert!(e.table.sel_row > 0),
        _ => panic!("the env editor closed"),
    }
}

/// Every dialog fits the smallest supported terminal instead of panicking.
#[test]
fn flow_dialogs_fit_a_small_terminal() {
    for (w, hgt) in [(100u16, 14u16), (100, 8), (40, 8)] {
        for cmd in [
            "settings",
            "help",
            "import",
            "envs",
            "mcpservers edit",
            "mcpadd probe",
        ] {
            let mut h = Harness::with(w, hgt, |d| {
                common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
            });
            h.app.run_command(cmd);
            h.draw();
        }
        let mut h = Harness::with(w, hgt, |d| {
            common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        });
        h.app.show_report("Report", &"line\n".repeat(40));
        h.draw();
        let mut h = Harness::with(w, hgt, |d| {
            common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        });
        h.app.open_palette("");
        h.draw();
    }
    // The picker needs a server list.
    for hgt in [8u16, 10, 12] {
        let mut h = Harness::with(120, hgt, |d| {
            common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
            common::write(
                d,
                ".sendito/config.toml",
                "[collection]\nname = \"demo\"\n[mcp.servers.echo]\nurl = \"http://127.0.0.1:9/mcp\"\n",
            );
        });
        h.app.run_command("newmcp");
        h.app.run_command("mcpservers");
        h.draw();
    }
}

/// A prompt with typed text asks before a click outside drops it; an untouched
/// prompt still closes on the same click, and "Keep editing" returns to it.
#[test]
fn flow_dirty_prompt_outside_click_asks_before_discarding() {
    use sendito_ui::overlay::{ConfirmAction, Overlay};
    let mut h = harness(&servers(), 120, 40);
    h.key("ctrl+w h");
    h.key("n");
    let dialog = |h: &Harness| match &h.app.overlay {
        Some(Overlay::Prompt(p)) => Some(p.input.text()),
        _ => None,
    };

    // Untouched: the click closes it.
    assert_eq!(dialog(&h), Some(String::new()));
    h.click(1, 38);
    assert!(h.app.overlay.is_none());

    // Typed text: the click parks the prompt behind a discard confirmation.
    h.key("n");
    h.type_text("half-typed");
    h.draw();
    h.click(1, 38);
    assert!(matches!(
        h.app.overlay,
        Some(Overlay::Confirm(ref c)) if matches!(c.action, ConfirmAction::DialogClose)
    ));
    assert!(h.screen().contains("Unsaved text"));

    // Keep editing brings the prompt back with the text intact.
    h.key("enter");
    assert_eq!(dialog(&h), Some("half-typed".to_string()));

    // Discard drops it for real.
    h.click(1, 38);
    h.key("left enter");
    assert!(h.app.overlay.is_none());
}

/// The import dialog follows the same rule: typed text is not dropped silently.
#[test]
fn flow_dirty_import_outside_click_asks_before_discarding() {
    use sendito_ui::overlay::{ConfirmAction, Overlay};
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("import");
    assert!(matches!(h.app.overlay, Some(Overlay::Import(_))));
    h.click(1, 38);
    assert!(
        h.app.overlay.is_none(),
        "an untouched dialog closes at once"
    );

    h.app.run_command("import");
    h.paste("curl https://example.test/users");
    h.draw();
    h.click(1, 38);
    assert!(matches!(
        h.app.overlay,
        Some(Overlay::Confirm(ref c)) if matches!(c.action, ConfirmAction::DialogClose)
    ));
    h.key("enter");
    assert!(matches!(
        &h.app.overlay,
        Some(Overlay::Import(i)) if i.input.text() == "curl https://example.test/users"
    ));
    h.click(1, 38);
    h.key("left enter");
    assert!(h.app.overlay.is_none());
}

#[test]
fn ctrl_w_menu_click_runs_the_entry() {
    let mut h = Harness::new(120, 40);
    let tabs = h.app.tabs.len();
    h.key("ctrl+w");
    let (x, y) = h.find("new gRPC").must();
    h.click(x, y);
    assert!(h.app.pending.is_empty());
    assert_eq!(h.app.tabs.len(), tabs + 1);
    assert!(h.screen().contains("GRPC"));
    // A click outside the menu only closes it.
    h.key("ctrl+w");
    assert!(h.find("new gRPC").is_some());
    h.click(1, 1);
    assert!(h.app.pending.is_empty());
    assert!(h.find("new gRPC").is_none());
}

/// The URL card's one button: Send while idle, Cancel while a call runs. A
/// click or Enter on it cancels the running call.
#[test]
fn flow_send_button_turns_into_cancel() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    open(&mut h, "health");
    h.app.tabs[h.app.active]
        .ed
        .url
        .set_text("{{base_url}}/delay/5000");
    assert!(h.screen().contains("Send"));
    h.key("f5");
    assert!(h.app.is_busy());
    let (x, y) = h.find("Cancel").must();
    assert!(!h.screen().contains("Send "));
    h.click(x, y);
    h.pump();
    assert!(!h.app.is_busy());
    assert!(h.screen().contains("Send"));

    // Enter on the focused button cancels the same way.
    h.key("f5");
    assert!(h.app.is_busy());
    h.app.focus = sendito_ui::app::Focus::Bar(2);
    h.key("enter");
    h.pump();
    assert!(!h.app.is_busy());
}

/// Without focus, the endpoint shows what its variables resolve to; a secret
/// stays a placeholder and an unknown name is reported.
#[test]
fn flow_endpoint_preview() {
    let s = servers();
    let mut h = harness(&s, 160, 40);
    open(&mut h, "health");
    h.app.focus = sendito_ui::app::Focus::ReqTabs;
    let url = h.app.tabs[h.app.active].ed.url.text();
    let shown = url.replace("{{base_url}}", &s.http.url());
    let sc = h.screen();
    assert!(sc.contains(&format!("→ {shown}")), "{sc}");

    h.app.tabs[h.app.active]
        .ed
        .url
        .set_text("{{base_url}}/{{token}}");
    let sc = h.screen();
    assert!(sc.contains("/{{token}}") && !sc.contains("s3cr3t"), "{sc}");

    h.app.tabs[h.app.active].ed.url.set_text("{{nope}}/x");
    assert!(h.screen().contains("→ undefined: nope"));

    // With focus in the input, the preview goes away.
    h.app.focus = sendito_ui::app::Focus::Bar(1);
    assert!(!h.screen().contains("→ undefined"));
}

/// A low screen keeps the URL strip on one row, without the card's border.
#[test]
fn flow_url_card_on_low_screen() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    open(&mut h, "health");
    assert!(h.screen().contains("╭ HTTP "));
    let mut h = harness(&s, 120, 16);
    open(&mut h, "health");
    let sc = h.screen();
    assert!(!sc.contains("╭ HTTP ") && sc.contains("Send"), "{sc}");
}

/// The status line text.
fn status(h: &Harness) -> String {
    h.app
        .status
        .as_ref()
        .map(|s| s.0.clone())
        .unwrap_or_default()
}

/// Selects the settings row labelled `label` (the dialog must be open).
fn select_setting(h: &mut Harness, label: &str) {
    let i = sendito_ui::overlay::SETTINGS
        .iter()
        .position(|(_, l)| *l == label)
        .must();
    if let Some(sendito_ui::overlay::Overlay::Settings { sel }) = &mut h.app.overlay {
        *sel = i;
    }
}

/// A number setting is typed in a dialog: a bad value keeps the dialog open
/// with the reason, a good one applies at once and returns to the settings.
/// Saved to project, it lands in the project config.
#[test]
fn flow_settings_typed_values_and_scope() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("settings");
    // The scope row comes first: switch it to the project file.
    select_setting(&mut h, "Save changes to");
    h.key("right");
    h.draw();
    assert!(h.screen().contains("project"), "{}", h.screen());

    select_setting(&mut h, "HTTP request timeout");
    h.key("enter");
    assert!(matches!(h.app.overlay, Some(Overlay::Prompt(_))));
    assert!(h.screen().contains("30000"), "{}", h.screen());
    h.key("ctrl+a");
    h.type_text("zero");
    h.key("enter");
    h.draw();
    assert!(
        h.screen().contains("type a whole number (1 or more)"),
        "{}",
        h.screen()
    );
    h.key("ctrl+a");
    h.type_text("1500");
    h.key("enter");
    assert!(matches!(h.app.overlay, Some(Overlay::Settings { .. })));
    assert_eq!(h.app.project.config.http.timeout_ms, 1500);
    let file = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert!(file.contains("[http]\ntimeout_ms = 1500"), "{file}");
    assert!(file.contains("name = \"demo\""), "{file}");

    // A text setting checks its value; empty removes the key.
    select_setting(&mut h, "Proxy");
    h.key("enter");
    h.type_text("ftp://x");
    h.key("enter");
    h.draw();
    assert!(
        h.screen().contains("invalid [http] proxy"),
        "{}",
        h.screen()
    );
    h.key("ctrl+a");
    h.type_text("env");
    h.key("enter");
    assert_eq!(h.app.project.config.http.proxy.as_deref(), Some("env"));
    select_setting(&mut h, "Proxy");
    h.key("enter");
    h.key("ctrl+a");
    h.key("backspace");
    h.key("enter");
    assert_eq!(h.app.project.config.http.proxy, None);
    let file = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert!(!file.contains("proxy"), "{file}");

    // Esc in the value dialog returns to the settings with nothing changed.
    select_setting(&mut h, "Largest response body");
    h.key("enter");
    h.type_text("9");
    h.key("esc");
    assert!(matches!(h.app.overlay, Some(Overlay::Settings { .. })));
    assert_eq!(h.app.project.config.http.max_body_mb, 100);

    // Saved to the global file while the project file sets the same key:
    // the status says the project value wins.
    select_setting(&mut h, "Save changes to");
    h.key("right");
    select_setting(&mut h, "HTTP request timeout");
    h.key("enter");
    h.key("ctrl+a");
    h.type_text("2000");
    h.key("enter");
    assert!(
        status(&h).contains("project config also sets"),
        "{}",
        status(&h)
    );
    let global = std::fs::read_to_string(h.dir.path().join(".config/sendito/config.toml")).must();
    assert!(global.contains("timeout_ms = 2000"), "{global}");
}

/// A TLS path must name a file, and a client certificate without its key is
/// reported; the collection name change warns about keychain secrets.
#[test]
fn flow_settings_tls_and_collection_name() {
    let mut h = harness(&servers(), 120, 40);
    common::write(h.dir.path(), "certs/client.pem", "x");
    h.app.run_command("settings");
    select_setting(&mut h, "Client certificate (PEM)");
    h.key("enter");
    h.type_text("certs/none.pem");
    h.key("enter");
    h.draw();
    assert!(h.screen().contains("no file at"), "{}", h.screen());
    h.key("ctrl+a");
    h.type_text("certs/client.pem");
    h.key("enter");
    assert_eq!(
        h.app.project.config.http.tls.client_cert.as_deref(),
        Some("certs/client.pem")
    );
    assert!(h.app.project.tls.client_cert.is_some());
    assert!(status(&h).contains("client_key"), "{}", status(&h));

    select_setting(&mut h, "Collection name");
    h.key("enter");
    h.key("ctrl+a");
    h.type_text("renamed");
    h.key("enter");
    assert_eq!(h.app.project.name, "renamed");
    assert_eq!(h.app.project.envs.collection_name, "renamed");
    assert!(status(&h).contains("keychain"), "{}", status(&h));
    let file = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert!(file.contains("name = \"renamed\""), "{file}");
}

/// Opens the settings, sets the scope to the project file and closes them.
fn save_to_project(h: &mut Harness) {
    h.app.config_scope = sendito_ui::overlay::ConfigScope::Project;
}

fn project_file(h: &Harness) -> String {
    std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must()
}

/// Ctrl+O on a server opens its details: the OAuth overrides and headers of
/// an http server, the env of a stdio one; Ctrl+S writes them and Esc goes
/// back to the servers table.
#[test]
fn flow_mcp_server_details() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    common::write(
        h.dir.path(),
        ".sendito/config.toml",
        "[collection]\nname = \"demo\"\n\n\
         [mcp.servers.files]\ncommand = \"npx\"\n\n\
         [mcp.servers.remote]\nurl = \"https://example.test/mcp\"\n\
         [mcp.servers.remote.auth]\ntype = \"oauth\"\n",
    );
    h.app.project = sendito_ui::runner::Project::open_with(
        h.dir.path(),
        sendito_ui::runner::default_keychain(),
        None,
    );
    h.key("ctrl+w S");
    // Rows are sorted by name: files, remote.
    h.key("down");
    h.key("ctrl+o");
    assert!(
        matches!(h.app.overlay, Some(Overlay::Cfg(_))),
        "{}",
        h.screen()
    );
    let sc = h.screen();
    assert!(sc.contains("MCP server remote"), "{sc}");
    assert!(
        sc.contains("verify_tls") && sc.contains("client_id"),
        "{sc}"
    );
    // The client_id row: type its value.
    h.key("down");
    h.key("tab");
    h.type_text("my-app");
    // A header on the second tab.
    h.key("ctrl+pagedown");
    h.type_text("x-api-key");
    h.key("tab");
    h.type_text("plain");
    h.key("ctrl+s");
    let file = project_file(&h);
    assert!(file.contains("client_id = \"my-app\""), "{file}");
    assert!(file.contains("x-api-key = \"plain\""), "{file}");
    assert!(status(&h).contains("plain text"), "{}", status(&h));
    h.key("esc");
    assert!(matches!(h.app.overlay, Some(Overlay::Mcp(_))));

    // The stdio server has an env table.
    h.key("up");
    h.key("ctrl+o");
    assert!(h.screen().contains("Environment"), "{}", h.screen());
    h.type_text("DEBUG");
    h.key("tab");
    h.type_text("1");
    h.key("ctrl+s");
    assert!(
        project_file(&h).contains("DEBUG = \"1\""),
        "{}",
        project_file(&h)
    );
    // OAuth fields need auth = oauth: none of them on a stdio server, and a
    // dirty servers table must be saved first.
    h.key("esc");
    h.type_text("x");
    h.key("ctrl+o");
    assert!(matches!(h.app.overlay, Some(Overlay::Mcp(_))));
    assert!(h.screen().contains("save (Ctrl+S)"), "{}", h.screen());
}

/// The OAuth editor writes [oauth.<name>] to the file the settings choose;
/// missing fields are reported, a plain secret is saved with a warning.
#[test]
fn flow_oauth_editor() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    save_to_project(&mut h);
    h.app.run_command("oauth");
    assert!(matches!(h.app.overlay, Some(Overlay::Cfg(_))));
    h.type_text("gh");
    h.key("tab");
    h.type_text("https://github.com/login/oauth/authorize");
    h.key("ctrl+s");
    assert!(h.screen().contains("gh: set a token URL"), "{}", h.screen());
    h.key("tab");
    h.type_text("https://github.com/login/oauth/access_token");
    h.key("tab");
    h.type_text("abc");
    h.key("tab");
    h.type_text("{{gh_secret}}");
    h.key("tab");
    h.type_text("repo read:user");
    h.key("ctrl+s");
    assert!(status(&h).starts_with("saved to"), "{}", status(&h));
    let id = &h.app.project.config.oauth["gh"];
    assert_eq!(id.client_id, "abc");
    assert_eq!(id.client_secret.as_deref(), Some("{{gh_secret}}"));
    assert_eq!(id.scopes, vec!["repo", "read:user"]);
    assert!(
        project_file(&h).contains("[oauth.gh]"),
        "{}",
        project_file(&h)
    );
}

/// The keymap editor replaces an action's keys at once; an unknown action
/// or key is refused.
#[test]
fn flow_keymap_editor() {
    use sendito_ui::keymap::{Action, Chord, Ctx, Lookup};
    let mut h = harness(&servers(), 120, 40);
    save_to_project(&mut h);
    h.app.run_command("settings");
    select_setting(&mut h, "Key bindings");
    h.key("enter");
    assert!(h.screen().contains("Key bindings"), "{}", h.screen());
    h.type_text("sav");
    assert!(h.screen().contains("Actions: save"), "{}", h.screen());
    h.type_text("e");
    h.key("tab");
    h.type_text("ctrl+nope");
    h.key("ctrl+s");
    assert!(h.screen().contains("save:"), "{}", h.screen());
    h.key("ctrl+a");
    h.type_text("ctrl+g, f9");
    h.key("ctrl+s");
    let g = Chord::parse("ctrl+g").must();
    assert_eq!(
        h.app.keymap.lookup(&[Ctx::Global], false, &[g]),
        Lookup::Action(Action::Save)
    );
    let file = project_file(&h);
    assert!(file.contains("[keymap.all]"), "{file}");
    assert!(file.contains("save = [\"ctrl+g\", \"f9\"]"), "{file}");
}

/// The palette finds settings and commands without a ':' prefix.
#[test]
fn flow_palette_finds_settings_and_commands() {
    use sendito_ui::overlay::{Overlay, SETTINGS, SettingId};
    let mut h = harness(&servers(), 120, 40);
    h.key("ctrl+p");
    h.type_text("setting: vim mode");
    h.key("enter");
    match &h.app.overlay {
        Some(Overlay::Settings { sel }) => assert_eq!(SETTINGS[*sel].0, SettingId::Vim),
        _ => panic!("the settings dialog did not open"),
    }
    h.key("esc");
    // A command that needs an argument waits for it in the input.
    h.key("ctrl+p");
    h.type_text("saveas");
    h.key("enter");
    match &h.app.overlay {
        Some(Overlay::Palette(p)) => assert_eq!(p.input.text(), ":saveas "),
        _ => panic!("the palette closed"),
    }
    h.key("esc");
    // The typed text as written wins over scattered matches.
    h.key("ctrl+p");
    h.type_text("rescan");
    h.key("enter");
    assert!(h.screen().contains("collection read from disk again"));
}

#[test]
fn flow_theme_picker_applies_the_theme() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("theme");
    assert!(matches!(&h.app.overlay, Some(Overlay::Picker(p)) if p.title == "Theme"));
    h.type_text("daylight");
    h.key("enter");
    assert_eq!(h.app.project.config.ui.theme, "daylight");
}

#[test]
fn flow_auth_and_mcp_pickers() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    // Nothing to sign in to: the status says so.
    h.app.run_command("auth login");
    assert!(h.app.overlay.is_none());
    assert!(h.screen().contains("no OAuth identities"));
    h.app.project.config.oauth.insert(
        "service".into(),
        sendito_store::config::OAuthIdentity::default(),
    );
    h.app.run_command("auth logout");
    match &h.app.overlay {
        Some(Overlay::Picker(p)) => {
            assert_eq!(p.items, vec!["service".to_string()]);
            assert_eq!(p.details, vec!["HTTP identity".to_string()]);
        }
        _ => panic!("the sign-out picker did not open"),
    }
    h.key("esc");
    h.app.run_command("mcpadd ledger");
    h.type_text("https://mcp.example.test/mcp");
    h.key("enter");
    h.app.run_command("mcpedit");
    assert!(
        matches!(&h.app.overlay, Some(Overlay::Picker(p)) if p.items.contains(&"ledger".to_string()))
    );
    h.type_text("ledger");
    h.key("enter");
    assert!(
        matches!(&h.app.overlay, Some(Overlay::Cfg(_))),
        "{}",
        h.screen()
    );
}

#[test]
fn flow_request_pickers_follow_the_request_kind() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("newgrpc");
    h.app.run_command("method");
    assert!(h.app.overlay.is_none());
    assert!(
        h.screen()
            .contains("the method is not available for gRPC requests")
    );
    h.app.run_command("new");
    h.app.run_command("method");
    assert!(matches!(&h.app.overlay, Some(Overlay::Picker(p)) if p.title == "HTTP method"));
    h.key("esc");
    h.app.run_command("bodytype");
    assert!(matches!(&h.app.overlay, Some(Overlay::Picker(_))));
    h.key("esc");
    h.app.run_command("newenv");
    h.type_text("staging");
    h.key("enter");
    assert!(h.app.project.envs.list().contains(&"staging".to_string()));
}
