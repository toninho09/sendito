//! MQTT sessions through the pipeline: scripts, secrets, control, TLS.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use sendito_core::*;
    use sendito_pipeline::runner::{self, Control, MqttControl, MqttUpdate, RunCtx};
    use sendito_testkit::mqtt_broker::{self, MqttBroker};
    use sendito_testkit::tls;

    fn context(root: &std::path::Path) -> RunCtx {
        RunCtx {
            root: root.into(),
            send: Default::default(),
            history: Some((root.join("history.jsonl"), 100)),
            name: "sensor".into(),
            path: Some("sensor.toml".into()),
            env: None,
            schemas: Default::default(),
            grpc_deadline_ms: 0,
            ws_timeout_ms: 0,
            sse_timeout_ms: 30_000,
            mqtt_timeout_ms: 30_000,
            mask_echoed_secrets: true,
            cookie_jar: false,
            oauth: None,
        }
    }

    fn secret_scope() -> Scope {
        let mut vars = BTreeMap::new();
        vars.insert("token".to_string(), VarValue::secret("tok-0123456789"));
        Scope::new().with_layer("env", vars)
    }

    fn request(url: String, deadline_ms: Option<u64>) -> MqttRequest {
        MqttRequest {
            url,
            client_id: "pipeline".into(),
            options: MqttOptions {
                deadline_ms,
                ..Default::default()
            },
            subscriptions: vec![MqttSubscription {
                topic: "p/#".into(),
                qos: 1,
                enabled: true,
            }],
            ..Default::default()
        }
    }

    /// Runs a session; `drive` gets the control sender and every update.
    async fn run(
        ctx: &RunCtx,
        req: &MqttRequest,
        scope: Scope,
        drive: impl Fn(&MqttUpdate, &tokio::sync::mpsc::UnboundedSender<Control>),
    ) -> Vec<MqttUpdate> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let (control, controls) = tokio::sync::mpsc::unbounded_channel();
        let exec = runner::exec_mqtt(ctx, req, scope, vec![], tx, controls);
        let mut updates = Vec::new();
        let collect = async {
            while let Some(update) = rx.recv().await {
                drive(&update, &control);
                updates.push(update);
            }
        };
        tokio::join!(exec, collect);
        updates
    }

    fn done(updates: &[MqttUpdate]) -> (u64, String, Option<String>, runner::ScriptResults) {
        match updates.last() {
            Some(MqttUpdate::Done {
                messages,
                reason,
                error,
                scripts,
                ..
            }) => (*messages, reason.clone(), error.clone(), scripts.clone()),
            other => panic!("no Done update: {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mqtt_scripts_secrets_and_history() {
        let broker = MqttBroker::start().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut req = request(broker.url(), Some(400));
        req.publish = vec![MqttMessage {
            topic: "p/{{room}}".into(),
            payload: MqttPayload::text("{\"token\":\"{{token}}\",\"n\":2}"),
            qos: 1,
            retain: false,
        }];
        req.common.scripts = Scripts {
            pre: vec![SetVar {
                var: "room".into(),
                value: "kitchen".into(),
            }],
            extract: vec![Extract {
                var: "n".into(),
                source: Source::Body,
                expr: "$.n".into(),
                secret: false,
            }],
            asserts: vec![
                Assertion {
                    source: Source::Status,
                    expr: String::new(),
                    op: AssertOp::Eq,
                    value: "0".into(),
                },
                Assertion {
                    source: Source::Header,
                    expr: "session-present".into(),
                    op: AssertOp::Eq,
                    value: "false".into(),
                },
                Assertion {
                    source: Source::Body,
                    expr: "$.token".into(),
                    op: AssertOp::Exists,
                    value: String::new(),
                },
            ],
            ..Scripts::default()
        };
        let ctx = context(root.path());
        let updates = run(&ctx, &req, secret_scope(), |_, _| {}).await;
        let (messages, reason, error, scripts) = done(&updates);
        assert_eq!((messages, reason.as_str(), error), (1, "deadline", None));
        assert_eq!(scripts.failed_asserts(), 0, "{:?}", scripts.asserts);
        assert_eq!(scripts.updates().last().unwrap().1.value, "2");
        let message = updates
            .iter()
            .find_map(|u| match u {
                MqttUpdate::Message { topic, payload, .. } => {
                    Some((topic.clone(), payload.clone()))
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(message.0, "p/kitchen");
        let shown = String::from_utf8(message.1).unwrap();
        assert!(!shown.contains("tok-0123456789"), "{shown}");
        assert!(shown.contains("••••••"), "{shown}");
        let history = std::fs::read_to_string(root.path().join("history.jsonl")).unwrap();
        assert!(history.contains("\"badge\":\"MQTT\""), "{history}");
        assert!(!history.contains("tok-0123456789"));
        let captures = mqtt_broker::wait_for(&broker, |c| !c.is_empty()).await;
        assert!(
            captures[0].dump().contains("tok-0123456789"),
            "the wire has the value"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mqtt_live_publish_and_disconnect() {
        let broker = MqttBroker::start().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let req = request(broker.url(), None);
        let updates = run(
            &context(root.path()),
            &req,
            Scope::new(),
            |u, control| match u {
                MqttUpdate::Subscribed { .. } => {
                    let _ = control.send(Control::Mqtt(MqttControl::Publish(MqttMessage {
                        topic: "p/#".into(),
                        payload: MqttPayload::text("bad"),
                        ..Default::default()
                    })));
                    let _ = control.send(Control::Mqtt(MqttControl::Publish(MqttMessage {
                        topic: "p/live".into(),
                        payload: MqttPayload::text("hello"),
                        qos: 2,
                        retain: false,
                    })));
                }
                MqttUpdate::Message { .. } => {
                    let _ = control.send(Control::Mqtt(MqttControl::Disconnect));
                }
                _ => {}
            },
        )
        .await;
        assert!(updates.iter().any(|u| matches!(
            u,
            MqttUpdate::PublishFailed { error } if error.contains("wildcards")
        )));
        assert!(updates.iter().any(|u| matches!(
            u,
            MqttUpdate::Published { topic, qos: 2, .. } if topic == "p/live"
        )));
        let (messages, reason, error, _) = done(&updates);
        assert_eq!(
            (messages, reason.as_str(), error),
            (1, "disconnected", None)
        );
        let captures = mqtt_broker::wait_for(&broker, |c| !c.is_empty()).await;
        assert!(captures[0].dump().contains("< DISCONNECT"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mqtt_cancel_sends_the_last_will() {
        let broker = MqttBroker::start().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut req = request(broker.url(), None);
        req.last_will = Some(MqttMessage {
            topic: "p/status".into(),
            payload: MqttPayload::text("offline"),
            ..Default::default()
        });
        let updates = run(&context(root.path()), &req, Scope::new(), |u, control| {
            if matches!(u, MqttUpdate::Subscribed { .. }) {
                let _ = control.send(Control::Mqtt(MqttControl::Cancel));
            }
        })
        .await;
        let (_, reason, error, _) = done(&updates);
        assert_eq!((reason.as_str(), error), ("cancelled", None));
        let captures = mqtt_broker::wait_for(&broker, |c| !c.is_empty()).await;
        let dump = captures[0].dump();
        assert!(dump.contains("< WILL topic p/status"), "{dump}");
        assert!(!dump.contains("DISCONNECT"), "{dump}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mqtt_refused_connection_and_bad_requests_fail_before_scripts() {
        let broker = MqttBroker::start().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let ctx = context(root.path());
        let mut req = request(broker.url(), Some(400));
        req.username = "bad".into();
        req.common.scripts.asserts = vec![Assertion {
            source: Source::Status,
            expr: String::new(),
            op: AssertOp::Eq,
            value: "0".into(),
        }];
        let (_, reason, error, scripts) = done(&run(&ctx, &req, Scope::new(), |_, _| {}).await);
        assert_eq!(reason, "error");
        assert!(error.unwrap().contains("Connection refused"));
        assert!(
            scripts.asserts.is_empty(),
            "no CONNACK accepted: no scripts"
        );
        // Problems found by resolution end the run before any connection.
        let mut req = request("ws://x".into(), None);
        let (_, _, error, _) = done(&run(&ctx, &req, Scope::new(), |_, _| {}).await);
        assert!(error.unwrap().contains("mqtt://"));
        req.url = broker.url();
        req.auth = Auth::Bearer { token: "t".into() };
        let updates = run(&ctx, &req, Scope::new(), |_, _| {}).await;
        assert!(
            !updates
                .iter()
                .any(|u| matches!(u, MqttUpdate::Started { .. }))
        );
        assert!(done(&updates).2.unwrap().contains("not supported for MQTT"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mqtt_connect_timeout_without_connack() {
        // A listener that accepts and never answers.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let hold = tokio::spawn(async move {
            let mut kept = Vec::new();
            while let Ok((socket, _)) = listener.accept().await {
                kept.push(socket);
            }
        });
        let root = tempfile::tempdir().unwrap();
        let mut ctx = context(root.path());
        ctx.mqtt_timeout_ms = 200;
        let req = request(format!("mqtt://127.0.0.1:{}", addr.port()), None);
        let started = std::time::Instant::now();
        let (_, reason, error, _) = done(&run(&ctx, &req, Scope::new(), |_, _| {}).await);
        assert!(started.elapsed() < Duration::from_secs(5));
        assert_eq!(reason, "error");
        assert!(error.unwrap().contains("no CONNACK within 200 ms"));
        hold.abort();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mqtt_tls_with_a_custom_ca() {
        let certs = tls::generate().unwrap();
        let broker = MqttBroker::start_tls(&certs).await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let ca = root.path().join("ca.pem");
        std::fs::write(&ca, &certs.ca_pem).unwrap();
        let mut ctx = context(root.path());
        let mut req = request(broker.url(), Some(400));
        req.publish = vec![MqttMessage {
            topic: "p/tls".into(),
            payload: MqttPayload::text("secure"),
            ..Default::default()
        }];
        // Without the CA the certificate is refused.
        let (_, _, error, _) = done(&run(&ctx, &req, Scope::new(), |_, _| {}).await);
        assert!(error.is_some());
        ctx.send.tls = sendito_http::TlsMaterial {
            ca: Some(ca),
            client_cert: None,
            client_key: None,
        };
        let (messages, reason, error, _) = done(&run(&ctx, &req, Scope::new(), |_, _| {}).await);
        assert_eq!((messages, reason.as_str(), error), (1, "deadline", None));
    }
}
