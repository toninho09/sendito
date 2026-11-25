//! MQTT model, resolution, file format and command-line sessions.
#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::process::{Command, Output};

    use sendito_core::inherit::{self, ChainItem};
    use sendito_core::resolve::{mask_request, mqtt_target, resolve_mqtt, unresolved_vars};
    use sendito_core::*;
    use sendito_testkit::mqtt_broker::MqttBroker;

    fn scope() -> Scope {
        let mut vars = BTreeMap::new();
        vars.insert("host".to_string(), VarValue::plain("broker.local"));
        vars.insert("topic".to_string(), VarValue::plain("room1"));
        vars.insert("pass".to_string(), VarValue::secret("s3cret-pass"));
        Scope::new().with_layer("env", vars)
    }

    fn full() -> MqttRequest {
        MqttRequest {
            url: "mqtts://{{host}}".into(),
            client_id: "dev-1".into(),
            username: String::new(),
            password: String::new(),
            protocol: MqttProtocol::V5,
            clean_session: false,
            keep_alive_s: 15,
            options: MqttOptions {
                timeout_ms: Some(1000),
                deadline_ms: Some(2000),
                verify_tls: false,
            },
            subscriptions: vec![
                MqttSubscription {
                    topic: "home/{{topic}}/#".into(),
                    qos: 1,
                    enabled: true,
                },
                MqttSubscription {
                    topic: "off".into(),
                    qos: 0,
                    enabled: false,
                },
            ],
            publish: vec![
                MqttMessage {
                    topic: "home/{{topic}}/set".into(),
                    payload: MqttPayload::text("{\"on\":true}"),
                    qos: 2,
                    retain: true,
                },
                MqttMessage {
                    topic: "bin".into(),
                    payload: MqttPayload {
                        text: "AAEC".into(),
                        base64: true,
                    },
                    qos: 0,
                    retain: false,
                },
            ],
            last_will: Some(MqttMessage {
                topic: "status".into(),
                payload: MqttPayload::text("offline"),
                qos: 1,
                retain: true,
            }),
            auth: Auth::Basic {
                username: "ana".into(),
                password: "{{pass}}".into(),
            },
            scripts: Scripts::default(),
            description: "Room sensors.\n\n- [x] temperature\n".into(),
        }
    }

    #[test]
    fn mqtt_file_round_trip_and_badge() {
        let req = Request::Mqtt(full());
        let text = sendito_store::request_file::to_toml(&req);
        assert_eq!(sendito_store::request_file::parse(&text).unwrap(), req);
        assert_eq!(sendito_store::request_file::to_toml(&req), text);
        assert_eq!(
            sendito_store::request_file::sniff_badge(&text).as_deref(),
            Some("MQTT")
        );
        assert_eq!(req.badge(), "MQTT");
        assert!(text.contains("payload_base64 = \"AAEC\""));
        // The defaults write nothing but the kind and the URL.
        let empty = sendito_store::request_file::to_toml(&Request::new_mqtt());
        assert_eq!(empty, "kind = \"mqtt\"\nurl = \"\"\n");
        assert_eq!(
            sendito_store::config::Config::default().mqtt.timeout_ms,
            30_000
        );
        for bad in [
            "kind = 'mqtt'\nmethod = 'GET'\n",
            "kind = 'mqtt'\n[[headers]]\nname = 'a'\n",
            "kind = 'mqtt'\nprotocol = '4'\n",
            "kind = 'mqtt'\n[[publish]]\ntopic = 't'\npayload = 'a'\npayload_base64 = 'YQ=='\n",
            "kind = 'mqtt'\n[[subscriptions]]\ntopic = 't'\nqos = 3\n",
        ] {
            assert!(sendito_store::request_file::parse(bad).is_err(), "{bad}");
        }
        // A draft with an unsupported auth kind still opens: it fails at send time.
        let bearer = "kind = 'mqtt'\nurl = 'mqtt://h'\n[auth]\ntype = 'bearer'\ntoken = 't'\n";
        assert!(sendito_store::request_file::parse(bearer).is_ok());
    }

    #[test]
    fn mqtt_resolution_renders_every_template() {
        let r = resolve_mqtt(&full(), &scope());
        assert_eq!(r.error, None);
        let target = r.target.clone().unwrap();
        assert_eq!(
            (target.host.as_str(), target.port, target.tls),
            ("broker.local", 8883, true)
        );
        assert_eq!(r.client_id, "dev-1");
        assert_eq!(r.username.as_deref(), Some("ana"));
        assert_eq!(r.password.as_deref(), Some("s3cret-pass"));
        assert_eq!(r.subscriptions, vec![("home/room1/#".to_string(), 1)]);
        assert_eq!(r.publish[0].topic, "home/room1/set");
        assert_eq!(r.publish[1].payload, vec![0, 1, 2]);
        assert_eq!(r.last_will.as_ref().unwrap().payload, b"offline");
        assert!(r.unresolved.is_empty());
        // The explicit fields win over Basic auth.
        let mut own = full();
        own.username = "dev".into();
        own.password = "pw".into();
        let r = resolve_mqtt(&own, &scope());
        assert_eq!(
            (r.username.as_deref(), r.password.as_deref()),
            (Some("dev"), Some("pw"))
        );
        // No credentials at all.
        let mut none = full();
        none.auth = Auth::None;
        assert_eq!(resolve_mqtt(&none, &scope()).username, None);
    }

    #[test]
    fn mqtt_urls_ports_client_ids_and_refusals() {
        let t = mqtt_target("mqtt://h").unwrap();
        assert_eq!((t.port, t.tls), (1883, false));
        let t = mqtt_target("h:1999").unwrap();
        assert_eq!((t.host.as_str(), t.port, t.tls), ("h", 1999, false));
        let t = mqtt_target("MQTTS://[::1]:9000/").unwrap();
        assert_eq!((t.host.as_str(), t.port, t.tls), ("::1", 9000, true));
        for bad in [
            "",
            "ws://h",
            "mqtt://h/path",
            "mqtt://u:p@h",
            "mqtt://h:0",
            "mqtt://h:x",
            "mqtt://",
        ] {
            assert!(mqtt_target(bad).is_err(), "{bad}");
        }
        let mut req = full();
        req.client_id = String::new();
        let id = resolve_mqtt(&req, &scope()).client_id;
        assert!(id.starts_with("sendito-") && id.len() == 16, "{id}");
        assert!(id[8..].chars().all(|c| c.is_ascii_hexdigit()));
        req.auth = Auth::Bearer { token: "t".into() };
        let error = resolve_mqtt(&req, &scope()).error.unwrap();
        assert!(error.contains("not supported for MQTT"), "{error}");
        let mut req = full();
        req.publish[1].payload.text = "%%%".into();
        let error = resolve_mqtt(&req, &scope()).error.unwrap();
        assert!(error.contains("base64"), "{error}");
        let mut req = full();
        req.url = "{{missing}}".into();
        assert_eq!(
            unresolved_vars(&Request::Mqtt(req), &scope()),
            vec!["missing".to_string()]
        );
    }

    #[test]
    fn mqtt_inherits_scripts_and_auth_but_not_headers() {
        let folder = FolderSettings {
            headers: vec![KeyValue::new("X-A", "1")],
            auth: Auth::Basic {
                username: "folder".into(),
                password: "pw".into(),
            },
            scripts: Scripts {
                pre: vec![SetVar {
                    var: "a".into(),
                    value: "1".into(),
                }],
                ..Default::default()
            },
            ..Default::default()
        };
        let chain: Vec<ChainItem> = vec![("Devices".into(), folder)];
        let req = Request::Mqtt(MqttRequest {
            auth: Auth::Inherit,
            ..full()
        });
        let Request::Mqtt(out) = inherit::apply(&req, &chain) else {
            panic!("not MQTT")
        };
        assert_eq!(out.scripts.pre.len(), 1);
        assert!(matches!(&out.auth, Auth::Basic { username, .. } if username == "folder"));
        let note = inherit::ignored_headers_note(&req, &chain).unwrap();
        assert!(note.contains("Devices"), "{note}");
        assert_eq!(
            inherit::ignored_headers_note(&Request::new_http(), &chain),
            None
        );
    }

    #[test]
    fn mqtt_masking_keeps_secrets_as_placeholders() {
        let mut req = full();
        req.password = "{{pass}}".into();
        req.publish[0].payload = MqttPayload::text("token={{pass}}");
        let masked = mask_request(&Request::Mqtt(req), &scope());
        let text = format!("{masked:?}");
        assert!(!text.contains("s3cret-pass"), "{text}");
        assert!(text.contains("{{pass}}"));
        assert!(text.contains("broker.local"));
    }

    async fn run_file(request: &Request, args: &[&str]) -> Output {
        let root = tempfile::tempdir().unwrap();
        sendito_store::init::init(root.path()).unwrap();
        let file = root.path().join("sensor.toml");
        std::fs::write(&file, sendito_store::request_file::to_toml(request)).unwrap();
        let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
        tokio::task::spawn_blocking(move || {
            let _root = root;
            Command::new(env!("CARGO_BIN_EXE_sendito"))
                .args(&args[..1])
                .arg(file)
                .args(&args[1..])
                .output()
                .unwrap()
        })
        .await
        .unwrap()
    }

    fn session(url: String) -> MqttRequest {
        MqttRequest {
            url,
            client_id: "cli".into(),
            options: MqttOptions {
                deadline_ms: Some(400),
                ..Default::default()
            },
            subscriptions: vec![MqttSubscription {
                topic: "cli/+".into(),
                qos: 2,
                enabled: true,
            }],
            publish: vec![MqttMessage {
                topic: "cli/a".into(),
                payload: MqttPayload::text("{\"n\":1}"),
                qos: 2,
                retain: false,
            }],
            ..Default::default()
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mqtt_cli_deadline_prints_payloads_and_exits_zero() {
        let broker = MqttBroker::start().await.unwrap();
        let req = Request::Mqtt(session(broker.url()));
        let out = run_file(&req, &["run", "--no-history", "--include"]).await;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(0), "{stderr}");
        assert_eq!(stdout, "# cli/a qos=2\n{\"n\":1}\n");
        assert!(stderr.contains("MQTT connected"), "{stderr}");
        assert!(stderr.contains("MQTT deadline: 1 messages"), "{stderr}");
        // The JSON report carries the message count.
        let out = run_file(&req, &["run", "--no-history", "--report", "json"]).await;
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["kind"], "mqtt");
        assert_eq!(report["messages"], 1);
        assert_eq!(report["outcome"], "pass");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mqtt_cli_refused_connack_exits_two() {
        let broker = MqttBroker::start().await.unwrap();
        let mut req = session(broker.url());
        req.username = "bad".into();
        let out = run_file(&Request::Mqtt(req), &["run", "--no-history"]).await;
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(2), "{stderr}");
        assert!(stderr.contains("Connection refused"), "{stderr}");
        assert!(out.stdout.is_empty());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mqtt_cli_refuses_curl_and_http_export() {
        let req = Request::Mqtt(session("mqtt://127.0.0.1:1".into()));
        for command in ["curl", "http"] {
            let out = run_file(&req, &[command]).await;
            let stderr = String::from_utf8_lossy(&out.stderr);
            assert_eq!(out.status.code(), Some(3), "{stderr}");
            assert!(stderr.contains("not MQTT"), "{stderr}");
            assert!(out.stdout.is_empty());
        }
    }
}
