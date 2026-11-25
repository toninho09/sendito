//! SSE live-session editor, event feed and controls.
mod common;
use common::{Harness, Must};
use sendito_ui::tab::Kind;

#[test]
fn sse_live_feed_is_bounded_and_saved_as_sse() {
    let mut h = Harness::new(120, 40);
    let server = h.rt.block_on(sendito_testkit::http_echo::start()).must();
    h.key("ctrl+w E");
    assert_eq!(h.app.tab().must().ed.kind, Kind::Sse);
    h.app.project.config.ui.stream_messages_kept = 2;
    let t = h.app.tab_mut().must();
    t.ed.url.set_text(&format!("{}/sse/events", server.url()));
    t.ed.max_events.set_text("3");
    t.refresh_model();
    let text = sendito_store::request_file::to_toml(&t.model);
    assert!(text.starts_with("kind = \"sse\""));
    assert!(text.contains("max_events = 3"));
    h.key("f5");
    h.wait_idle(10);
    let response = h.app.tab().must().response.as_ref().must();
    assert_eq!(response.stream.received, 3);
    assert_eq!(response.stream.dropped, 1);
    assert_eq!(response.sse_events.len(), 2);
    assert!(response.summary.ok, "{:?}", response.summary);
    assert_eq!(response.messages().last().must().text, "{\"n\":3}");
    h.app.run_command("curl");
    assert!(h.app.clipboard.text.contains("curl -N"));
}

#[test]
fn sse_end_and_cancel_finish_the_session() {
    for command in ["end", "cancel"] {
        let mut h = Harness::new(120, 40);
        let server = h.rt.block_on(sendito_testkit::http_echo::start()).must();
        h.app.run_command("newsse");
        h.app
            .tab_mut()
            .must()
            .ed
            .url
            .set_text(&format!("{}/sse/slow", server.url()));
        h.app.tab_mut().must().refresh_model();
        h.key("f5");
        h.wait_for("streaming", 10);
        h.app.run_command(command);
        h.wait_idle(10);
        let response = h.app.tab().must().response.as_ref().must();
        assert!(
            response.summary.status.contains(if command == "end" {
                "ended"
            } else {
                "cancelled"
            }),
            "{}",
            response.summary.status
        );
    }
}
