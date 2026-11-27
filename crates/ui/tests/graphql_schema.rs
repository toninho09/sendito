//! GraphQL schema command and background status.
mod common;

use common::{Harness, Must};
use sendito_core::{BodyKind, Request};
use std::io::{Read, Write};

#[test]
fn graphql_schema_command_loads_in_background() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").must();
    let address = listener.local_addr().must();
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().must();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .must();
        let mut input = Vec::new();
        loop {
            let mut buffer = [0; 4096];
            let n = stream.read(&mut buffer).must();
            assert!(n > 0);
            input.extend_from_slice(&buffer[..n]);
            if let Some(end) = input.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&input[..end]);
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().must())
                    })
                    .unwrap_or(0);
                if input.len() >= end + 4 + length {
                    break;
                }
            }
        }
        let body = r#"{"data":{"__schema":{"queryType":{"name":"Query"},"types":[{"kind":"OBJECT","name":"Query","fields":[{"name":"hero","type":{"kind":"INTERFACE","name":"Character"},"args":[{"name":"id","type":{"kind":"SCALAR","name":"ID"}},{"name":"limit","type":{"kind":"SCALAR","name":"Int"}}]}]},{"kind":"INTERFACE","name":"Character","fields":[{"name":"id","type":{"kind":"SCALAR","name":"ID"}}],"possibleTypes":[{"name":"Hero"}]},{"kind":"OBJECT","name":"Hero","fields":[{"name":"name","type":{"kind":"SCALAR","name":"String"}}]}]}}}"#;
        write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).must();
        String::from_utf8(input).must()
    });
    let mut h = Harness::new(120, 40);
    h.app.run_command("new");
    let tab = h.app.tab_mut().must();
    tab.ed.set_body_kind(BodyKind::GraphQL);
    tab.ed.url.set_text(&format!("http://{address}/graphql"));
    tab.ed.body_text.set_text("{ draft }");
    tab.ed.graphql_variables.set_text("invalid draft");
    tab.refresh_model();
    let before = h.app.tab().must().model.clone();
    h.app.run_command("graphqlschema");
    let start = std::time::Instant::now();
    while start.elapsed() < std::time::Duration::from_secs(10) {
        h.pump();
        if h.app
            .status
            .as_ref()
            .is_some_and(|(text, _, _)| text == "schema loaded: 3 types")
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        h.app
            .status
            .as_ref()
            .is_some_and(|(text, _, _)| text == "schema loaded: 3 types")
    );
    assert_eq!(h.app.tab().must().model, before);
    assert!(matches!(before, Request::Http(_)));
    assert_eq!(h.app.graphql_schemas.lock().must().len(), 1);
    let wire = server.join().must();
    assert!(wire.starts_with("POST /graphql"));
    assert!(!wire.contains("invalid draft"));
    for (query, name, expected) in [
        ("{ he", "hero", "{ hero"),
        ("{ hero(i", "id", "{ hero(id: "),
        ("{ hero(id: 1, li", "limit", "{ hero(id: 1, limit: "),
        ("{ hero { ... on He", "Hero", "{ hero { ... on Hero"),
        (
            "{ hero { ... on Hero { na",
            "name",
            "{ hero { ... on Hero { name",
        ),
    ] {
        let t = h.app.tab_mut().must();
        t.ed.body_text.set_text(query);
        t.ed.body_text.move_to_end();
        t.req_tab = 2;
        t.field = 2;
        t.ed.graphql_section = 0;
        t.refresh_model();
        h.app.focus = sendito_ui::app::Focus::ReqBody;
        h.key("ctrl+space");
        let c = h.app.complete.as_ref().must();
        assert_eq!(c.selected().must().name, name, "{query}");
        h.key("enter");
        assert_eq!(h.app.tab().must().ed.body_text.text(), expected);
    }
    let t = h.app.tab_mut().must();
    t.ed.body_text.set_text("{ hero { id ");
    t.ed.body_text.move_to_end();
    t.refresh_model();
    h.key("ctrl+space");
    assert!(h.app.complete.as_ref().must().items.is_empty());
}

#[test]
fn graphql_schema_command_requires_graphql_body() {
    let mut h = Harness::new(120, 40);
    h.app.run_command("new");
    h.app.run_command("graphqlschema");
    assert!(h.app.status.as_ref().is_some_and(|(text, _, _)| text
        == "GraphQL schema requires a GraphQL body or a GraphQL WebSocket session"));
}
