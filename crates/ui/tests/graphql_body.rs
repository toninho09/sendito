//! GraphQL body editing and persistence.
mod common;

use common::{Harness, Must};
use sendito_core::{Body, BodyKind, Request};
use sendito_ui::app::Focus;
use sendito_ui::tab::Field;

#[test]
fn graphql_body_sections_keep_independent_text() {
    let mut h = Harness::new(120, 40);
    h.app.run_command("new");
    h.app.run_command("bodytype");
    h.type_text("GraphQL");
    h.key("enter");
    assert_eq!(h.app.tab().must().ed.body_kind, BodyKind::GraphQL);
    {
        let t = h.app.tab_mut().must();
        t.req_tab = 2;
        t.field = 2;
    }
    h.app.focus = Focus::ReqBody;
    h.paste("query Lookup { hero { name } }");
    {
        let t = h.app.tab_mut().must();
        t.ed.set_choice(Field::GraphqlSection, 1);
    }
    h.paste("{\"id\":1}");
    assert!(h.screen().contains("✓ JSON"));
    {
        let t = h.app.tab_mut().must();
        t.ed.set_choice(Field::GraphqlSection, 2);
    }
    h.paste("Lookup");
    let model = &h.app.tab().must().model;
    assert!(
        matches!(model, Request::Http(r) if r.body == Body::GraphQL {
            query: "query Lookup { hero { name } }".into(),
            variables: "{\"id\":1}".into(),
            operation_name: "Lookup".into(),
        })
    );
    let text = sendito_store::request_file::to_toml(model);
    assert_eq!(sendito_store::request_file::parse(&text).must(), *model);
    let t = h.app.tab_mut().must();
    t.ed.set_body_kind(BodyKind::Json);
    let json = t.ed.body_text.text();
    assert!(json.contains("operationName"));
    t.ed.set_body_kind(BodyKind::GraphQL);
    t.refresh_model();
    assert_eq!(t.ed.graphql_variables.text(), "{\"id\":1}");
}
