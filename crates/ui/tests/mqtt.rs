//! MQTT editor, live messages, publish, disconnect, cancel and export.
mod common;
use common::{Harness, Must};
use sendito_ui::editor::TextEditor;
use sendito_ui::form::Cell;
use sendito_ui::tab::Kind;

fn text(s: &str) -> Cell {
    Cell::Text(TextEditor::single(s))
}

/// A new MQTT tab on the test broker: one subscription on `ui/#`.
fn session(h: &mut Harness, url: &str) {
    h.key("ctrl+w M");
    assert_eq!(h.app.tab().must().ed.kind, Kind::Mqtt);
    let t = h.app.tab_mut().must();
    t.ed.url.set_text(url);
    t.ed.mqtt_client_id.set_text("ui-test");
    t.ed.mqtt_subs
        .push(true, vec![text("ui/#"), Cell::Choice(1)]);
    t.ed.mqtt_publish.push(
        true,
        vec![
            text("ui/live"),
            text("{\"n\":1}"),
            Cell::Choice(1),
            Cell::Toggle(false),
            Cell::Toggle(false),
        ],
    );
    t.refresh_model();
}

#[test]
fn mqtt_saved_request_and_live_messages() {
    let mut h = Harness::new(120, 40);
    let broker =
        h.rt.block_on(sendito_testkit::mqtt_broker::MqttBroker::start())
            .must();
    session(&mut h, &broker.url());
    let t = h.app.tab_mut().must();
    t.ed.deadline.set_text("400");
    t.ed.will_topic.set_text("ui/status");
    t.ed.will_payload.set_text("offline");
    t.refresh_model();
    let text = sendito_store::request_file::to_toml(&t.model);
    assert!(text.starts_with("kind = \"mqtt\""), "{text}");
    assert!(
        text.contains("[[subscriptions]]\ntopic = \"ui/#\"\nqos = 1"),
        "{text}"
    );
    assert!(
        text.contains("[last_will]\ntopic = \"ui/status\""),
        "{text}"
    );
    // The editors read back what they wrote.
    let back = sendito_ui::tab::Editors::from_request(&t.model);
    assert_eq!(back.to_request(), t.model);
    h.key("f5");
    h.wait_idle(10);
    let response = h.app.tab().must().response.as_ref().must();
    assert!(response.summary.ok, "{:?}", response.summary);
    assert_eq!(response.summary.status, "1 messages · deadline");
    assert_eq!(response.mqtt_messages.len(), 1);
    assert_eq!(response.messages().last().must().text, "{\"n\":1}");
    assert!(response.messages.doc.raw.text.contains("subscribed ui/#"));
}

#[test]
fn mqtt_publish_disconnect_and_cancel() {
    for command in ["end", "cancel"] {
        let mut h = Harness::new(120, 40);
        let broker =
            h.rt.block_on(sendito_testkit::mqtt_broker::MqttBroker::start())
                .must();
        session(&mut h, &broker.url());
        h.key("f5");
        h.wait_for("connected", 10);
        // Send on a running session publishes the selected Publish row.
        h.app.run_command("send");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while h
            .app
            .tab()
            .and_then(|t| t.response.as_ref())
            .is_none_or(|r| r.mqtt_messages.len() < 2)
            && std::time::Instant::now() < deadline
        {
            h.pump();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let response = h.app.tab().must().response.as_ref().must();
        assert_eq!(response.mqtt_messages.len(), 2, "initial and live publish");
        h.app.run_command(command);
        h.wait_idle(10);
        let response = h.app.tab().must().response.as_ref().must();
        let expected = if command == "end" {
            "disconnected"
        } else {
            "cancelled"
        };
        assert!(
            response.summary.status.contains(expected),
            "{}",
            response.summary.status
        );
    }
}

#[test]
fn mqtt_export_is_refused() {
    let mut h = Harness::new(120, 40);
    session(&mut h, "mqtt://127.0.0.1:1");
    h.app.clipboard.text.clear();
    h.app.run_command("curl");
    assert!(h.app.clipboard.text.is_empty());
    let (status, _, _) = h.app.status.as_ref().must();
    assert!(status.contains("not available for MQTT"), "{status}");
}
