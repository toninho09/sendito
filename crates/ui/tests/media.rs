//! Generates screenshots and flow recordings (run on demand):
//! `cargo test -p sendito-ui --test media -- --ignored`, then
//! `python3 docs/media/render.py`. Frames are the app's real styled output.

mod common;

use std::path::PathBuf;

use common::{Harness, Must};
use sendito_testkit::{grpc_echo, http_echo};

fn out_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/media")
}

struct Recorder {
    name: String,
    frames: Vec<(String, u32)>,
}

impl Recorder {
    fn new(name: &str) -> Self {
        Self {
            name: name.into(),
            frames: Vec::new(),
        }
    }

    /// Captures the current screen; `ms` is how long it is shown in the recording.
    fn shot(&mut self, h: &mut Harness, caption: &str, ms: u32) {
        h.draw();
        let json = common::buffer_json(h.term.backend().buffer());
        let cursor = match h.app.last_cursor {
            Some((x, y)) => format!("[{x}, {y}]"),
            None => "null".into(),
        };
        self.frames.push((
            format!(
                "{{\"caption\": {}, \"ms\": {ms}, \"cursor\": {cursor}, \"screen\": {json}}}",
                serde_json::Value::String(caption.into())
            ),
            ms,
        ));
    }

    fn save(&self) {
        let dir = out_dir().join("frames");
        std::fs::create_dir_all(&dir).must();
        let body = self
            .frames
            .iter()
            .map(|(j, _)| j.clone())
            .collect::<Vec<_>>()
            .join(",\n");
        std::fs::write(
            dir.join(format!("{}.json", self.name)),
            format!("[\n{body}\n]\n"),
        )
        .must();
    }
}

struct Servers {
    http: http_echo::HttpEcho,
    grpc: grpc_echo::GrpcEcho,
    mqtt: sendito_testkit::mqtt_broker::MqttBroker,
    _rt: tokio::runtime::Runtime,
}

fn servers() -> Servers {
    let rt = tokio::runtime::Runtime::new().must();
    let (http, grpc, mqtt) = rt.block_on(async {
        (
            http_echo::start().await.must(),
            grpc_echo::start().await.must(),
            sendito_testkit::mqtt_broker::MqttBroker::start()
                .await
                .must(),
        )
    });
    Servers {
        http,
        grpc,
        mqtt,
        _rt: rt,
    }
}

fn harness(s: &Servers, w: u16, h: u16, theme: &str) -> Harness {
    let (hu, gt) = (s.http.url(), s.grpc.target());
    let mut h = Harness::with(w, h, |d| common::demo_collection(d, &hu, &gt));
    // Full color whatever the terminal running the test supports.
    h.app.theme = sendito_ui::theme::Registry::builtin().theme_full(theme);
    h.app.set_env(Some("local".into()));
    h.app.status = None;
    h
}

fn open(h: &mut Harness, name: &str) {
    h.key("ctrl+w h");
    h.app.tree.set_filter(name);
    h.key("enter");
    h.app.tree.set_filter("");
    h.draw();
}

fn type_slow(h: &mut Harness, r: &mut Recorder, text: &str, caption: &str) {
    let mut acc = String::new();
    for (i, ch) in text.chars().enumerate() {
        acc.push(ch);
        h.type_text(&ch.to_string());
        if i % 4 == 3 || i + 1 == text.chars().count() {
            r.shot(h, caption, 120);
        }
    }
}

#[test]
#[ignore]
fn screenshots() {
    let s = servers();
    for theme in ["dusk", "daylight"] {
        for (w, ht) in [(120u16, 40u16), (200, 60)] {
            let mut r = Recorder::new(&format!("screens-{theme}-{w}x{ht}"));
            let mut h = harness(&s, w, ht, theme);
            r.shot(&mut h, "welcome", 0);
            open(&mut h, "create user");
            h.key("f5");
            h.wait_idle(10);
            h.key("ctrl+w k alt+3 enter down");
            r.shot(&mut h, "request-body-and-response", 0);
            h.key("ctrl+w j");
            r.shot(&mut h, "response-focused", 0);
            h.key("alt+3");
            r.shot(&mut h, "timing", 0);
            open(&mut h, "echo unary");
            h.key("f5");
            h.wait_idle(10);
            h.key("ctrl+w j alt+4");
            r.shot(&mut h, "grpc-unary", 0);
            h.key("ctrl+p");
            r.shot(&mut h, "palette", 0);
            h.key("esc f1");
            r.shot(&mut h, "help", 0);
            h.key("esc ctrl+w V");
            r.shot(&mut h, "environments", 0);
            h.key("esc");
            h.app.new_tab(sendito_core::Request::new_http());
            let t = h.app.tab_mut().must();
            t.ed.method = "POST".into();
            t.ed.url.set_text("https://example.test/graphql");
            t.ed.set_body_kind(sendito_core::BodyKind::GraphQL);
            t.ed.body_text
                .set_text("query Lookup($id: ID!) {\n  hero(id: $id) {\n    name\n  }\n}");
            t.ed.graphql_variables.set_text("{\"id\": \"42\"}");
            t.ed.graphql_operation.set_text("Lookup");
            t.req_tab = 2;
            t.field = 2;
            t.refresh_model();
            h.app.focus = sendito_ui::app::Focus::ReqBody;
            r.shot(&mut h, "graphql-body", 0);
            let env = h.app.active_env.clone();
            let t = h.app.tab_mut().must();
            t.ed.body_text.set_text("query Lookup {\n  hero(");
            t.ed.body_text.move_to_end();
            t.graphql_schema = Some(
                sendito_pipeline::http::graphql::GraphqlSchema::from_json(include_bytes!(
                    "fixtures/graphql-schema.json"
                ))
                .must(),
            );
            t.graphql_source = format!("{env:?}:graphql:POST https://example.test/graphql");
            t.refresh_model();
            h.key("ctrl+space");
            r.shot(&mut h, "graphql-completion", 0);
            h.key("esc");
            common::sse::demo(&mut h);
            r.shot(&mut h, "sse-options", 0);
            h.key("ctrl+w j");
            r.shot(&mut h, "sse-events", 0);
            h.key("alt+4");
            r.shot(&mut h, "sse-log", 0);
            common::mqtt::demo(&mut h);
            r.shot(&mut h, "mqtt-topics", 0);
            h.key("ctrl+w j");
            r.shot(&mut h, "mqtt-messages", 0);
            h.key("alt+4");
            r.shot(&mut h, "mqtt-log", 0);
            r.save();
        }
    }
    // A gallery: the same screens in the other built-in themes and with
    // Nerd Font icons.
    for (theme, nerd) in [
        ("ember", false),
        ("linen", false),
        ("contrast", false),
        ("ansi", false),
        ("dusk", true),
    ] {
        let name = if nerd {
            format!("screens-{theme}-120x40-nerd")
        } else {
            format!("screens-{theme}-120x40")
        };
        let mut r = Recorder::new(&name);
        let mut h = harness(&s, 120, 40, theme);
        if nerd {
            h.app.icons = sendito_ui::icons::Icons::nerd();
        }
        r.shot(&mut h, "welcome", 0);
        open(&mut h, "create user");
        h.key("f5");
        h.wait_idle(10);
        h.key("ctrl+w k alt+3 enter down");
        r.shot(&mut h, "request-body-and-response", 0);
        h.key("ctrl+w j alt+3");
        r.shot(&mut h, "timing", 0);
        h.key("ctrl+p");
        r.shot(&mut h, "palette", 0);
        h.key("esc");
        r.save();
    }
}

#[test]
#[ignore]
fn sse_recording() {
    let s = servers();
    let mut h = harness(&s, 120, 40, "dusk");
    let mut r = Recorder::new("13-sse-session");
    h.app.run_command("newsse");
    let t = h.app.tab_mut().must();
    t.ed.url.set_text(&format!("{}/sse/slow", s.http.url()));
    t.ed.deadline.set_text("5000");
    t.refresh_model();
    r.shot(&mut h, "Create an SSE request", 1200);
    h.key("f5");
    h.wait_for("streaming", 10);
    h.key("ctrl+w j");
    for _ in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(120));
        h.pump();
        r.shot(&mut h, "Receive events in the live feed", 500);
    }
    h.key("[");
    r.shot(&mut h, "Select an earlier event with [", 1200);
    h.key("]");
    h.app.run_command("end");
    h.wait_idle(10);
    r.shot(&mut h, "End the session with :end", 1800);
    r.save();
}

#[test]
#[ignore]
fn mqtt_recording() {
    let s = servers();
    let mut h = harness(&s, 120, 40, "dusk");
    let mut r = Recorder::new("14-mqtt-session");
    h.app.run_command("newmqtt");
    let text = |s: &str| sendito_ui::form::Cell::Text(sendito_ui::editor::TextEditor::single(s));
    let t = h.app.tab_mut().must();
    t.ed.url.set_text(&s.mqtt.url());
    t.ed.mqtt_client_id.set_text("sendito-demo");
    t.ed.mqtt_subs.push(
        true,
        vec![text("demo/#"), sendito_ui::form::Cell::Choice(1)],
    );
    t.ed.mqtt_publish.push(
        true,
        vec![
            text("demo/hello"),
            text("{\"hello\":\"mqtt\"}"),
            sendito_ui::form::Cell::Choice(1),
            sendito_ui::form::Cell::Toggle(false),
            sendito_ui::form::Cell::Toggle(false),
        ],
    );
    t.refresh_model();
    r.shot(&mut h, "Create an MQTT request: subscribe to demo/#", 1200);
    h.key("f5");
    h.wait_for("connected", 10);
    h.key("ctrl+w j");
    std::thread::sleep(std::time::Duration::from_millis(150));
    h.pump();
    r.shot(
        &mut h,
        "The initial publish comes back on the subscription",
        1200,
    );
    for _ in 0..2 {
        h.app.run_command("send");
        std::thread::sleep(std::time::Duration::from_millis(150));
        h.pump();
        r.shot(&mut h, "Send publishes the selected row live", 700);
    }
    h.key("alt+4");
    r.shot(&mut h, "The Log tab lists every packet", 1200);
    h.app.run_command("end");
    h.wait_idle(10);
    r.shot(&mut h, "Disconnect with :end", 1800);
    r.save();
}

#[test]
#[ignore]
fn recordings() {
    let s = servers();
    let (w, ht) = (120u16, 40u16);

    // 1. Create a folder and a request.
    let mut r = Recorder::new("01-create-folder-and-request");
    let mut h = harness(&s, w, ht, "dusk");
    r.shot(
        &mut h,
        "The collection is open; the Requests sidebar is focused",
        1200,
    );
    h.key("ctrl+w h N");
    r.shot(&mut h, "N: new folder", 900);
    type_slow(&mut h, &mut r, "Payments", "Type the folder name");
    h.key("tab");
    while h.app.overlay.as_ref().is_some_and(
        |o| matches!(o, sendito_ui::overlay::Overlay::Prompt(p) if p.choices[0].2 != 0),
    ) {
        h.key("left");
    }
    r.shot(
        &mut h,
        "Tab to the folder row, ←/→ to choose the collection root",
        1200,
    );
    h.key("enter");
    assert!(h.dir.path().join("Payments").is_dir());
    r.shot(&mut h, "Enter: the folder is created on disk", 1200);
    h.key("n");
    r.shot(
        &mut h,
        "n: new request in the selected folder (HTTP or gRPC)",
        1200,
    );
    type_slow(&mut h, &mut r, "charge card", "Type the request name");
    h.key("enter");
    r.shot(&mut h, "The request opens with the URL field focused", 1200);
    type_slow(
        &mut h,
        &mut r,
        "{{base_url}}/status/201",
        "Type the URL; {{variables}} are highlighted",
    );
    h.key("ctrl+s");
    r.shot(&mut h, "Ctrl+S saves Payments/charge card.toml", 1800);
    r.save();

    // 2. Send GET, then POST JSON.
    let mut r = Recorder::new("02-send-get-and-post-json");
    let mut h = harness(&s, w, ht, "dusk");
    open(&mut h, "list users");
    r.shot(&mut h, "Open \"list users\" from the tree (Enter)", 1200);
    h.key("f5");
    h.wait_idle(10);
    r.shot(
        &mut h,
        "F5 / Ctrl+Enter sends; the response shows status, time and size",
        1800,
    );
    open(&mut h, "create user");
    h.key("alt+3");
    r.shot(
        &mut h,
        "Alt+3: the Body tab of \"create user\" (JSON)",
        1200,
    );
    h.key("enter down ctrl+a");
    r.shot(
        &mut h,
        "Enter goes into the editor; Ctrl+A selects the whole body",
        1200,
    );
    type_slow(
        &mut h,
        &mut r,
        "{\"name\": \"Zé\", \"email\": \"ze@example.com\"}",
        "Enter on the Body tab goes into the editor; Ctrl+A selects all, typing replaces it",
    );
    h.key("f5");
    h.wait_idle(10);
    r.shot(
        &mut h,
        "Send: the echo server shows the exact request it received",
        1800,
    );
    h.key("ctrl+w j alt+4");
    r.shot(&mut h, "Results tab: the request's assertion passed", 1800);
    r.save();

    // 3. Multipart upload.
    let mut r = Recorder::new("03-multipart-upload");
    let mut h = harness(&s, w, ht, "dusk");
    std::fs::write(h.dir.path().join("report.txt"), "quarterly numbers\n").must();
    h.key("ctrl+n");
    type_slow(
        &mut h,
        &mut r,
        "{{base_url}}/upload",
        "Ctrl+N: new request; type the URL",
    );
    h.key("shift+tab enter");
    h.type_text("POST");
    r.shot(&mut h, "Shift+Tab to the method, Enter: pick POST", 1000);
    h.key("enter");
    assert!(h.app.tab().is_some_and(|t| t.ed.method == "POST"));
    h.key("alt+3 enter");
    r.shot(
        &mut h,
        "Body tab: Enter on \"Body type\" opens the picker",
        1200,
    );
    h.type_text("multi");
    r.shot(&mut h, "Type to filter", 900);
    h.key("enter down");
    r.shot(&mut h, "Multipart parts table", 1000);
    type_slow(&mut h, &mut r, "title", "Part name");
    h.key("enter enter");
    type_slow(
        &mut h,
        &mut r,
        "Q3",
        "Enter moves to the next cell; type the value",
    );
    h.key("down");
    type_slow(&mut h, &mut r, "report", "Next row: its name");
    h.key("enter");
    r.shot(
        &mut h,
        "On the Kind cell ‹text›, the hint row says: Space changes it",
        1200,
    );
    h.key("space");
    r.shot(&mut h, "Space: the part is now a file part", 1000);
    h.key("enter");
    type_slow(
        &mut h,
        &mut r,
        "report.txt",
        "File path (relative to the collection)",
    );
    h.key("f5");
    h.wait_idle(10);
    h.key("ctrl+w j");
    r.shot(
        &mut h,
        "Sent as multipart/form-data with the file contents",
        2000,
    );
    h.key("ctrl+s");
    r.shot(
        &mut h,
        "Ctrl+S on an unsaved request asks for a name and a folder",
        1500,
    );
    type_slow(&mut h, &mut r, "upload report", "Type the name");
    h.key("enter");
    assert!(h.app.tab().is_some_and(|t| t.path.is_some()));
    r.shot(
        &mut h,
        "Enter: saved as a file; the tab is no longer unsaved",
        1800,
    );
    r.save();

    // 4. Switch environment.
    let mut r = Recorder::new("04-switch-env");
    let mut h = harness(&s, w, ht, "dusk");
    open(&mut h, "get order");
    r.shot(&mut h, "{{order_id}} is red: it is undefined", 1500);
    h.key("f4");
    r.shot(&mut h, "F4: choose the environment", 1200);
    h.type_text("stag");
    h.key("enter");
    r.shot(
        &mut h,
        "Switched to \"staging\" (status line and tab bar)",
        1200,
    );
    h.key("f5");
    r.shot(&mut h, "Sending with unresolved variables asks first", 1800);
    h.key("esc ctrl+w V");
    r.shot(
        &mut h,
        "Ctrl+W V: environments, sources and masked secrets",
        2000,
    );
    h.type_text("x");
    r.shot(
        &mut h,
        "Editing marks the dialog \"unsaved\" (Ctrl+S saves)",
        1800,
    );
    h.key("esc");
    r.shot(
        &mut h,
        "Esc with unsaved edits asks: Save, Discard or Cancel",
        1800,
    );
    h.key("d");
    h.key("ctrl+p");
    type_slow(&mut h, &mut r, ":env local", "Or the palette: :env local");
    h.key("enter");
    r.shot(&mut h, "Switched to \"local\"", 1500);
    r.save();

    // 5. Unary gRPC via reflection.
    let mut r = Recorder::new("05-grpc-unary-reflection");
    let mut h = harness(&s, w, ht, "dusk");
    open(&mut h, "echo unary");
    r.shot(&mut h, "A gRPC request: target, method and message", 1500);
    // Opening a gRPC request focuses the method chip; Enter lists the schema.
    h.key("enter");
    let _ = h.wait_for("Choose a gRPC method", 5);
    r.shot(
        &mut h,
        "The method picker lists services from server reflection",
        1800,
    );
    h.key("esc f5");
    h.wait_idle(10);
    let _ = h.wait_for("OK (0)", 5);
    r.shot(&mut h, "Send: the response message, status OK", 1800);
    h.key("ctrl+w j alt+4");
    r.shot(
        &mut h,
        "Log: every message sent and received, with timestamps",
        1800,
    );
    r.save();

    // 6. Bidi stream.
    let mut r = Recorder::new("06-grpc-bidi-stream");
    let mut h = harness(&s, w, ht, "dusk");
    h.key("ctrl+p");
    type_slow(&mut h, &mut r, ":newgrpc", "Palette: :newgrpc");
    h.key("enter");
    type_slow(&mut h, &mut r, &s.grpc.target(), "Type the target");
    h.key("tab enter");
    let _ = h.wait_for("Choose a gRPC method", 5);
    h.type_text("Echo/Bidi");
    r.shot(&mut h, "Pick Echo/Bidi", 1200);
    h.key("enter");
    let _ = h.wait_for("\"text\": \"\"", 5);
    r.shot(&mut h, "A template is generated from the schema", 1500);
    h.key("alt+1 enter ctrl+a");
    type_slow(
        &mut h,
        &mut r,
        "{\"text\": \"one\"}",
        "Ctrl+A selects the template; type the first message",
    );
    h.key("f5");
    let _ = h.wait_for("bidi stream open", 5);
    h.key("ctrl+w j alt+4");
    r.shot(
        &mut h,
        "The stream is open; messages are logged as they arrive",
        1500,
    );
    h.key("ctrl+w k");
    h.app.focus = sendito_ui::app::Focus::ReqBody;
    h.key("ctrl+a");
    type_slow(
        &mut h,
        &mut r,
        "{\"text\": \"two\"}",
        "Edit the next message",
    );
    h.key("f5");
    std::thread::sleep(std::time::Duration::from_millis(150));
    h.pump();
    r.shot(
        &mut h,
        "F5 sends it on the open stream; the sent text stays selected, so typing replaces it",
        1800,
    );
    h.key("ctrl+w d");
    h.wait_idle(10);
    r.shot(
        &mut h,
        "Ctrl+W d ends the request stream; the request finishes with OK",
        2000,
    );
    r.save();

    // 7. Postman import.
    let mut r = Recorder::new("07-postman-import");
    let mut h = harness(&s, w, ht, "dusk");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/fixtures/acme.postman_collection.json");
    h.key("ctrl+w i");
    r.shot(
        &mut h,
        "Ctrl+W i: import a Postman collection or a curl command",
        1500,
    );
    std::fs::copy(&fixture, h.dir.path().join("acme.postman_collection.json")).must();
    h.paste("acme.postman_collection.json");
    r.shot(
        &mut h,
        "Type or paste the path of the collection file",
        1200,
    );
    h.key("f5");
    r.shot(
        &mut h,
        "Report: counts and everything not imported as-is",
        2500,
    );
    h.key("esc");
    r.shot(
        &mut h,
        "Esc closes the report; the status line says what was imported",
        1800,
    );
    h.key("ctrl+w h");
    h.app.tree.select_rel("Acme API/Users/Admin");
    h.key("right");
    r.shot(&mut h, "The requests are files in the tree", 1800);
    r.save();

    // 8. curl paste and export.
    let mut r = Recorder::new("08-curl-paste-and-export");
    let mut h = harness(&s, w, ht, "dusk");
    h.key("ctrl+n");
    r.shot(
        &mut h,
        "Ctrl+N, then paste a curl command into the URL field",
        1200,
    );
    h.paste(&format!("curl -X POST '{}/items?x=1' -H 'X-Token: abc' -H 'Content-Type: application/json' --data-raw '{{\"a\":1}}'", s.http.url()));
    r.shot(
        &mut h,
        "It becomes a request (method, URL, headers, body)",
        1800,
    );
    h.key("f5");
    h.wait_idle(10);
    r.shot(&mut h, "Send it", 1200);
    h.key("ctrl+w u");
    r.shot(&mut h, "Ctrl+W u copies it back as curl", 1800);
    h.key("esc");
    open(&mut h, "list users");
    r.shot(
        &mut h,
        "Open \"list users\" (its Authorization uses the secret {{token}})",
        1200,
    );
    h.key("ctrl+w u");
    r.shot(
        &mut h,
        "Secrets stay {{placeholders}} in exports (:curl! includes them)",
        2200,
    );
    r.save();

    // 9. History resend.
    let mut r = Recorder::new("09-history-resend");
    let mut h = harness(&s, w, ht, "dusk");
    open(&mut h, "list users");
    h.key("f5");
    h.wait_idle(10);
    open(&mut h, "health");
    h.key("f5");
    h.wait_idle(10);
    h.key("f7");
    for _ in 0..200 {
        if h.app.history.as_ref().is_some_and(|v| v.len() >= 2) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.pump();
    }
    r.shot(
        &mut h,
        "F7: history, newest first (secret values are never stored)",
        1800,
    );
    h.key("down");
    r.shot(&mut h, "Select an entry", 900);
    h.key("f5");
    h.wait_idle(10);
    r.shot(&mut h, "F5 resends it in a new tab", 1800);
    h.key("f7");
    h.key("s");
    r.shot(&mut h, "s saves an entry back to the tree", 1800);
    h.key("esc");
    r.save();

    // 10. Vim-mode editing.
    let mut r = Recorder::new("10-vim-mode-editing");
    let mut h = harness(&s, w, ht, "dusk");
    h.key("ctrl+w ,");
    r.shot(&mut h, "Ctrl+W ,: settings", 1200);
    h.key("down down down");
    h.key("enter");
    r.shot(&mut h, "Vim mode on", 1000);
    h.key("esc");
    open(&mut h, "delete user");
    r.shot(&mut h, "Editors start in NORMAL mode (status line)", 1500);
    h.key("A");
    r.shot(&mut h, "A: append (INSERT)", 900);
    type_slow(&mut h, &mut r, "/v2", "Type");
    h.key("esc");
    r.shot(&mut h, "Esc: back to NORMAL", 900);
    h.type_text("0w");
    r.shot(
        &mut h,
        "0, w: word motions ({{base_url}} counts as one word)",
        1200,
    );
    h.type_text("0dw");
    r.shot(&mut h, "0, dw: deletes the word {{base_url}}", 1000);
    h.type_text("u");
    r.shot(&mut h, "u undoes", 1000);
    h.type_text(":w");
    r.shot(&mut h, ": opens the palette in command mode", 1000);
    h.key("enter");
    r.shot(&mut h, ":w saved the file", 1500);
    h.key("ctrl+w h");
    h.type_text("jj");
    r.shot(&mut h, "hjkl in the tree", 1200);
    r.save();

    // 11. Mouse drag in the tree.
    let mut r = Recorder::new("11-mouse-drag-in-tree");
    let mut h = harness(&s, w, ht, "dusk");
    r.shot(&mut h, "Drag \"health\" onto Orders/", 1200);
    let from = h.find("health").must();
    let to = h.find("Orders/").must();
    h.draw();
    let ev = |kind, (x, y): (u16, u16)| {
        crossterm::event::Event::Mouse(crossterm::event::MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        })
    };
    h.app.on_event(ev(
        crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
        from,
    ));
    h.draw();
    h.app.on_event(ev(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        (to.0, to.1 + 1),
    ));
    h.draw();
    h.app.on_event(ev(
        crossterm::event::MouseEventKind::Drag(crossterm::event::MouseButton::Left),
        to,
    ));
    r.shot(&mut h, "The drop target is highlighted", 1500);
    h.app.on_event(ev(
        crossterm::event::MouseEventKind::Up(crossterm::event::MouseButton::Left),
        to,
    ));
    h.pump();
    r.shot(&mut h, "moved to Orders/health.toml on disk", 1500);
    let (x, y) = h.find("Orders/").must();
    h.click(x, y);
    r.shot(&mut h, "Click a folder to expand it", 1000);
    let pos = h.find("health").must();
    h.double_click(pos.0 + 1, pos.1);
    r.shot(&mut h, "Double-click opens the request", 1200);
    let send = h.find(" Send").must();
    h.click(send.0 + 1, send.1);
    h.wait_idle(10);
    r.shot(&mut h, "Click Send", 1200);
    let bx = h.app.areas.sidebar.x + h.app.areas.sidebar.width - 1;
    h.drag((bx, 12), (bx + 10, 12));
    r.shot(&mut h, "Drag the sidebar border to resize", 1500);
    r.save();

    // 12. Failures, cancel and delete.
    let mut r = Recorder::new("12-errors-cancel-delete");
    let mut h = harness(&s, w, ht, "dusk");
    h.key("ctrl+n");
    h.type_text("{{base_url}}/status/404");
    h.key("f5");
    h.wait_idle(10);
    r.shot(
        &mut h,
        "A 404 response: the status is shown in the warning color",
        1800,
    );
    h.key("ctrl+n");
    h.type_text("http://127.0.0.1:1/unreachable");
    h.key("f5");
    h.wait_idle(10);
    r.shot(
        &mut h,
        "A connection error: the reason is shown in the response pane",
        1800,
    );
    h.key("ctrl+n");
    h.type_text("{{base_url}}/delay/5000");
    h.key("f5");
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.pump();
    }
    r.shot(&mut h, "A slow request is running", 1500);
    h.key("ctrl+w x");
    h.wait_idle(10);
    r.shot(&mut h, "Ctrl+W x cancels it", 1500);
    h.key("ctrl+w h");
    h.app.tree.select_rel("health.toml");
    h.draw();
    h.key("delete");
    r.shot(&mut h, "Delete in the tree asks first", 1800);
    h.key("esc");
    r.shot(&mut h, "Esc keeps the request", 1000);
    h.key("ctrl+w c");
    r.shot(
        &mut h,
        "Closing a tab with unsaved changes asks: save, discard or cancel",
        1800,
    );
    h.key("esc");
    r.shot(&mut h, "Cancel keeps the tab open", 1000);
    r.save();
}
