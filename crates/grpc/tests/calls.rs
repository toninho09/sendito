use std::time::Duration;

use sendito_grpc::schema::{from_reflection, parse_messages, template_for};
use sendito_grpc::{CallKind, ConnSettings, Event, Schema, connect, metadata_map, start_call};
use sendito_testkit::grpc_echo;

async fn collect(mut call: sendito_grpc::Call) -> Vec<Event> {
    let mut out = Vec::new();
    while let Some(e) = call.events.recv().await {
        let done = matches!(e, Event::Finished { .. });
        out.push(e);
        if done {
            break;
        }
    }
    out
}

fn finished(ev: &[Event]) -> (i32, String) {
    ev.iter()
        .find_map(|e| match e {
            Event::Finished { code, message, .. } => Some((*code, message.clone())),
            _ => None,
        })
        .unwrap_or((-1, String::new()))
}

fn received(ev: &[Event]) -> Vec<serde_json::Value> {
    ev.iter()
        .filter_map(|e| match e {
            Event::Received { json, .. } => serde_json::from_str(json).ok(),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn reflection_and_all_call_kinds() {
    let srv = grpc_echo::start().await.unwrap();
    let ch = connect(&ConnSettings {
        uri: format!("http://{}", srv.target()),
        verify_tls: true,
        connect_timeout: Duration::from_secs(5),
    })
    .await
    .unwrap();
    let schema = from_reflection(ch.clone()).await.unwrap();
    let svcs = schema.services();
    assert_eq!(svcs.len(), 1);
    assert_eq!(svcs[0].name, "sendito.test.v1.Echo");
    let kinds: Vec<_> = svcs[0]
        .methods
        .iter()
        .map(|m| (m.name.as_str(), m.kind))
        .collect();
    assert!(kinds.contains(&("Bidi", CallKind::Bidi)));
    assert!(kinds.contains(&("ClientStream", CallKind::ClientStreaming)));

    // Template parses back into a message.
    let unary = schema.method("sendito.test.v1.Echo", "Unary").unwrap();
    let tpl = template_for(&unary.input());
    assert!(tpl.contains("\"by_id\""), "{tpl}");
    let parsed = parse_messages(&unary.input(), &tpl, false).unwrap();
    assert_eq!(parsed.len(), 1);

    // Unary with metadata.
    let msgs = parse_messages(
        &unary.input(),
        r#"{"text":"hi","counts":{"a":"5"},"at":"2024-01-02T03:04:05Z"}"#,
        false,
    )
    .unwrap();
    let md = metadata_map(&[("x-trace".into(), "abc".into())]).unwrap();
    let ev = collect(start_call(ch.clone(), &unary, md, None, msgs)).await;
    assert_eq!(finished(&ev).0, 0, "{ev:?}");
    let r = received(&ev);
    assert_eq!(r.len(), 1);
    assert_eq!(r[0]["request"]["text"], "hi");
    assert_eq!(r[0]["metadata"]["x-trace"], "abc");
    assert!(ev.iter().any(|e| matches!(e, Event::Headers { metadata, .. } if metadata.iter().any(|(k, _)| k == "x-served-by"))));

    // Server streaming.
    let ss = schema
        .method("sendito.test.v1.Echo", "ServerStream")
        .unwrap();
    let msgs = parse_messages(&ss.input(), r#"{"text":"s","repeat":4}"#, false).unwrap();
    let ev = collect(start_call(ch.clone(), &ss, Default::default(), None, msgs)).await;
    assert_eq!(received(&ev).len(), 4);

    // Client streaming via JSON array.
    let cs = schema
        .method("sendito.test.v1.Echo", "ClientStream")
        .unwrap();
    let msgs = parse_messages(
        &cs.input(),
        r#"[{"text":"a"},{"text":"b"},{"text":"c"}]"#,
        true,
    )
    .unwrap();
    let mut call = start_call(ch.clone(), &cs, Default::default(), None, msgs);
    assert!(call.can_send());
    call.close_send();
    let ev = collect(call).await;
    let r = received(&ev);
    assert_eq!(r[0]["count"], 3);
    assert_eq!(r[0]["request"]["text"], "c");

    // Bidi, interactive.
    let bd = schema.method("sendito.test.v1.Echo", "Bidi").unwrap();
    let first = parse_messages(&bd.input(), r#"{"text":"one"}"#, true).unwrap();
    let mut call = start_call(ch.clone(), &bd, Default::default(), None, first);
    let more = parse_messages(&bd.input(), r#"[{"text":"two"},{"text":"three"}]"#, true).unwrap();
    call.send(more).unwrap();
    call.close_send();
    assert!(call.send(vec![]).is_err());
    let ev = collect(call).await;
    let texts: Vec<String> = received(&ev)
        .iter()
        .map(|v| v["request"]["text"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(texts, vec!["one", "two", "three"]);
    assert_eq!(
        ev.iter()
            .filter(|e| matches!(e, Event::Sent { .. }))
            .count(),
        3
    );

    // Server error status.
    let msgs = parse_messages(
        &unary.input(),
        r#"{"fail_code":5,"fail_message":"nope"}"#,
        false,
    )
    .unwrap();
    let ev = collect(start_call(
        ch.clone(),
        &unary,
        Default::default(),
        None,
        msgs,
    ))
    .await;
    assert_eq!(finished(&ev), (5, "nope".to_string()));

    // Deadline exceeded.
    let msgs = parse_messages(&unary.input(), r#"{"delay_ms":1000}"#, false).unwrap();
    let ev = collect(start_call(
        ch.clone(),
        &unary,
        Default::default(),
        Some(Duration::from_millis(100)),
        msgs,
    ))
    .await;
    assert_eq!(finished(&ev).0, 4, "{ev:?}");

    // Cancel a long stream.
    let msgs = parse_messages(&ss.input(), r#"{"repeat":100,"delay_ms":50}"#, false).unwrap();
    let mut call = start_call(ch.clone(), &ss, Default::default(), None, msgs);
    tokio::time::sleep(Duration::from_millis(120)).await;
    call.cancel();
    let ev = collect(call).await;
    assert_eq!(finished(&ev).0, 1);

    // Unknown fields are rejected; arrays only for client-streaming.
    assert!(parse_messages(&unary.input(), r#"{"nope":1}"#, false).is_err());
    assert!(parse_messages(&unary.input(), r#"[{}]"#, false).is_err());
    assert!(parse_messages(&unary.input(), r#"not json"#, false).is_err());
    assert!(parse_messages(&unary.input(), "", false).is_ok());

    let caps = srv.take();
    assert!(
        caps.iter()
            .any(|c| c.method == "Bidi" && c.messages.len() == 3)
    );
    assert!(caps.iter().any(
        |c| c.method == "Unary" && c.metadata.iter().any(|(k, v)| k == "x-trace" && v == "abc")
    ));
}

#[tokio::test]
async fn files_schema() {
    let dir = grpc_echo::proto_dir();
    let s = Schema::from_files(
        &["sendito/test/v1/echo.proto".into()],
        &[dir.display().to_string()],
        &dir,
    )
    .unwrap();
    assert_eq!(s.services()[0].methods.len(), 5);
    assert!(
        s.template("sendito.test.v1.Echo", "Ping")
            .unwrap()
            .contains("{}")
    );
    assert!(s.method("nope.Svc", "X").is_err());
    assert!(s.method("sendito.test.v1.Echo", "Nope").is_err());
    let s2 = Schema::from_files(
        &[dir
            .join("sendito/test/v1/common.proto")
            .display()
            .to_string()],
        &[],
        &dir,
    )
    .unwrap();
    assert!(s2.services().is_empty());
    assert!(Schema::from_files(&[], &[], &dir).is_err());
    assert!(Schema::from_files(&["missing.proto".into()], &[], &dir).is_err());
}

#[tokio::test]
async fn connect_errors() {
    let r = connect(&ConnSettings {
        uri: "http://127.0.0.1:1".into(),
        verify_tls: true,
        connect_timeout: Duration::from_millis(500),
    })
    .await;
    assert!(r.is_err());
    let r = connect(&ConnSettings {
        uri: "not a uri".into(),
        verify_tls: true,
        connect_timeout: Duration::from_millis(500),
    })
    .await;
    assert!(r.is_err());
}
