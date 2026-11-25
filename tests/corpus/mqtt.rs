//! MQTT wire captures: the packets the test broker saw and the messages the
//! client received.
use super::*;
use sendito_testkit::mqtt_broker;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mqtt_corpus() {
    let e = setup().await;
    let p = project(&e);
    let names = cases(e.dir.path(), "mqtt");
    assert_eq!(names.len(), 13);
    let mut failures = Vec::new();
    let mut passed = 0;
    for file in &names {
        let case = file.trim_end_matches(".toml");
        let rel = format!("mqtt/{file}");
        let (mut scope, errors) = runner::build_scope(&p.envs, Some("corpus"), &Default::default());
        assert!(errors.is_empty());
        let chain = p.coll.chain(&rel).must();
        let Request::Mqtt(req) =
            runner::with_folders(&p.coll.load(&rel).must(), &chain, &mut scope)
        else {
            panic!("not MQTT")
        };
        let secure = req.url.contains("mqtt_tls_url");
        let broker = if secure { &e.mqtt_tls } else { &e.mqtt };
        broker.take();
        let run_ctx = ctx(&e, &p, &rel);
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let (_control, controls) = tokio::sync::mpsc::unbounded_channel();
        let run = runner::exec_mqtt(&run_ctx, &req, scope, vec![], tx, controls);
        let mut events = String::new();
        let mut failed = false;
        let collect = async {
            while let Some(update) = rx.recv().await {
                match update {
                    runner::MqttUpdate::Message {
                        topic,
                        qos,
                        retain,
                        payload,
                        ..
                    } => {
                        let payload = match String::from_utf8(payload) {
                            Ok(text) => serde_json::json!(text),
                            Err(e) => serde_json::json!({ "hex": hex(e.as_bytes()) }),
                        };
                        events.push_str(&format!(
                            "### message {}\n",
                            serde_json::json!({"topic": topic, "qos": qos, "retain": retain, "payload": payload})
                        ));
                    }
                    runner::MqttUpdate::Done {
                        messages,
                        reason,
                        error,
                        ..
                    } => {
                        failed = error.is_some();
                        events.push_str(&format!("### done: {messages} messages, {reason}\n"));
                        if let Some(error) = error {
                            events.push_str(&format!("### error: {error}\n"));
                        }
                    }
                    _ => {}
                }
            }
        };
        tokio::join!(run, collect);
        let before = failures.len();
        assert_eq!(failed, case == "10-refused", "{case}");
        let captures = mqtt_broker::wait_for(broker, |c| !c.is_empty()).await;
        broker.take();
        assert_eq!(captures.len(), 1, "{case}: one session");
        if case == "08-retained" {
            assert_eq!(
                broker.retained("corpus/retained"),
                Some((b"kept by the broker".to_vec(), 1))
            );
        }
        let mut dump = String::new();
        for capture in &captures {
            dump.push_str(&mask_secrets(capture.dump(), &e.secrets));
        }
        dump.push_str(&events);
        check("mqtt", case, &dump, &mut failures);
        passed += usize::from(failures.len() == before);
    }
    write_summary("mqtt", names.len(), passed, &failures);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
