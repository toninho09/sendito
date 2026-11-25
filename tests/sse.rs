//! SSE model, file format and command-line sessions.
#[cfg(test)]
mod tests {
    use sendito_core::{Auth, KeyValue, Request, SseOptions, SseRequest};
    use sendito_testkit::http_echo;
    use std::process::{Command, Output};

    async fn run_case(route: &str, options: SseOptions, arguments: &[&str]) -> Output {
        let server = http_echo::start().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        sendito_store::init::init(root.path()).unwrap();
        let request = Request::Sse(SseRequest {
            url: format!("{}{route}", server.url()),
            options,
            ..Default::default()
        });
        let file = root.path().join("events.toml");
        std::fs::write(&file, sendito_store::request_file::to_toml(&request)).unwrap();
        let arguments: Vec<String> = arguments.iter().map(|a| a.to_string()).collect();
        tokio::task::spawn_blocking(move || {
            let _root = root;
            Command::new(env!("CARGO_BIN_EXE_sendito"))
                .arg("run")
                .arg(file)
                .arg("--no-history")
                .args(arguments)
                .output()
                .unwrap()
        })
        .await
        .unwrap()
    }

    #[test]
    fn sse_model_resolution_inheritance_masking_and_roundtrip() {
        let req = Request::Sse(SseRequest {
            url: "{{base}}/events".into(),
            headers: vec![KeyValue::new("X-Token", "{{token}}")],
            params: vec![KeyValue::new("q", "a b")],
            options: SseOptions {
                max_events: Some(3),
                deadline_ms: Some(2000),
                timeout_ms: Some(1000),
                cookies: Some(false),
                follow_redirects: false,
                verify_tls: false,
            },
            ..Default::default()
        });
        let text = sendito_store::request_file::to_toml(&req);
        assert_eq!(sendito_store::request_file::parse(&text).unwrap(), req);
        assert_eq!(
            sendito_store::request_file::sniff_badge(&text).as_deref(),
            Some("SSE")
        );
        assert_eq!(
            sendito_store::config::Config::default().sse.timeout_ms,
            30_000
        );
        assert!(sendito_store::request_file::parse("kind = 'sse'\nmethod = 'POST'\n").is_err());
        assert!(
            sendito_store::request_file::parse("kind = 'sse'\n[body]\ntype = 'none'\n").is_err()
        );
        let mut scope = sendito_core::Scope::new();
        scope.set(
            "test",
            "base",
            sendito_core::VarValue::plain("https://example.test"),
        );
        scope.set(
            "test",
            "token",
            sendito_core::VarValue::secret("private-token"),
        );
        let inherited = sendito_core::inherit::apply(
            &req,
            &[(
                String::new(),
                sendito_core::FolderSettings {
                    auth: Auth::Bearer {
                        token: "{{token}}".into(),
                    },
                    headers: vec![KeyValue::new("X-Folder", "yes")],
                    ..Default::default()
                },
            )],
        );
        let Request::Sse(sse) = &inherited else {
            panic!()
        };
        let resolved = sendito_core::resolve::resolve_sse(sse, &scope);
        assert_eq!(resolved.http.method, "GET");
        assert!(resolved.http.url.ends_with("/events?q=a%20b"));
        assert_eq!(resolved.http.header("accept"), Some("text/event-stream"));
        assert_eq!(
            resolved.http.header("authorization"),
            Some("Bearer private-token")
        );
        assert!(
            !format!(
                "{:?}",
                sendito_core::resolve::mask_request(&inherited, &scope)
            )
            .contains("private-token")
        );
    }

    #[tokio::test]
    async fn sse_cli_max_events_and_json_report() {
        let options = SseOptions {
            max_events: Some(2),
            ..Default::default()
        };
        let out = run_case("/sse/events", options.clone(), &[]).await;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "{\"n\":1}\n{\"n\":2}\n"
        );
        let out = run_case("/sse/events", options, &["--report", "json"]).await;
        assert!(out.status.success());
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["events"], 2);
        assert_eq!(report["outcome"], "pass");
    }

    #[tokio::test]
    async fn sse_suite_reports_events_and_chains_extractions() {
        use sendito_core::{
            AssertOp, Assertion, Extract, HttpRequest, RequestCommon, Scripts, Source,
        };
        let server = http_echo::start().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        sendito_store::init::init(root.path()).unwrap();
        let suite = root.path().join("suite");
        std::fs::create_dir(&suite).unwrap();
        let stream = Request::Sse(SseRequest {
            url: format!("{}/sse/events", server.url()),
            options: SseOptions {
                max_events: Some(1),
                ..Default::default()
            },
            common: RequestCommon {
                scripts: Scripts {
                    extract: vec![Extract {
                        var: "n".into(),
                        source: Source::Body,
                        expr: "$.n".into(),
                        secret: false,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        });
        let http = Request::Http(HttpRequest {
            url: format!("{}/echo", server.url()),
            params: vec![KeyValue::new("n", "{{n}}")],
            common: RequestCommon {
                scripts: Scripts {
                    asserts: vec![Assertion {
                        source: Source::Body,
                        expr: "$.target".into(),
                        op: AssertOp::Eq,
                        value: "/echo?n=1".into(),
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        });
        std::fs::write(
            suite.join("01-stream.toml"),
            sendito_store::request_file::to_toml(&stream),
        )
        .unwrap();
        std::fs::write(
            suite.join("02-http.toml"),
            sendito_store::request_file::to_toml(&http),
        )
        .unwrap();
        let out = tokio::task::spawn_blocking(move || {
            let _root = root;
            Command::new(env!("CARGO_BIN_EXE_sendito"))
                .arg("run")
                .arg(suite)
                .args(["--no-history", "--report", "json"])
                .output()
                .unwrap()
        })
        .await
        .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["passed"], 2);
        assert_eq!(report["requests"][0]["events"], 1);
        assert!(report["requests"][1].get("events").is_none());
    }

    #[tokio::test]
    async fn sse_cli_errors_and_stream_formats() {
        for route in ["/sse/html", "/sse/status", "/sse/compressed", "/sse/large"] {
            let out = run_case(route, Default::default(), &[]).await;
            assert_eq!(
                out.status.code(),
                Some(2),
                "{route}: {}",
                String::from_utf8_lossy(&out.stderr)
            );
        }
        for (route, expected) in [
            ("/sse/utf8", "héllo\n"),
            ("/sse/multiline", "first\nsecond\n"),
            ("/sse/comments", "ok\n"),
            ("/sse/redirect", "{\"n\":1}\n{\"n\":2}\n{\"n\":3}\n"),
        ] {
            let out = run_case(route, Default::default(), &[]).await;
            assert!(
                out.status.success(),
                "{}",
                String::from_utf8_lossy(&out.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&out.stdout), expected, "{route}");
        }
        let out = run_case(
            "/sse/slow",
            SseOptions {
                deadline_ms: Some(150),
                ..Default::default()
            },
            &["--report", "json"],
        )
        .await;
        assert!(out.status.success());
        let report: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(report["status"], "deadline");
        assert!(report["events"].as_u64().unwrap() >= 1);
    }
}
