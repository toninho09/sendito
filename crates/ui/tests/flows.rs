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
    save_to_project(&mut h);
    h.key("ctrl+w S");
    assert!(matches!(h.app.overlay, Some(Overlay::Records(_))));
    // The form of a shows its auth and the OAuth section.
    h.key("enter");
    let sc = h.screen();
    assert!(sc.contains("Auth") && sc.contains(" oauth "), "{sc}");
    assert!(sc.contains("Scopes") && sc.contains("read"), "{sc}");
    // Server b: Auth to bearer (name, kind, URL, auth) and a token variable.
    h.key("esc down enter");
    h.key("down down down right");
    let sc = h.screen();
    assert!(sc.contains("Token") && !sc.contains("Scopes"), "{sc}");
    h.key("down");
    h.type_text("{{tok}}");
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

/// `f` marks a request as a favorite: it shows in the Favorites section and
/// on the start screen, follows a rename, and survives a restart.
#[test]
fn flow_favorites() {
    let mut h = Harness::new(100, 30);
    h.key("ctrl+w h");
    h.app.tree.select_rel("health.toml");
    h.key("f");
    assert_eq!(h.app.tree.favorites, vec!["health.toml"]);
    let sc = h.screen();
    assert!(sc.contains("★ Favorites"), "{sc}");
    assert!(sc.contains("added to favorites: health"), "{sc}");
    // The Favorites section opens the request with Enter.
    let fav = h
        .app
        .tree
        .rows
        .iter()
        .position(|r| r.fav && !r.is_section());
    h.app.tree.selected = fav.must();
    h.key("enter");
    assert_eq!(
        h.app.tab().and_then(|t| t.path.as_deref()),
        Some("health.toml")
    );
    // A folder cannot be a favorite.
    h.key("ctrl+w h");
    h.app.tree.select_rel("Users");
    h.key("f");
    assert!(h.screen().contains("only requests can be favorites"));
    // A rename keeps the favorite.
    h.app.tree.select_rel("health.toml");
    h.key("r");
    h.key("ctrl+a");
    h.type_text("status");
    h.key("enter");
    assert_eq!(h.app.tree.favorites, vec!["status.toml"]);
    // Ctrl+W F toggles the active tab's request.
    h.key("ctrl+w l");
    h.key("ctrl+w F");
    assert!(h.app.tree.favorites.is_empty());
    h.key("ctrl+w F");
    assert_eq!(h.app.tree.favorites, vec!["status.toml"]);
    // The favorites survive a restart and show on the start screen.
    h.key("ctrl+w c");
    h.reopen();
    assert_eq!(h.app.tree.favorites, vec!["status.toml"]);
    let sc = h.screen();
    assert!(sc.contains("Favorites") && sc.contains("Recent"), "{sc}");
    // A delete removes the favorite.
    h.key("ctrl+w h");
    h.app.tree.select_rel("status.toml");
    h.key("delete");
    h.key("left enter");
    assert!(h.app.tree.favorites.is_empty());
    assert!(!h.screen().contains("★ Favorites"));
}

/// A tab that was never saved cannot be a favorite.
#[test]
fn flow_favorite_needs_a_saved_request() {
    let mut h = Harness::new(100, 30);
    h.key("ctrl+n");
    h.key("ctrl+w F");
    assert!(h.screen().contains("save the request first"));
    assert!(h.app.tree.favorites.is_empty());
}

/// Ctrl+W P pins a tab: it moves before the others, shows the pin icon, and
/// stays pinned after a restart. Closing it needs no confirmation.
#[test]
fn flow_pin_tab() {
    let mut h = Harness::new(120, 30);
    open(&mut h, "health");
    open(&mut h, "get order");
    assert_eq!(h.app.active, 1);
    h.key("ctrl+w P");
    let names: Vec<&str> = h.app.tabs.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["get order", "health"]);
    assert!(h.app.tabs[0].pinned && h.app.active == 0);
    assert!(h.screen().contains("◆ GET get order"));
    // A new tab opens after the pinned ones.
    open(&mut h, "list users");
    assert_eq!(h.app.tabs[0].name, "get order");
    h.reopen();
    assert!(h.app.tabs[0].pinned && h.app.tabs[0].name == "get order");
    assert!(h.app.tabs[1..].iter().all(|t| !t.pinned));
    // Unpinning moves the tab to the start of the others.
    h.app.active = 0;
    h.key("ctrl+w P");
    assert!(h.app.tabs.iter().all(|t| !t.pinned));
    h.key("ctrl+w P");
    h.key("ctrl+w c");
    assert!(h.app.tabs.iter().all(|t| t.name != "get order"));
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

/// While the tree scrolls, the folder of the top row stays pinned above the
/// list; a click on it selects the folder and scrolls the tree to it.
#[test]
fn flow_tree_pins_folders_while_scrolling() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    for i in 0..60 {
        common::write(
            h.dir.path(),
            &format!("Big/route {i:02}.toml"),
            "kind = \"http\"\nmethod = \"GET\"\nurl = \"http://x\"\n",
        );
    }
    h.app.rescan();
    h.app.tree.select_rel("Big/route 00.toml");
    h.draw();
    let tr = h.app.areas.tree;
    let top = |h: &mut Harness| -> String {
        h.screen()
            .lines()
            .nth(tr.y as usize)
            .unwrap_or_default()
            .to_string()
    };
    for _ in 0..10 {
        h.scroll(tr.x + 4, tr.y + 4, true);
    }
    assert!(
        top(&mut h).contains("Big/"),
        "folder not pinned\n{}",
        h.screen()
    );
    h.click(tr.x + 6, tr.y);
    assert_eq!(h.app.tree.selected_rel().as_deref(), Some("Big"));
    h.draw();
    assert_eq!(h.app.tree.rows[h.app.tree.scroll].rel, "Big");
    // Off: the top line is a request row.
    h.app.project.config.ui.sticky_folders = false;
    for _ in 0..10 {
        h.scroll(tr.x + 4, tr.y + 4, true);
    }
    assert!(!top(&mut h).contains("Big/"), "{}", h.screen());
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
            response: None,
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

/// The Scripts tab edits the Lua pre script; the send runs it and the
/// Messages tab shows its output.
#[test]
fn flow_lua_pre_script() {
    let s = servers();
    let mut h = harness(&s, 100, 30);
    open(&mut h, "health");
    // Scripts tab, the section choice, then section 4: the Lua pre script.
    h.key("alt+5");
    h.key("enter");
    h.key("right right right");
    assert!(h.screen().contains("Pre-request: Lua script (empty)"));
    h.key("down");
    h.type_text("request:set_header(\"X-Flow\", \"lua\") print(\"hi\")");
    h.key("ctrl+enter");
    h.wait_idle(10);
    let caps = s.http.take();
    let c = caps.last().must();
    assert_eq!(c.header("x-flow").as_deref(), Some("lua"));
    h.key("ctrl+w j alt+4");
    let sc = h.screen();
    assert!(sc.contains("Script output:"), "{sc}");
    assert!(sc.contains("  hi"), "{sc}");
}

/// The response tab bar: Left and Right switch the tab and Enter opens it,
/// before the viewer bindings see the keys; the hint row says so.
#[test]
fn flow_response_tab_bar_keys() {
    use sendito_ui::app::Focus;
    let s = servers();
    let mut h = harness(&s, 120, 40);
    open(&mut h, "create user");
    h.key("f5");
    h.wait_idle(10);
    h.app.focus = Focus::RespTabs;
    let labels: Vec<String> = h.app.hints().into_iter().map(|(_, l)| l).collect();
    assert!(labels.iter().any(|l| l == "switch tab"), "{labels:?}");
    assert!(!labels.iter().any(|l| l == "fold"), "{labels:?}");
    let tab = |h: &Harness| h.app.tab().must().resp_tab;
    h.key("right");
    assert_eq!(tab(&h), 1);
    h.key("left");
    assert_eq!(tab(&h), 0);
    h.key("enter");
    assert_eq!(h.app.focus, Focus::RespBody);
}

/// A note leaves the footer after its time; a warning stays until a new
/// message replaces it.
#[test]
fn flow_status_note_expires_warning_stays() {
    use sendito_ui::app::StatusKind;
    let s = servers();
    let mut h = harness(&s, 120, 40);
    let old = std::time::Instant::now()
        .checked_sub(std::time::Duration::from_secs(30))
        .must();
    h.app.status = Some(("check the url".into(), StatusKind::Warn, old));
    h.app.tick();
    assert!(h.app.status.is_some());
    h.app.status = Some(("saved".into(), StatusKind::Ok, old));
    h.app.tick();
    assert!(h.app.status.is_none());
}

/// `:debug FILE` turns the debug log on: an error status and a dialog that
/// stops a send go to the file. `:debug` turns it off. The log is global to
/// the process, so the test checks only its own lines.
#[test]
fn flow_debug_command_logs_errors() {
    use sendito_ui::app::StatusKind;
    let dir = tempfile::tempdir().must();
    let log = dir.path().join("debug.log");
    let s = servers();
    let mut h = harness(&s, 120, 40);
    h.app.run_command(&format!("debug {}", log.display()));
    let (msg, kind, _) = h.app.status.clone().must();
    assert_eq!(msg, format!("debug log on: {}", log.display()));
    assert_eq!(kind, StatusKind::Info);
    h.app.run_command("watch abc");
    h.app.run_command("env staging");
    open(&mut h, "get order");
    h.key("f5");
    assert!(h.screen().contains("Undefined variables"));
    h.key("esc");
    h.app.run_command("debug");
    let (msg, _, _) = h.app.status.clone().must();
    assert_eq!(msg, format!("debug log off: {}", log.display()));
    let text = std::fs::read_to_string(&log).must();
    assert!(text.contains(" ERROR usage: :watch [seconds]"), "{text}");
    assert!(
        text.contains(" WARN not sent: Undefined variables: {{order_id}} not defined"),
        "{text}"
    );
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
    // Ctrl+R shows the body without the filter while the text is typed;
    // Enter turns the filter on again.
    h.key("j");
    h.type_text(".method");
    h.key("ctrl+r");
    {
        let sc = h.screen();
        assert!(sc.contains("filter off"), "{sc}");
        assert!(sc.contains("\"target\""), "{sc}");
    }
    h.key("enter");
    {
        let sc = h.screen();
        assert!(!sc.contains("filter off"), "{sc}");
        assert!(sc.contains("\"POST\""), "{sc}");
        assert!(!sc.contains("\"target\""), "{sc}");
    }
    // Esc also ends the bypass and goes back to the kept filter.
    h.key("j");
    h.key("ctrl+r");
    assert!(h.screen().contains("\"target\""));
    h.key("esc");
    {
        let sc = h.screen();
        assert!(sc.contains("jq ▸ .method"), "{sc}");
        assert!(!sc.contains("\"target\""), "{sc}");
    }
    h.key("j");
    h.key("ctrl+a backspace enter");
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

/// A terminal paste into the query bar is text: the filter (or the search)
/// follows it, it goes in at the cursor and a newline folds to a space.
#[test]
fn flow_response_jq_paste() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    open(&mut h, "create user");
    h.key("f5");
    h.wait_idle(10);
    h.key("ctrl+w j");
    h.key("j");
    h.paste(".method");
    {
        let sc = h.screen();
        assert!(sc.contains("jq ▸ .method"), "{sc}");
        assert!(sc.contains("1 result"), "{sc}");
        assert!(sc.contains("\"POST\""), "{sc}");
    }
    // At the cursor: Ctrl+A selects the text, the paste replaces it. The
    // trailing newline folds to a space and the filter still runs.
    h.key("ctrl+a");
    h.paste(".method | ascii_downcase\n");
    {
        let sc = h.screen();
        assert!(sc.contains("jq ▸ .method | ascii_downcase"), "{sc}");
        assert!(sc.contains("1 result"), "{sc}");
        assert!(sc.contains("\"post\""), "{sc}");
    }
    // Esc: nothing was kept with Enter, so the bar closes.
    h.key("esc");
    assert!(!h.screen().contains("jq ▸"));
    // The find bar takes a paste too.
    h.key("/");
    h.paste("server-session-1");
    {
        let sc = h.screen();
        assert!(sc.contains("find ▸ server-session-1"), "{sc}");
        assert!(sc.contains("match 1/1"), "{sc}");
    }
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
        sc.contains("Environments") && sc.contains("Type says where a value lives"),
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

/// Enter on a history entry opens the request and the response kept with
/// it, its echoed secret masked.
#[test]
fn flow_history_shows_the_kept_response() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "list users");
    h.key("f5");
    h.wait_idle(10);
    s.http.take();
    h.key("f7");
    for _ in 0..200 {
        if h.app.history.as_ref().is_some_and(|v| !v.is_empty()) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.pump();
    }
    h.key("enter");
    // Nothing is sent: the response comes from the history file.
    assert!(s.http.take().is_empty());
    let t = &h.app.tabs[h.app.active];
    assert!(
        matches!(
            t.response.as_ref().and_then(|r| r.stored.as_ref()),
            Some(sendito_ui::tab::Stored::History(_))
        ),
        "{}",
        h.screen()
    );
    let sc = h.screen();
    assert!(sc.contains("history: ") && sc.contains("200 OK"), "{sc}");
    assert!(
        sc.contains("Bearer ••••••") && !sc.contains("s3cr3t-token"),
        "{sc}"
    );
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
    // Vim mode is the fifth setting (after Theme, Icons, Key hints and Hide
    // plain secrets).
    h.key("down down down down enter esc");
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
        Some(sendito_ui::overlay::Overlay::Help(_))
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

/// The env editor shows and edits the `dotenv` path: the line says whether
/// the file exists, Tab completes the path, and an empty line removes the key.
#[test]
fn flow_env_editor_edits_the_dotenv_path() {
    use sendito_ui::overlay::Overlay;
    let s = servers();
    let mut h = harness(&s, 100, 30);
    let dotenv = |h: &Harness| match &h.app.overlay {
        Some(Overlay::Env(e)) => (
            e.dotenv.text(),
            e.focus == sendito_ui::overlay::EnvFocus::Dotenv,
        ),
        _ => panic!("the env editor is not open"),
    };
    // The editor opens on the active environment.
    let p = h.dir.path().join(".sendito/environments/local.toml");
    std::fs::write(&p, "dotenv = \".env.x\"\n\n[variables]\na = \"1\"\n").must();
    std::fs::write(h.dir.path().join(".env.x"), "A=1\nB=2\n").must();
    h.key("ctrl+w V");
    let screen = h.screen();
    assert!(screen.contains(".env file"), "{screen}");
    assert!(screen.contains("found · 2 keys"), "{screen}");

    // Up from the first row goes to the line; a path that is not there says so.
    h.key("up");
    assert_eq!(dotenv(&h), (".env.x".to_string(), true));
    h.type_text("y");
    assert!(h.screen().contains("not found"));

    // Tab completes the path, then a second Tab goes to the table.
    h.key("backspace backspace");
    h.key("tab");
    assert_eq!(dotenv(&h), (".env.x".to_string(), true));
    h.key("tab");
    assert_eq!(dotenv(&h), (".env.x".to_string(), false));

    // An edited path is saved; an empty line removes the key.
    h.key("up");
    h.type_text("y");
    h.key("ctrl+s");
    let after = std::fs::read_to_string(&p).must();
    assert_eq!(after, "dotenv = \".env.xy\"\n\n[variables]\na = \"1\"\n");
    h.key("up");
    for _ in 0..7 {
        h.key("backspace");
    }
    assert!(h.screen().contains(".env (default)"));
    h.key("ctrl+s");
    let after = std::fs::read_to_string(&p).must();
    assert_eq!(after, "[variables]\na = \"1\"\n");
}

/// A secret or keychain entry under another name is its own type, and the
/// name is the row's Value: one field per row.
#[test]
fn flow_env_editor_key_types_use_the_value() {
    let s = servers();
    let mut h = harness(&s, 120, 30);
    let p = h.dir.path().join(".sendito/environments/local.toml");
    std::fs::write(
        &p,
        "[variables]\nadmin_pw = { source = \"local\", key = \"admin\" }\n",
    )
    .must();
    h.key("ctrl+w V");
    let screen = h.screen();
    assert!(screen.contains("secret key"), "{screen}");
    assert!(screen.contains("admin"), "{screen}");
    assert!(!screen.contains("Key (optional)"), "{screen}");
    // A save writes the same file back.
    h.key("ctrl+s");
    let after = std::fs::read_to_string(&p).must();
    assert!(after.contains("key = \"admin\""), "{after}");
}

/// F2 on the Value of a `file` row browses for the file: a folder opens,
/// a file fills the Value relative to the collection, and Esc goes back.
#[test]
fn flow_env_editor_browses_for_a_file() {
    use sendito_ui::overlay::{Overlay, PickAction};
    let s = servers();
    let mut h = harness(&s, 120, 30);
    std::fs::create_dir_all(h.dir.path().join("keys")).must();
    std::fs::write(h.dir.path().join("keys/a.pem"), "x").must();
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
    h.type_text("cert");
    h.key("right");
    for _ in 0..5 {
        h.key("space"); // type: file
    }
    h.key("right");
    assert!(h.screen().contains("F2 browses"));

    // Esc in the picker goes back to the editor, edits kept.
    h.key("f2");
    assert!(matches!(
        h.app.overlay,
        Some(Overlay::Picker(ref p)) if matches!(p.action, PickAction::EnvFile { .. })
    ));
    h.key("esc");
    assert!(matches!(h.app.overlay, Some(Overlay::Env(ref e)) if e.dirty));

    h.key("f2");
    h.type_text("keys/");
    h.key("enter");
    assert!(h.screen().contains("Choose a file · keys"));
    h.type_text("a.pem");
    h.key("enter");
    let value = match &h.app.overlay {
        Some(Overlay::Env(e)) => e.table.rows[e.table.sel_row].cells[2].text(),
        _ => panic!("the env editor did not come back"),
    };
    assert_eq!(value, "keys/a.pem");
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
    h.key("space"); // type: secret
    // The secret is typed in the Value column, masked.
    h.key("right");
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
fn flow_help_sections() {
    use sendito_ui::overlay::Overlay;
    let open = |h: &Harness| match &h.app.overlay {
        Some(Overlay::Help(help)) => help.sections[help.sel].name,
        _ => "",
    };
    // F1 opens "Start here"; Tab opens the next section, Shift+Tab the
    // previous one, and both wrap at the ends.
    let mut h = Harness::new(200, 60);
    h.key("f1");
    assert_eq!(open(&h), "Start here");
    assert!(h.find("(type to search this section)").is_some());
    h.key("tab");
    assert_eq!(open(&h), "How it works");
    h.key("shift+tab shift+tab");
    assert_eq!(open(&h), "Lua catalog");
    h.key("tab");
    assert_eq!(open(&h), "Start here");
    // A group heading is not a section: a click on it changes nothing.
    let (x, y) = h.find("Tutorials").must();
    h.click(x, y);
    assert_eq!(open(&h), "Start here");
    // A tutorial shows the keys of the keymap, not the placeholders.
    let (x, y) = h.find("First request").must();
    h.click(x, y);
    assert_eq!(open(&h), "First request");
    assert!(h.find("{key:").is_none());
    // The search applies to the open section and follows it.
    assert!(h.find("$base64(text)").is_none());
    h.type_text("$base64");
    assert!(h.find("no match for \"$base64\"").is_some());
    // A click on a sidebar row opens its section.
    let (x, y) = h.find("Template functions").must();
    h.click(x, y);
    assert_eq!(open(&h), "Template functions");
    assert!(h.find("$base64(text)").is_some(), "the search follows");
    // One column at any width: the dialog stops at the longest row.
    let screen = h.screen();
    let title = screen.lines().find(|l| l.contains("Help (Tab")).must();
    let rule = title.split("Esc closes)").nth(1).must();
    let dashes = rule.chars().take_while(|c| *c == '─').count();
    assert!(dashes < 120, "{dashes}");
    h.key("esc");
    assert!(h.app.overlay.is_none());
    // `:help <section>` opens the help at the section that matches: its
    // name first, else its text; then the search box holds the word.
    h.app.run_command(":help grpc");
    assert_eq!(open(&h), "gRPC call");
    h.key("esc");
    h.app.run_command(":help xpath");
    assert_eq!(open(&h), "Scripts tab");
    let filter = match &h.app.overlay {
        Some(Overlay::Help(help)) => help.filter.text(),
        _ => String::new(),
    };
    assert_eq!(filter, "xpath");
    assert!(h.find("xpath").is_some());
    h.key("esc");
    h.app.run_command(":help no such section");
    assert!(h.app.overlay.is_none());
    let status = h.app.status.as_ref().map(|s| s.0.as_str());
    assert_eq!(status, Some("no help section matches \"no such section\""));
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

/// Ctrl+W O asks the event loop for $EDITOR on a multi-line text; the edited
/// text replaces it. A one-line field and a missing response are refused.
#[test]
fn flow_external_editor_and_pager() {
    use sendito_ui::external::External;
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "echo unary");
    h.app.tabs[h.app.active]
        .ed
        .message
        .set_text("{\"text\": \"a\"}");
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.app.tabs[h.app.active].field = 0;
    h.key("ctrl+w O");
    assert_eq!(
        h.app.external.take(),
        Some(External::Edit {
            text: "{\"text\": \"a\"}".into(),
            ext: "json"
        })
    );
    h.app
        .finish_external(Ok(Some("{\"text\": \"from vi\"}\n".into())));
    let t = &h.app.tabs[h.app.active];
    assert_eq!(t.ed.message.text(), "{\"text\": \"from vi\"}\n");
    assert!(t.dirty());
    assert!(h.screen().contains("text from the editor"));
    // An editor that fails leaves the text as it is.
    h.app
        .finish_external(Err("vi exited with exit status: 1".into()));
    assert!(h.screen().contains("vi exited"));
    assert_eq!(
        h.app.tabs[h.app.active].ed.message.text(),
        "{\"text\": \"from vi\"}\n"
    );
    // The target is one line.
    h.app.focus = sendito_ui::app::Focus::Bar(0);
    h.key("ctrl+w O");
    assert!(h.app.external.is_none());
    assert!(h.screen().contains("this field is one line"));
    // No response yet: nothing for the pager.
    h.key("ctrl+p");
    h.type_text(":pager");
    h.key("enter");
    assert!(h.app.external.is_none());
    assert!(h.screen().contains("no response to show"), "{}", h.screen());
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
    // The target field pulls the tool list in the background, then filters it.
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.app.tabs[h.app.active].field = 2;
    h.type_text("ad");
    let start = std::time::Instant::now();
    while h.app.complete.as_ref().is_none_or(|c| c.items.is_empty()) {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "tool list did not arrive; tools={:?}",
            h.app.tabs[h.app.active].mcp.tools.len()
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
    let sc = h.screen();
    assert!(sc.contains("insert"), "the hint row describes it\n{sc}");
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
    h.app.tabs[h.app.active].field = 2;
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

/// The servers editor lists the servers and shows the form of the selected
/// one; a new server is added with Ctrl+N, and a save keeps every field of
/// the others (TLS check, headers, auth, arguments).
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

    save_to_project(&mut h);
    h.key("ctrl+w S");
    assert!(matches!(h.app.overlay, Some(Overlay::Records(_))));
    let sc = h.screen();
    assert!(sc.contains("MCP servers"), "{sc}");
    assert!(sc.contains("remote") && sc.contains("files"), "{sc}");
    // A stdio server shows its command and one argument per row.
    h.key("enter");
    let sc = h.screen();
    assert!(sc.contains("Command") && sc.contains("npx"), "{sc}");
    assert!(
        sc.contains("@mcp/server-fs") && !sc.contains("URL "),
        "{sc}"
    );

    // Ctrl+N adds an HTTP server: name, then (after the kind) the URL.
    h.key("ctrl+n");
    h.type_text("ledger");
    h.key("down down");
    h.type_text("http://127.0.0.1:9/mcp");
    h.key("ctrl+s");
    let text = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert!(text.contains("[mcp.servers.ledger]"), "{text}");
    assert!(text.contains("http://127.0.0.1:9/mcp"), "{text}");
    // The untouched remote entry kept its verification, headers and auth.
    assert!(text.contains("verify_tls = false"), "{text}");
    assert!(text.contains("x-key"), "{text}");
    assert!(text.contains("type = \"bearer\""), "{text}");
    // So did the stdio entry's arguments.
    assert!(text.contains("\"@mcp/server-fs\""), "{text}");
    assert_eq!(h.app.project.config.mcp.servers.len(), 3);
}

/// With the project scope, the editor shows and writes only the project
/// file's servers: a server from the global config (which may hold a literal
/// token) never reaches the versioned project file, and it stays usable after
/// a save.
#[test]
fn flow_mcp_editor_never_copies_global_servers() {
    use sendito_ui::overlay::Overlay;
    let s = servers();
    let mut h = harness(&s, 120, 40);
    save_to_project(&mut h);
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
    assert!(!matches!(h.app.overlay, Some(Overlay::Records(ref m)) if m.error.is_some()));
    let text = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert!(
        !text.contains("github") && !text.contains("ghp_literal"),
        "{text}"
    );
    assert_eq!(h.app.project.config.mcp.servers.len(), 2);
}

/// With the global scope (the default), the editor shows, adds and edits the
/// global file's servers and writes that file only; `:mcpadd` does the same.
/// The details of a server open in the file that holds it.
#[test]
fn flow_mcp_editor_edits_the_global_scope() {
    use sendito_ui::overlay::Overlay;
    let s = servers();
    let mut h = harness(&s, 120, 40);
    let global = h.dir.path().join(".config/sendito/config.toml");
    std::fs::write(&global, "[mcp.servers.github]\ncommand = \"gh-mcp\"\n").must();
    common::write(
        h.dir.path(),
        ".sendito/config.toml",
        "[mcp.servers.files]\ncommand = \"npx\"\n",
    );
    // Read the two files again, as a start does.
    h.app.project = sendito_ui::runner::Project::open_with(
        h.dir.path(),
        sendito_ui::runner::default_keychain(),
        Some(global.clone()),
    );
    h.key("ctrl+w S");
    let sc = h.screen();
    assert!(sc.contains("MCP servers · global config"), "{sc}");
    assert!(sc.contains("github") && !sc.contains("files"), "{sc}");
    // A new server goes to the global file.
    h.key("ctrl+n");
    h.type_text("ledger");
    h.key("down down");
    h.type_text("http://127.0.0.1:9/mcp");
    h.key("ctrl+s");
    let text = std::fs::read_to_string(&global).must();
    assert!(text.contains("[mcp.servers.ledger]"), "{text}");
    assert!(text.contains("[mcp.servers.github]"), "{text}");
    assert!(!project_file(&h).contains("ledger"));
    assert_eq!(h.app.project.config.mcp.servers.len(), 3);

    // The environment of the global stdio server (the first entry): after
    // the name, the kind, the command and the empty arguments table.
    h.key("esc up enter");
    assert!(h.screen().contains("Environment"), "{}", h.screen());
    h.key("down down down down");
    h.type_text("GITHUB_TOKEN");
    h.key("enter");
    h.type_text("{{token}}");
    h.key("ctrl+s");
    let text = std::fs::read_to_string(&global).must();
    assert!(text.contains("GITHUB_TOKEN = \"{{token}}\""), "{text}");
    assert!(matches!(h.app.overlay, Some(Overlay::Records(_))));

    // `:mcpadd` writes to the scope's file too.
    h.key("esc esc");
    h.app.run_command("mcpadd probe");
    h.type_text("http://127.0.0.1:9/mcp");
    h.key("enter");
    let text = std::fs::read_to_string(&global).must();
    assert!(text.contains("[mcp.servers.probe]"), "{text}");
    assert!(!project_file(&h).contains("probe"));
}

/// `Esc` on a dirty editor asks before discarding; Discard drops the edit and
/// leaves the file alone.
#[test]
fn flow_mcp_editor_asks_before_discarding() {
    use sendito_ui::overlay::{ConfirmAction, Overlay};
    let s = servers();
    let mut h = harness(&s, 120, 40);
    save_to_project(&mut h);
    h.key("ctrl+w S");
    h.key("ctrl+n");
    h.type_text("only");
    // Esc returns from the form to the list, then asks.
    h.key("esc esc");
    assert!(matches!(
        h.app.overlay,
        Some(Overlay::Confirm(ref c)) if matches!(c.action, ConfirmAction::RecordsClose)
    ));
    assert!(h.screen().contains("Unsaved changes"));
    // Cancel keeps the editor and the new entry.
    h.key("right right enter");
    assert!(matches!(h.app.overlay, Some(Overlay::Records(ref e)) if e.records.len() == 1));
    // Discard closes without writing.
    h.key("esc right enter");
    assert!(h.app.overlay.is_none());
    let text = std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must();
    assert!(!text.contains("[mcp.servers"), "{text}");
}

/// `:mcpadd` asks for the URL (or command) and writes the server at once to
/// the config file of the Settings scope.
#[test]
fn flow_mcp_add_command_writes_the_project_config() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    save_to_project(&mut h);
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

/// A settings row click selects it; the value chip changes it (the Theme
/// chip opens the list), so a click on the row does not rewrite the config.
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
        .find(|(_, hit)| matches!(hit, Hit::Overlay(OverlayHit::Item(0))))
        .map(|(r, _)| *r)
        .must();
    h.click(row.x + 2, row.y);
    assert_eq!(theme(&h), before, "a click only selects the row");
    let value = h
        .app
        .hits
        .iter()
        .rev()
        .find(|(_, hit)| matches!(hit, Hit::Overlay(OverlayHit::Value(0))))
        .map(|(r, _)| *r)
        .must();
    // The Theme value opens the list of themes, as Space does.
    h.click(value.x + 1, value.y);
    assert!(matches!(
        &h.app.overlay,
        Some(sendito_ui::overlay::Overlay::Picker(p)) if p.title == "Theme"
    ));
    h.key("down");
    h.key("enter");
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

/// Ctrl+N in the MCP editor adds a server, and the button does the same.
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
        Some(Overlay::Records(e)) => {
            assert_eq!(e.records.len(), 1, "Ctrl+N created the entry");
            assert_eq!(e.records[0].name(), "echo");
        }
        _ => panic!("the MCP editor closed"),
    }
    let (x, y) = h.find("New (Ctrl+N)").must();
    h.click(x + 1, y);
    h.draw();
    match &h.app.overlay {
        Some(Overlay::Records(e)) => assert_eq!(e.records.len(), 2, "the button added one"),
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
        Some(sendito_ui::overlay::Overlay::Settings(v)) => v.sel,
        _ => usize::MAX,
    };
    // Outside the dialog (sidebar) does nothing.
    h.scroll(2, 20, true);
    assert_eq!(sel(&h), 0);
    // Over the dialog moves the selection.
    let (x, y) = h.find("Appearance").must();
    h.scroll(x, y, true);
    assert_eq!(sel(&h), 1, "the wheel moved the settings selection");

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
    if let Some(sendito_ui::overlay::Overlay::Settings(v)) = &mut h.app.overlay {
        v.sel = i;
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
    // Tab switches the file the changes go to.
    h.key("tab");
    assert_eq!(
        h.app.config_scope,
        sendito_ui::overlay::ConfigScope::Project
    );

    // A number is edited in place.
    select_setting(&mut h, "HTTP request timeout");
    h.key("enter");
    assert!(matches!(h.app.overlay, Some(Overlay::Settings(ref v)) if v.edit.is_some()));
    assert!(h.screen().contains("30 s"), "{}", h.screen());
    h.key("ctrl+a");
    h.type_text("zero");
    h.key("enter");
    h.draw();
    assert!(
        h.screen().contains("type a time such as 30 s"),
        "{}",
        h.screen()
    );
    h.key("ctrl+a");
    h.type_text("1500");
    h.key("enter");
    assert!(matches!(h.app.overlay, Some(Overlay::Settings(ref v)) if v.edit.is_none()));
    assert_eq!(h.app.project.config.http.timeout_ms, 1500);
    // The project file sets it now: the row has the dot.
    assert!(
        h.screen().contains("• HTTP request timeout"),
        "{}",
        h.screen()
    );
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

    // Esc leaves the value as it was.
    select_setting(&mut h, "Largest response body");
    h.key("enter");
    h.type_text("9");
    h.key("esc");
    assert!(matches!(h.app.overlay, Some(Overlay::Settings(ref v)) if v.edit.is_none()));
    assert_eq!(h.app.project.config.http.max_body_mb, 100);

    // Saved to the global file while the project file sets the same key:
    // the row and the status say the project value wins.
    h.key("tab");
    assert!(h.screen().contains("project wins"), "{}", h.screen());
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

/// The "MCP servers" row counts the servers of the chosen file (what Enter
/// opens), and Enter opens the servers editor of that scope.
#[test]
fn flow_settings_opens_the_mcp_servers_editor() {
    use sendito_ui::overlay::Overlay;
    let s = servers();
    let mut h = harness(&s, 120, 40);
    common::write(
        h.dir.path(),
        ".sendito/config.toml",
        "[mcp.servers.files]\ncommand = \"npx\"\n",
    );
    h.app.project = sendito_ui::runner::Project::open_with(
        h.dir.path(),
        sendito_ui::runner::default_keychain(),
        None,
    );
    save_to_project(&mut h);
    h.app.run_command("settings");
    select_setting(&mut h, "MCP servers");
    h.draw();
    let sc = h.screen();
    assert!(sc.contains("1 server"), "{sc}");
    assert!(sc.contains("Enter opens the editor"), "{sc}");
    h.key("enter");
    assert!(matches!(h.app.overlay, Some(Overlay::Records(_))));
    assert!(h.screen().contains("files"), "{}", h.screen());
}

/// The scope chosen in Settings is kept in `session.toml`: a start restores
/// it, and a session without it starts at global.
#[test]
fn flow_settings_scope_survives_a_restart() {
    use sendito_ui::overlay::ConfigScope;
    let s = servers();
    let mut h = harness(&s, 120, 40);
    assert_eq!(h.app.config_scope, ConfigScope::Global);
    h.app.run_command("settings");
    h.key("tab");
    assert_eq!(h.app.config_scope, ConfigScope::Project);
    h.reopen();
    assert_eq!(h.app.config_scope, ConfigScope::Project);
    h.app.run_command("settings");
    assert!(h.screen().contains("project"), "{}", h.screen());
}

/// Opens the settings, sets the scope to the project file and closes them.
fn save_to_project(h: &mut Harness) {
    h.app.config_scope = sendito_ui::overlay::ConfigScope::Project;
}

fn project_file(h: &Harness) -> String {
    std::fs::read_to_string(h.dir.path().join(".sendito/config.toml")).must()
}

/// The form of a server shows what applies to it: the TLS check, headers
/// and OAuth overrides of an http server, the arguments and environment of a
/// stdio one. Ctrl+S writes them; a plain header value gets a warning.
#[test]
fn flow_mcp_server_details() {
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
    save_to_project(&mut h);
    h.key("ctrl+w S");
    // Entries are sorted by name: files, remote.
    h.key("down enter");
    let sc = h.screen();
    assert!(
        sc.contains("Verify TLS") && sc.contains("Client ID"),
        "{sc}"
    );
    // Name, kind, URL, auth, TLS check, then the headers table.
    h.key("down down down down down");
    h.type_text("x-api-key");
    h.key("enter");
    h.type_text("plain");
    // Down twice leaves the table for the client ID.
    h.key("down down");
    h.type_text("my-app");
    h.key("ctrl+s");
    let file = project_file(&h);
    assert!(file.contains("client_id = \"my-app\""), "{file}");
    assert!(file.contains("x-api-key = \"plain\""), "{file}");
    assert!(status(&h).contains("plain text"), "{}", status(&h));

    // The stdio server: one argument per row keeps its spaces and quotes.
    h.key("esc up enter");
    let sc = h.screen();
    assert!(
        sc.contains("Arguments") && sc.contains("Environment"),
        "{sc}"
    );
    h.key("down down down");
    h.type_text("-y");
    h.key("down");
    h.type_text("say \"hi\"");
    h.key("down down");
    h.type_text("DEBUG");
    h.key("enter");
    h.type_text("1");
    h.key("ctrl+s");
    let file = project_file(&h);
    assert!(file.contains("args = [\"-y\", 'say \"hi\"']"), "{file}");
    assert!(file.contains("DEBUG = \"1\""), "{file}");
    // The test uses the saved server: unsaved changes are refused.
    h.key("up up up up up up");
    h.type_text("x");
    h.key("ctrl+t");
    assert!(h.screen().contains("save (Ctrl+S) first"), "{}", h.screen());
}

/// The connection test reports the number of tools in the editor.
#[test]
fn flow_mcp_editor_test_result() {
    use sendito_ui::app::AppMsg;
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    h.key("ctrl+w S");
    h.app.on_msg(AppMsg::McpTest {
        server: "echo".into(),
        result: Ok(3),
    });
    assert!(
        h.screen().contains("✓ echo answered: 3 tools"),
        "{}",
        h.screen()
    );
    h.app.on_msg(AppMsg::McpTest {
        server: "echo".into(),
        result: Err("connection refused".into()),
    });
    assert!(
        matches!(&h.app.overlay, Some(Overlay::Records(e)) if e.error.as_deref() == Some("echo: connection refused"))
    );
}

/// The OAuth editor writes [oauth.<name>] to the file the settings choose:
/// the grant is a choice that shows only its fields, missing fields are
/// reported, a plain secret is saved with a warning.
#[test]
fn flow_oauth_editor() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    save_to_project(&mut h);
    h.app.run_command("oauth");
    assert!(matches!(h.app.overlay, Some(Overlay::Records(_))));
    h.key("ctrl+n");
    h.type_text("gh");
    // The browser grant: no username or password fields.
    let sc = h.screen();
    assert!(
        sc.contains("Authorize URL") && !sc.contains("Username"),
        "{sc}"
    );
    h.key("ctrl+s");
    assert!(
        h.screen().contains("gh: set the authorize URL"),
        "{}",
        h.screen()
    );
    // The password grant shows the username and the password instead.
    h.key("down right right");
    let sc = h.screen();
    assert!(
        sc.contains("Username") && !sc.contains("Authorize URL"),
        "{sc}"
    );
    h.key("left left down");
    h.type_text("https://github.com/login/oauth/authorize");
    h.key("ctrl+s");
    assert!(
        h.screen().contains("gh: set the token URL"),
        "{}",
        h.screen()
    );
    h.key("down");
    h.type_text("https://github.com/login/oauth/access_token");
    h.key("down");
    h.type_text("abc");
    h.key("down");
    h.type_text("{{gh_secret}}");
    h.key("down");
    h.type_text("repo read:user");
    h.key("ctrl+s");
    assert!(status(&h).starts_with("saved to"), "{}", status(&h));
    let id = &h.app.project.config.oauth["gh"];
    assert_eq!(id.client_id, "abc");
    assert_eq!(id.client_secret.as_deref(), Some("{{gh_secret}}"));
    assert_eq!(id.scopes, vec!["repo", "read:user"]);
    assert_eq!(
        id.grant,
        sendito_store::config::OAuthGrant::AuthorizationCode
    );
    assert!(
        project_file(&h).contains("[oauth.gh]"),
        "{}",
        project_file(&h)
    );
}

/// F2 on a request's OAuth identity lists the identities; choosing one
/// fills the field, and "New identity…" opens the editor on a new entry.
#[test]
fn flow_oauth_identity_picker() {
    use sendito_ui::app::Focus;
    use sendito_ui::overlay::Overlay;
    use sendito_ui::tab::Field;
    let mut h = harness(&servers(), 120, 40);
    h.app.project.config.oauth.insert(
        "service".into(),
        sendito_store::config::OAuthIdentity::default(),
    );
    h.key("ctrl+n");
    let t = h.app.tab_mut().must();
    t.ed.auth_kind = sendito_core::AuthKind::OAuth;
    t.req_tab = 3;
    t.field = t
        .fields()
        .iter()
        .position(|f| *f == Field::AuthOAuthName)
        .must();
    h.app.focus = Focus::ReqBody;
    h.key("f2");
    match &h.app.overlay {
        Some(Overlay::Picker(p)) => {
            assert_eq!(
                p.items,
                vec!["service".to_string(), "New identity…".to_string()]
            )
        }
        _ => panic!("no picker"),
    }
    h.key("enter");
    assert_eq!(h.app.tab_mut().must().ed.auth_oauth_name.text(), "service");
    // The editor reads the config file (the identity above is in memory
    // only) and opens on a new entry.
    h.key("f2");
    h.key("down enter");
    assert!(
        matches!(&h.app.overlay, Some(Overlay::Records(e)) if e.records.len() == 1
            && e.pane == sendito_ui::records::Pane::Form),
        "{}",
        h.screen()
    );
}

/// The keymap editor lists every action with its keys: Enter records new
/// keys (a pause ends the recording), Ctrl+A adds one, Del restores the
/// default, and Ctrl+S applies and writes them.
#[test]
fn flow_keymap_editor() {
    use sendito_ui::keymap::{Action, Chord, Ctx, Lookup};
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    save_to_project(&mut h);
    h.app.run_command("settings");
    select_setting(&mut h, "Key bindings");
    h.key("enter");
    let sc = h.screen();
    assert!(sc.contains("Key bindings · project config"), "{sc}");
    assert!(sc.contains("Send the request"), "{sc}");
    // The filter finds the action; Enter records Ctrl+G.
    h.type_text("save the request");
    assert!(h.screen().contains("Ctrl+S"), "{}", h.screen());
    h.key("enter");
    assert!(h.screen().contains("press the keys"), "{}", h.screen());
    h.key("ctrl+g");
    // The recording ends after a pause.
    if let Some(Overlay::Keymap(e)) = &mut h.app.overlay
        && let Some(c) = &mut e.capture
    {
        c.at -= sendito_ui::keymap_editor::CAPTURE_PAUSE;
    }
    h.app.tick();
    assert!(h.screen().contains("Ctrl+G  (changed)"), "{}", h.screen());
    // Ctrl+A adds F9 after it.
    h.key("ctrl+a");
    h.key("f9");
    if let Some(Overlay::Keymap(e)) = &mut h.app.overlay
        && let Some(c) = &mut e.capture
    {
        c.at -= sendito_ui::keymap_editor::CAPTURE_PAUSE;
    }
    h.app.tick();
    h.key("ctrl+s");
    let g = Chord::parse("ctrl+g").must();
    assert_eq!(
        h.app.keymap.lookup(&[Ctx::Global], false, &[g]),
        Lookup::Action(Action::Save)
    );
    let file = project_file(&h);
    assert!(file.contains("[keymap.all]"), "{file}");
    assert!(file.contains("save = [\"Ctrl+G\", \"F9\"]"), "{file}");
    // Esc cancels a recording; Del restores the default keys.
    h.key("enter esc");
    assert!(h.screen().contains("recording cancelled"), "{}", h.screen());
    h.key("delete");
    assert!(!h.screen().contains("(changed)"), "{}", h.screen());
    h.key("ctrl+s");
    assert!(!project_file(&h).contains("save ="), "{}", project_file(&h));
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
        Some(Overlay::Settings(v)) => assert_eq!(SETTINGS[v.sel].0, SettingId::Vim),
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
    // The selection shows at once and is not saved; Esc brings back the
    // saved theme.
    h.app.run_command("theme");
    h.key("down");
    assert_eq!(&*h.app.theme.name, "ember");
    assert_eq!(h.app.project.config.ui.theme, "daylight");
    h.key("esc");
    assert!(h.app.overlay.is_none());
    assert_eq!(&*h.app.theme.name, "daylight");
    // A click outside closes it as Esc does.
    h.app.run_command("theme");
    h.key("down");
    h.click(0, 0);
    assert!(h.app.overlay.is_none());
    assert_eq!(&*h.app.theme.name, "daylight");
    // The kind of a theme is a detail; typing it filters the list.
    assert_eq!(h.app.themes.kind("bauhaus"), "light · solid selection");
    assert_eq!(h.app.themes.kind("dracula"), "dark");
    h.app.run_command("theme");
    h.type_text("solid");
    match &h.app.overlay {
        Some(Overlay::Picker(p)) => {
            let names: Vec<&str> = p.visible().iter().map(|i| p.items[*i].as_str()).collect();
            assert!(names.contains(&"turbo") && names.contains(&"contrast"));
            assert!(!names.contains(&"dracula"));
        }
        _ => panic!("the theme picker closed"),
    }
    h.key("esc");
    // Settings: Space opens the list and Esc returns to Settings; Enter in
    // the list saves the theme and returns too.
    h.app.open_settings(sendito_ui::overlay::SettingId::Theme);
    h.key("space");
    assert!(matches!(&h.app.overlay, Some(Overlay::Picker(p)) if p.title == "Theme"));
    h.key("esc");
    assert!(matches!(&h.app.overlay, Some(Overlay::Settings(_))));
    h.key("enter");
    h.type_text("nord");
    h.key("enter");
    assert!(matches!(&h.app.overlay, Some(Overlay::Settings(_))));
    assert_eq!(h.app.project.config.ui.theme, "nord");
    // ← → still step through the themes.
    h.key("right");
    assert_eq!(h.app.project.config.ui.theme, "gruvbox");
}

/// The `theme` of an environment shows while it is active, and the top bar
/// shows that environment as a solid chip. `:theme` lasts until the next
/// switch; a theme that does not exist falls back to `[ui] theme`.
#[test]
fn flow_environment_theme() {
    use sendito_ui::app::StatusKind;
    let mut h = harness(&servers(), 120, 40);
    let staging = h.dir.path().join(".sendito/environments/staging.toml");
    let vars = std::fs::read_to_string(&staging).must();
    std::fs::write(&staging, format!("theme = \"Ember\"\n{vars}")).must();
    assert!(h.app.env_theme.is_none());
    h.draw();
    let (x, y) = h.find("local ▾").must();
    let plain = h.term.backend().buffer()[(x, y)].clone();
    assert_ne!(plain.bg, h.app.theme.env_color("local"));
    // The environment's theme and the solid chip.
    h.app.set_env(Some("staging".into()));
    assert_eq!(&*h.app.theme.name, "ember");
    assert_eq!(h.app.env_theme.as_deref(), Some("ember"));
    h.draw();
    let (x, y) = h.find("staging ▾").must();
    let chip = h.term.backend().buffer()[(x, y)].clone();
    assert_eq!(chip.bg, h.app.theme.env_color("staging"));
    // :theme shows its theme until the environment changes.
    h.app.run_command("theme daylight");
    assert_eq!(&*h.app.theme.name, "daylight");
    h.app.set_env(Some("local".into()));
    assert_eq!(&*h.app.theme.name, "daylight");
    assert!(h.app.env_theme.is_none());
    h.app.set_env(Some("staging".into()));
    assert_eq!(&*h.app.theme.name, "ember");
    // A restart on that environment starts with its theme.
    h.reopen();
    assert_eq!(&*h.app.theme.name, "ember");
    // A theme that does not exist: the [ui] theme and a warning.
    std::fs::write(&staging, format!("theme = \"nope\"\n{vars}")).must();
    h.app.set_env(Some("local".into()));
    h.app.set_env(Some("staging".into()));
    assert_eq!(&*h.app.theme.name, "daylight");
    assert!(h.app.env_theme.is_none());
    let (msg, kind, _) = h.app.status.as_ref().must();
    assert_eq!(*kind, StatusKind::Warn);
    assert!(msg.contains("no theme named \"nope\""), "{msg}");
}

/// The Theme line of the environment editor is a choice row, as in
/// Settings: ← → change the theme and show it while the line has the
/// focus, Space lists the themes and shows the selection, Esc keeps the
/// line, Enter takes the selection, and a save writes the key and applies
/// the theme of the active environment.
#[test]
fn flow_env_editor_sets_the_theme() {
    use sendito_ui::overlay::{EnvFocus, Overlay};
    let mut h = harness(&servers(), 120, 40);
    let shown = h.app.theme.name.clone();
    let line = |h: &Harness| match &h.app.overlay {
        Some(Overlay::Env(e)) => (e.theme.clone(), e.focus, e.dirty),
        _ => panic!("the env editor is not open"),
    };
    let names = h.app.themes.names();
    let total = names.len() + 1;
    h.key("ctrl+w V");
    let screen = h.screen();
    assert!(
        screen.contains(&format!("‹ (base theme) › (1 of {total})")),
        "{screen}"
    );
    // Up from the table: the .env file line, then the Theme line.
    h.key("up up");
    assert_eq!(line(&h), (None, EnvFocus::Theme, false));
    // ← → step through the themes, after "(base theme)", and the screen
    // shows each one.
    h.key("right");
    assert_eq!(h.app.theme.name, h.app.themes.theme(&names[0]).name);
    assert_eq!(line(&h), (Some(names[0].clone()), EnvFocus::Theme, true));
    assert!(
        h.screen()
            .contains(&format!("‹ {} › (2 of {total})", names[0]))
    );
    h.key("left");
    assert_eq!(line(&h).0, None);
    assert_eq!(h.app.theme.name, shown);
    h.key("left");
    assert_eq!(line(&h).0, names.last().cloned());
    assert_eq!(
        h.app.theme.name,
        h.app.themes.theme(&names[names.len() - 1]).name
    );
    // The theme in use comes back when the line loses the focus.
    h.key("down");
    assert_eq!(h.app.theme.name, shown);
    h.key("up right");
    // Space lists the themes; the selection shows at once and Esc keeps
    // the line.
    h.key("space");
    assert!(
        matches!(&h.app.overlay, Some(Overlay::Picker(p)) if p.title == "Theme of the environment")
    );
    h.type_text("ember");
    assert_eq!(&*h.app.theme.name, "ember");
    h.key("esc");
    assert_eq!(h.app.theme.name, shown);
    assert_eq!(line(&h).0, None);
    // Enter takes the selection; the theme shows after the save.
    h.key("enter");
    h.type_text("ember");
    h.key("enter");
    assert_eq!(line(&h), (Some("ember".to_string()), EnvFocus::Theme, true));
    assert_eq!(&*h.app.theme.name, "ember");
    assert!(h.screen().contains("✓ dark"));
    h.key("ctrl+s");
    let p = h.dir.path().join(".sendito/environments/local.toml");
    let after = std::fs::read_to_string(&p).must();
    assert!(after.starts_with("theme = \"ember\"\n"), "{after}");
    assert_eq!(&*h.app.theme.name, "ember");
    // A click on the value lists the themes too.
    let (x, y) = h.find("‹ ember ›").must();
    h.click(x, y);
    assert!(matches!(&h.app.overlay, Some(Overlay::Picker(_))));
    h.key("esc");
    // A name in the file that no theme has says so.
    let text = std::fs::read_to_string(&p).must();
    std::fs::write(&p, text.replace("ember", "nope")).must();
    h.app.overlay = None;
    h.key("ctrl+w V");
    assert!(h.screen().contains("no theme has this name"));
    // The collection variables have no Theme line.
    h.key("ctrl+pageup");
    h.key("up up");
    assert_eq!(line(&h).1, EnvFocus::Dotenv);
}

/// Each row of the theme list ends with that theme's colors on its own
/// background.
#[test]
fn flow_theme_picker_rows_show_the_colors() {
    use sendito_ui::app::{Hit, OverlayHit};
    let mut h = harness(&servers(), 120, 40);
    h.app.run_command("theme");
    h.type_text("bauhaus");
    h.draw();
    let row = h
        .app
        .hits
        .iter()
        .rev()
        .find(|(_, hit)| matches!(hit, Hit::Overlay(OverlayHit::Item(0))))
        .map(|(r, _)| *r)
        .must();
    let t = h.app.themes.theme("bauhaus");
    let sw = t.swatch();
    // The swatch, a cell of background on each side, then a margin.
    let first = row.x + row.width - sw.len() as u16 - 2;
    let buf = h.term.backend().buffer().clone();
    assert_eq!(buf[(first - 1, row.y)].bg, t.bg);
    for (i, c) in sw.iter().enumerate() {
        let cell = &buf[(first + i as u16, row.y)];
        assert_eq!(cell.symbol(), "█");
        assert_eq!((cell.fg, cell.bg), (*c, t.bg));
    }
    assert_eq!(buf[(first + sw.len() as u16, row.y)].bg, t.bg);
}

/// `:theme <text>` lists the matching themes in the palette.
#[test]
fn flow_palette_lists_themes() {
    use sendito_ui::overlay::{Overlay, PaletteItem};
    let mut h = harness(&servers(), 120, 40);
    h.key("ctrl+p");
    h.type_text(":theme latte");
    match &h.app.overlay {
        Some(Overlay::Palette(p)) => {
            let v = p.visible();
            assert_eq!(v.len(), 1);
            assert_eq!(
                p.items[v[0]].0,
                PaletteItem::Theme("catppuccin-latte".into())
            );
        }
        _ => panic!("the palette closed"),
    }
    h.key("enter");
    assert_eq!(h.app.project.config.ui.theme, "catppuccin-latte");
    // Without a ':' the themes are found by name too.
    h.key("ctrl+p");
    h.type_text("theme: monokai");
    h.key("enter");
    assert_eq!(h.app.project.config.ui.theme, "monokai");
    // `:theme reload` matches no theme name and still runs.
    h.key("ctrl+p");
    h.type_text(":theme reload");
    h.key("enter");
    assert!(h.screen().contains("themes reloaded"));
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
    // Without a name, the editor lists the servers; with one, its form opens.
    h.app.run_command("mcpedit");
    assert!(
        matches!(&h.app.overlay, Some(Overlay::Records(e)) if e.records.iter().any(|r| r.name() == "ledger"))
    );
    h.key("esc");
    h.app.run_command("mcpedit ledger");
    assert!(
        matches!(&h.app.overlay, Some(Overlay::Records(e)) if e.pane == sendito_ui::records::Pane::Form),
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

/// `s` on a tree folder opens its settings; saved headers and variables go
/// out with the requests below it, and the request shows what it inherits.
#[test]
fn flow_folder_settings_apply_to_requests() {
    let s = servers();
    let mut h = harness(&s, 140, 40);
    h.key("ctrl+w h");
    assert!(h.app.tree.select_rel("Users"));
    h.key("s");
    let t = &mut h.app.tabs[h.app.active];
    assert_eq!(t.folder.as_deref(), Some("Users"));
    t.ed.headers.push_texts(true, &["X-Team", "{{team}}"]);
    t.ed.vars.push_texts(true, &["team", "blue"]);
    t.refresh_model();
    assert!(t.dirty());
    let sc = h.screen();
    assert!(
        sc.contains("FOLDER") && sc.contains("applies to 3 requests"),
        "{sc}"
    );
    // Its own variables are defined while the folder edits them.
    assert!(!sc.contains("undefined"), "{sc}");
    // A folder tab is not sent.
    h.key("f5");
    assert!(h.app.tabs[h.app.active].running.is_none());
    h.key("ctrl+s");
    let file = std::fs::read_to_string(h.dir.path().join("Users/folder.toml")).must();
    assert!(file.contains("X-Team") && file.contains("blue"), "{file}");
    assert!(!h.app.tabs[h.app.active].dirty());
    // A request of the folder lists the inherited header and sends it.
    open(&mut h, "delete user");
    h.app.tabs[h.app.active].req_tab = 1;
    let sc = h.screen();
    assert!(
        sc.contains("From folders") && sc.contains("X-Team: {{team}}"),
        "{sc}"
    );
    h.key("f5");
    h.wait_idle(10);
    let caps = s.http.take();
    assert_eq!(caps.len(), 1);
    assert_eq!(caps[0].header("x-team").as_deref(), Some("blue"));
    // The folder tab comes back with the session.
    let session = h.app.session();
    assert!(
        session
            .tabs
            .iter()
            .any(|t| t.folder.as_deref() == Some("Users"))
    );
    // Renaming the folder moves the open folder tab with it.
    h.key("ctrl+w h");
    h.app.tree.select_rel("Users");
    h.key("r");
    h.key("ctrl+a");
    h.type_text("People");
    h.key("enter");
    assert!(
        h.app
            .tabs
            .iter()
            .any(|t| t.folder.as_deref() == Some("People")),
        "{:?}",
        h.app
            .tabs
            .iter()
            .map(|t| t.folder.clone())
            .collect::<Vec<_>>()
    );
}

/// `:saveexample` saves the response next to the request, with credential
/// headers masked; `:examples` lists the examples and shows one.
#[test]
fn flow_save_and_show_an_example() {
    let s = servers();
    let mut h = harness(&s, 140, 40);
    open(&mut h, "list users");
    // Nothing to save before a send.
    h.app.run_command("saveexample");
    assert!(
        status(&h).contains("send the request first"),
        "{}",
        status(&h)
    );
    h.key("f5");
    h.wait_idle(10);
    h.app.run_command("saveexample");
    assert!(
        status(&h).contains("saved example 200-ok"),
        "{}",
        status(&h)
    );
    let file =
        std::fs::read_to_string(h.dir.path().join("Users/list users.examples/200-ok.toml")).must();
    assert!(file.starts_with("status = 200\n"), "{file}");
    // The echo body names the request's headers: the bearer token is masked.
    assert!(!file.contains("s3cr3t-token"), "{file}");
    // The examples directory is not a folder in the tree.
    h.app.rescan();
    assert!(
        h.app
            .tree
            .root
            .find_folder("Users/list users.examples")
            .is_none()
    );
    h.app.run_command("examples");
    assert!(h.screen().contains("Saved examples"), "{}", h.screen());
    h.key("enter");
    let sc = h.screen();
    assert!(sc.contains("example: 200-ok"), "{sc}");
    h.app.run_command("examples delete 200-ok");
    assert!(!h.dir.path().join("Users/list users.examples").exists());
}

/// Opens a new HTTP tab with Basic auth and `pass` typed in the password,
/// with the focus on the password field.
fn basic_auth_tab(h: &mut Harness, pass: &str) {
    use sendito_ui::app::Focus;
    use sendito_ui::tab::Field;
    h.key("ctrl+n");
    let t = h.app.tab_mut().must();
    t.ed.auth_kind = sendito_core::AuthKind::Basic;
    t.ed.auth_pass.set_text(pass);
    t.req_tab = 3;
    t.field = t.fields().iter().position(|f| *f == Field::AuthPass).must();
    t.refresh_model();
    h.app.focus = Focus::ReqBody;
    h.draw();
}

/// A password typed as plain text is marked, and Ctrl+W K moves it into a
/// secret of the active environment: the field becomes {{name}}.
#[test]
fn flow_move_plain_password_into_a_secret() {
    use sendito_ui::overlay::Overlay;
    let mut h = Harness::new(120, 40);
    h.app.set_env(Some("local".into()));
    basic_auth_tab(&mut h, "hunter2");
    let screen = h.screen();
    assert!(screen.contains("⚠ plain text"), "{screen}");
    assert!(
        screen.contains("Plain text: the request file keeps it"),
        "{screen}"
    );
    h.key("ctrl+w K");
    match &h.app.overlay {
        Some(Overlay::Prompt(p)) => assert_eq!(p.input.text(), "password"),
        _ => panic!("no prompt"),
    }
    h.key("enter");
    assert!(h.app.overlay.is_none());
    assert_eq!(h.app.tab_mut().must().ed.auth_pass.text(), "{{password}}");
    let env = h.app.project.envs.load("local").must();
    assert!(matches!(
        env.get("password").map(|v| &v.source),
        Some(sendito_store::env::VarSource::Local { key: None })
    ));
    let secrets = std::fs::read_to_string(h.dir.path().join(".sendito/secrets.toml")).must();
    assert!(secrets.contains("hunter2"), "{secrets}");
    assert!(!h.screen().contains("⚠ plain text"));

    // A name the environment already has is refused; nothing changes.
    basic_auth_tab(&mut h, "other");
    h.key("ctrl+w K");
    h.key("enter");
    match &h.app.overlay {
        Some(Overlay::Prompt(p)) => {
            assert!(p.error.as_deref().must().contains("already defined"))
        }
        _ => panic!("the prompt closed"),
    }
    assert_eq!(h.app.tab_mut().must().ed.auth_pass.text(), "other");
}

/// With Hide plain secrets on, a plain password shows as dots until
/// Ctrl+W * shows it.
#[test]
fn flow_hide_plain_secrets() {
    let mut h = Harness::new(120, 40);
    h.app.project.config.ui.hide_plain_secrets = true;
    basic_auth_tab(&mut h, "hunter2");
    let screen = h.screen();
    assert!(!screen.contains("hunter2"), "{screen}");
    assert!(screen.contains("•••"), "{screen}");
    h.key("ctrl+w *");
    assert!(h.screen().contains("hunter2"));
}

/// An empty timeout says what applies; a typed "2s" is 2000 ms in the
/// request, and the help line explains the field.
#[test]
fn flow_timeout_shows_the_default_and_takes_units() {
    use sendito_ui::app::Focus;
    let mut h = Harness::new(120, 40);
    h.key("ctrl+n");
    let t = h.app.tab_mut().must();
    t.req_tab = 5;
    t.field = 0;
    h.app.focus = Focus::ReqBody;
    let screen = h.screen();
    assert!(screen.contains("default · 30 s (Settings)"), "{screen}");
    assert!(
        screen.contains("The longest time the request may take"),
        "{screen}"
    );
    h.type_text("2s");
    assert_eq!(h.app.tab_mut().must().ed.timeout.text(), "2s");
    let Some(sendito_core::Request::Http(r)) = h.app.tab().map(|t| t.model.clone()) else {
        panic!("not http")
    };
    assert_eq!(r.options.timeout_ms, Some(2000));
}

/// A timeout setting shows and takes a time with its unit.
#[test]
fn flow_setting_timeout_in_seconds() {
    use sendito_ui::overlay::{Overlay, SettingId};
    let mut h = Harness::new(120, 40);
    h.app.open_settings(SettingId::HttpTimeout);
    assert!(h.screen().contains("‹ 30 s ›"));
    // Enter edits the value in place, selected.
    h.key("enter");
    match &h.app.overlay {
        Some(Overlay::Settings(v)) => assert_eq!(v.edit.as_ref().must().text(), "30 s"),
        _ => panic!("the settings closed"),
    }
    h.type_text("5s");
    h.key("enter");
    assert_eq!(h.app.project.config.http.timeout_ms, 5000);
    h.key("enter");
    h.key("ctrl+a");
    h.type_text("soon");
    h.key("enter");
    match &h.app.overlay {
        Some(Overlay::Settings(v)) => {
            assert!(v.error.as_deref().must().contains("30 s"));
            assert!(v.edit.is_some(), "the value stays to be fixed");
        }
        _ => panic!("the settings closed"),
    }
}

/// "Define" in the undefined-variables dialog opens the environment editor
/// with a row for each missing variable, ready for its value.
#[test]
fn flow_undefined_variables_define() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    open(&mut h, "get order");
    h.key("f5");
    assert!(h.screen().contains("Define (d)"), "{}", h.screen());
    h.key("d");
    match &h.app.overlay {
        Some(Overlay::Env(e)) => {
            let row = e.table.rows.get(e.table.sel_row).must();
            assert_eq!(row.cells[0].text(), "order_id");
            assert_eq!(e.table.sel_col, 2, "the value cell is selected");
            assert!(e.dirty);
        }
        _ => panic!("the environment editor did not open"),
    }
    h.type_text("42");
    h.key("ctrl+s");
    let env = h.app.project.envs.load("local").must();
    assert!(env.get("order_id").is_some());
}

/// A name prompt shows the file it creates, and a name already taken
/// before Enter.
#[test]
fn flow_new_request_previews_the_file() {
    let mut h = harness(&servers(), 120, 40);
    h.app.tree.select_rel("Orders");
    h.key("ctrl+w h n");
    h.type_text("get order");
    assert!(
        h.screen().contains("already exists in /Orders"),
        "{}",
        h.screen()
    );
    h.type_text("s");
    assert!(
        h.screen().contains("→ Orders/get orders.toml"),
        "{}",
        h.screen()
    );
}

/// The import dialog says what the text would import, and Tab completes
/// a path.
/// A dialog key changed in the keymap applies in the dialog and shows in its
/// hints; the old key does nothing there.
#[test]
fn flow_dialog_key_follows_the_keymap() {
    use sendito_store::config::KeySpec;
    use sendito_ui::keymap::Keymap;
    use std::collections::BTreeMap;
    let mut h = harness(&servers(), 120, 40);
    let cfg = BTreeMap::from([(
        "all".to_string(),
        BTreeMap::from([("dialog_confirm".to_string(), KeySpec::One("f6".into()))]),
    )]);
    h.app.keymap = Keymap::defaults().with_overrides(&cfg).0;
    h.key("ctrl+w i");
    h.type_text("curl https://example.test/a");
    let hints: Vec<String> = h.app.hints().into_iter().map(|(k, _)| k).collect();
    assert_eq!(hints.first().map(String::as_str), Some("F6"), "{hints:?}");
    h.key("ctrl+s");
    assert!(h.app.overlay.is_some(), "the old key imported");
    h.key("f6");
    assert!(h.app.overlay.is_none(), "the new key did not import");
}

#[test]
fn flow_import_preview_and_completion() {
    use sendito_ui::overlay::Overlay;
    let mut h = harness(&servers(), 120, 40);
    common::write(
        h.dir.path(),
        "exports/shop.postman_collection.json",
        r#"{"info":{"name":"shop","schema":"https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},"item":[{"name":"list","request":{"method":"GET","url":"https://shop.test/items"}}]}"#,
    );
    h.key("ctrl+w i");
    h.type_text("curl -X POST https://example.test/a");
    assert!(
        h.screen().contains("✓ curl · POST https://example.test/a"),
        "{}",
        h.screen()
    );
    let dir = h.dir.path().join("exports").display().to_string();
    if let Some(Overlay::Import(i)) = &mut h.app.overlay {
        i.input.set_text(&format!("{dir}/sh"));
        i.input.move_to_end();
    }
    h.key("tab");
    let text = match &h.app.overlay {
        Some(Overlay::Import(i)) => i.input.text(),
        _ => panic!("the import dialog closed"),
    };
    assert!(text.ends_with("/shop.postman_collection.json"), "{text}");
    assert!(
        h.screen().contains("\"shop\" · 1 request"),
        "{}",
        h.screen()
    );
    if let Some(Overlay::Import(i)) = &mut h.app.overlay {
        i.input.set_text("no/such/file.json");
    }
    assert!(
        h.screen().contains("✗ no file or directory"),
        "{}",
        h.screen()
    );
}

/// A Lua function that fails with the loaded values stops the send with the
/// reason; the names check counts it as defined.
#[test]
fn flow_lua_function_that_fails_asks_before_sending() {
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
        common::write(
            d,
            ".sendito/lua/util.lua",
            "function boom() error('no order') end\n",
        );
        common::write(
            d,
            "boom.toml",
            "kind = \"http\"\nmethod = \"GET\"\nurl = \"http://127.0.0.1:9/{{ $lua.boom() }}\"\n",
        );
    });
    open(&mut h, "boom");
    assert!(!h.screen().contains("undefined"), "{}", h.screen());
    h.key("f5");
    let sc = h.wait_for("Functions not resolved", 10);
    assert!(sc.contains("no order"), "{sc}");
}

/// `$prompt` and `$select` ask their values before the send, one dialog per
/// label; the request goes out with the answers, and the next send suggests
/// the last value that is not a secret.
#[test]
fn flow_prompt_and_select_ask_before_sending() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    let url = format!(
        "{}/p/{{{{ $prompt('Order ID') }}}}/{{{{ $select('Mode', 'sandbox', 'live') }}}}?k={{{{ $prompt('Key', 'secret') }}}}",
        s.http.url()
    );
    common::write(
        h.dir.path(),
        "ask.toml",
        &format!("kind = \"http\"\nmethod = \"GET\"\nurl = \"{url}\"\n"),
    );
    h.app.rescan();
    open(&mut h, "ask");
    assert!(!h.screen().contains("undefined"), "{}", h.screen());
    h.key("f5");
    let sc = h.screen();
    assert!(
        sc.contains("Value to send (1 of 3)") && sc.contains("Order ID"),
        "{sc}"
    );
    h.type_text("42");
    h.key("enter");
    assert!(
        h.screen().contains("Mode: value to send (2 of 3)"),
        "{}",
        h.screen()
    );
    h.key("down");
    h.key("enter");
    let sc = h.screen();
    assert!(
        sc.contains("Value to send (3 of 3)") && sc.contains("Key"),
        "{sc}"
    );
    h.type_text("s3cret");
    assert!(!h.screen().contains("s3cret"), "{}", h.screen());
    h.key("enter");
    h.wait_for("200", 10);
    let got = s.http.last().must();
    assert_eq!(got.target(), "/p/42/live?k=s3cret");
    // The next send suggests the values that are not secrets.
    h.key("f5");
    match &h.app.overlay {
        Some(sendito_ui::overlay::Overlay::Prompt(p)) => assert_eq!(p.input.text(), "42"),
        _ => panic!("no prompt: {}", h.screen()),
    }
    assert_eq!(
        h.app
            .session()
            .prompt_values
            .get("Mode")
            .map(String::as_str),
        Some("live")
    );
    assert!(!h.app.session().prompt_values.contains_key("Key"));
}

/// `$response` in the TUI: the first send sends the dependency first; the
/// next send uses the response of the session; the Results say so.
#[test]
fn flow_response_sends_the_dependency_once() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    common::write(
        h.dir.path(),
        "Auth/login.toml",
        "kind = \"http\"\nmethod = \"POST\"\nurl = \"{{base_url}}/login\"\n",
    );
    common::write(
        h.dir.path(),
        "needs-login.toml",
        "kind = \"http\"\nurl = \"{{base_url}}/use?t={{ $response('Auth/login', '$.target') }}\"\n",
    );
    h.app.rescan();
    open(&mut h, "needs-login");
    assert!(!h.screen().contains("undefined"), "{}", h.screen());
    let targets =
        |s: &Servers| -> Vec<String> { s.http.take().iter().map(|c| c.target()).collect() };
    h.key("f5");
    h.wait_for("200", 10);
    assert_eq!(targets(&s), ["/login", "/use?t=/login"]);
    let logs = |h: &Harness| {
        h.app
            .tab()
            .and_then(|t| t.response.as_ref())
            .map(|r| String::from_utf8_lossy(r.messages.doc.bytes()).into_owned())
            .unwrap_or_default()
    };
    assert!(
        logs(&h).contains("dependency Auth/login: sent (200 OK"),
        "{}",
        logs(&h)
    );
    h.key("f5");
    h.wait_idle(10);
    assert_eq!(targets(&s), ["/use?t=/login"]);
    assert!(
        logs(&h).contains("the response of this session"),
        "{}",
        logs(&h)
    );
}

/// The run tab: `:run F --data users.csv` sends the folder once per row with
/// its own runtime values, Up chooses a row, a `$prompt` request is an error
/// row, F5 runs again, and the session does not keep the tab.
#[test]
fn flow_run_tab_runs_a_folder_once_per_row() {
    let s = servers();
    let mut h = harness(&s, 120, 40);
    common::write(
        h.dir.path(),
        "F/1-a.toml",
        "kind = \"http\"\nurl = \"{{base_url}}/{{user}}\"\n\n[[scripts.extract]]\nvar = \"got\"\nfrom = \"jsonpath\"\nexpr = \"$.target\"\n",
    );
    common::write(
        h.dir.path(),
        "F/2-b.toml",
        "kind = \"http\"\nurl = \"{{base_url}}/b{{got}}\"\n",
    );
    common::write(
        h.dir.path(),
        "F/3-c.toml",
        "kind = \"http\"\nurl = \"{{base_url}}/c?otp={{ $prompt('OTP') }}\"\n",
    );
    common::write(h.dir.path(), "users.csv", "user\nana\nbob\n");
    h.app.rescan();
    let targets =
        |s: &Servers| -> Vec<String> { s.http.take().iter().map(|c| c.target()).collect() };
    h.app.run_command("run F --data users.csv");
    h.wait_idle(10);
    assert_eq!(targets(&s), ["/ana", "/b/ana", "/bob", "/b/bob"]);
    let screen = h.wait_for("F/2-b.toml[2]", 5);
    assert!(
        screen.contains("4 passed, 0 failed, 2 error(s)"),
        "{screen}"
    );
    let view = |h: &Harness| {
        let v = h.app.tab().and_then(|t| t.run.as_ref()).unwrap();
        (v.rows.len(), v.sel)
    };
    // The selection followed the last row: the `$prompt` error.
    assert_eq!(view(&h), (6, 5));
    let body = |h: &Harness| {
        h.app
            .tab()
            .and_then(|t| t.response.as_ref())
            .map(|r| String::from_utf8_lossy(r.body.doc.bytes()).into_owned())
            .unwrap_or_default()
    };
    assert!(body(&h).contains("a run tab cannot ask"), "{}", body(&h));
    h.key("up");
    assert_eq!(view(&h), (6, 4));
    assert!(body(&h).contains("/b/bob"), "{}", body(&h));
    // The footer names the keys of a run tab, not those of a request.
    let labels: Vec<String> = h.app.hints().into_iter().map(|(_, l)| l).collect();
    for l in ["run again", "choose row", "show response"] {
        assert!(labels.iter().any(|x| x == l), "{l}: {labels:?}");
    }
    assert!(
        !labels.iter().any(|x| x == "save" || x == "edit"),
        "{labels:?}"
    );
    // The run has its own runtime values.
    assert!(!h.app.runtime.contains_key("got"));
    assert!(h.app.session().tabs.is_empty());
    h.key("f5");
    h.wait_idle(10);
    assert_eq!(targets(&s).len(), 4);
}

/// The target picker lists resources and prompts; a resource pick sets the
/// operation; the second send uses the kept session; `:mcp restart` closes it.
#[test]
fn flow_mcp_resource_pick_and_kept_session() {
    let rt = tokio::runtime::Runtime::new().must();
    let echo = rt.block_on(sendito_testkit::mcp_echo::start()).must();
    let mut h = Harness::with(120, 40, |d| {
        common::demo_collection(d, "http://127.0.0.1:9", "127.0.0.1:9");
    });
    h.app.run_command("newmcp");
    h.app.focus = sendito_ui::app::Focus::Bar(1);
    h.type_text(&format!("{}/mcp", echo.url()));
    h.draw();
    h.app.focus = sendito_ui::app::Focus::Bar(0);
    h.key("enter");
    let screen = h.wait_for("Choose an MCP tool, resource or prompt", 10);
    assert!(screen.contains("prompt · Says hello"), "{screen}");
    h.type_text("readme");
    h.key("enter");
    let ed = &h.app.tabs[h.app.active].ed;
    assert_eq!(ed.mcp_op, sendito_core::McpOp::Resource);
    assert_eq!(ed.mcp_tool.text(), "echo://readme");
    let count = |method: &str| {
        echo.captured()
            .iter()
            .filter(|c| c.method_of_body() == method)
            .count()
    };
    let opened = count("initialize");
    h.key("f5");
    h.wait_idle(10);
    h.key("f5");
    h.wait_idle(10);
    assert_eq!(count("resources/read"), 2);
    assert_eq!(count("initialize"), opened + 1);
    let body = h
        .app
        .tab()
        .and_then(|t| t.response.as_ref())
        .map(|r| String::from_utf8_lossy(r.body.doc.bytes()).into_owned())
        .unwrap_or_default();
    assert!(body.contains("resource echo://readme"), "{body}");
    h.app.run_command("mcp restart");
    h.key("f5");
    h.wait_idle(10);
    assert_eq!(count("initialize"), opened + 2);
}

/// A WebSocket tab in GraphQL mode: the request has `[graphql]` and no
/// messages, and the session subscribes and ends when the server completes.
#[test]
fn flow_ws_graphql_mode_subscribes() {
    use sendito_ui::tab::Field;
    let s = servers();
    let mut h = harness(&s, 120, 40);
    h.app.run_command("newws");
    {
        let t = &mut h.app.tabs[h.app.active];
        t.ed.url.set_text(&s.ws.url("/graphql"));
        t.ed.set_choice(Field::WsMode, 1);
        t.ed.body_text.set_text("subscription { tick }");
        t.refresh_model();
        let sendito_core::Request::Ws(w) = &t.model else {
            panic!("not websocket")
        };
        assert_eq!(w.messages, "[]");
        assert_eq!(
            w.graphql.as_ref().map(|g| g.query.as_str()),
            Some("subscription { tick }")
        );
        assert!(t.fields().contains(&Field::GraphqlSection) || t.req_tab != 1);
    }
    h.key("f5");
    h.wait_idle(10);
    let frames: Vec<String> =
        s.ws.take()
            .iter()
            .flat_map(|c| c.frames.clone())
            .filter(|(out, _, _)| !out)
            .map(|(_, _, p)| String::from_utf8_lossy(&p).into_owned())
            .collect();
    assert!(frames[0].contains("connection_init"), "{frames:?}");
    assert!(frames[1].contains(r#""type":"subscribe""#), "{frames:?}");
    let summary = h
        .app
        .tab()
        .and_then(|t| t.response.as_ref())
        .map(|r| r.summary.status.clone());
    assert!(
        summary.unwrap_or_default().contains("1000"),
        "{:?}",
        h.screen()
    );
}

/// A refused connection: the status line names the cause in a few words and
/// the response pane has the fix.
#[test]
fn flow_http_failure_hint() {
    let s = servers();
    let mut h = harness(&s, 110, 32);
    open(&mut h, "list users");
    {
        let t = &mut h.app.tabs[h.app.active];
        t.ed.url.set_text("http://127.0.0.1:9/users");
        t.refresh_model();
    }
    h.key("f5");
    h.wait_idle(10);
    // The footer cuts the message to half of its free room: read the full
    // text from the app.
    let sc = h.wait_for("✗ connection refused", 5);
    let status = h.app.status.as_ref().map(|s| s.0.as_str());
    assert_eq!(
        status,
        Some("connection refused · details in the response pane"),
        "{sc}"
    );
    let body = h
        .app
        .tab()
        .and_then(|t| t.response.as_ref())
        .map(|r| String::from_utf8_lossy(r.body.doc.bytes()).into_owned())
        .unwrap_or_default();
    assert!(
        body.contains(
            "Fix: Nothing listens at 127.0.0.1:9. Check the port and the URL variables of the local environment."
        ),
        "{body}"
    );
}
