//! GraphQL completion load failures do not cause automatic retries.
mod common;

use common::{Harness, Must};
use sendito_core::BodyKind;
use sendito_ui::app::Focus;

#[test]
fn graphql_completion_failure_requires_explicit_refresh() {
    let mut h = Harness::new(120, 40);
    h.app.run_command("new");
    let t = h.app.tab_mut().must();
    t.ed.url.set_text("http://127.0.0.1:1/graphql");
    t.ed.set_body_kind(BodyKind::GraphQL);
    t.ed.body_text.set_text("{ he");
    t.ed.body_text.move_to_end();
    t.req_tab = 2;
    t.field = 2;
    t.refresh_model();
    h.app.focus = Focus::ReqBody;
    h.key("ctrl+space");
    let start = std::time::Instant::now();
    while h.app.complete.as_ref().is_none_or(|c| c.failed.is_none()) {
        assert!(start.elapsed() < std::time::Duration::from_secs(10));
        std::thread::sleep(std::time::Duration::from_millis(10));
        h.pump();
    }
    h.app.status = None;
    h.key("esc ctrl+space");
    assert!(h.app.complete.as_ref().must().failed.is_some());
    assert!(h.app.complete.as_ref().must().loading.is_none());
    assert!(h.app.status.is_none());
    h.type_text("r");
    assert!(h.app.complete.as_ref().must().failed.is_some());
    assert!(h.app.status.is_none());
    h.key("esc");
    h.app.run_command("graphqlschema");
    assert!(
        h.app
            .status
            .as_ref()
            .is_some_and(|(text, _, _)| text == "loading GraphQL schema")
    );
}
