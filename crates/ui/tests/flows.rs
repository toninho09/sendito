//! Scripted key/mouse sequences for the UX flows, asserting real outcomes
//! (files on disk, what the servers received, what the screen shows).

mod common;

use common::{Harness, Must};
use sendito_testkit::{grpc_echo, http_echo};

struct Servers {
    http: http_echo::HttpEcho,
    grpc: grpc_echo::GrpcEcho,
    _rt: tokio::runtime::Runtime,
}

fn servers() -> Servers {
    let rt = tokio::runtime::Runtime::new().must();
    let (http, grpc) = rt.block_on(async {
        (
            http_echo::start().await.must(),
            grpc_echo::start().await.must(),
        )
    });
    Servers {
        http,
        grpc,
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
    h.app.focus = sendito_ui::app::Focus::Bar(1);
    h.key("enter");
    let sc = h.wait_for("Choose a gRPC method", 5);
    assert!(sc.contains("sendito.test.v1.Echo/Bidi"), "{sc}");
    h.type_text("ServerStream");
    h.key("enter");
    assert_eq!(h.app.tabs[h.app.active].ed.grpc_method, "ServerStream");
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
    // Vim mode is the third setting (after Theme and Icons).
    h.key("down down enter esc");
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
    assert_eq!(rgb(h.app.theme.accent), rgb(Color::Rgb(0x33, 0x66, 0x99)));
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
    assert_eq!(rgb(h.app.theme.accent), rgb(Color::Rgb(0, 0x44, 0)));
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
