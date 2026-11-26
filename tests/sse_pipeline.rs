//! SSE authentication, TLS, scripts and secret handling.
#[cfg(test)]
mod tests {
    use sendito_core::*;
    use sendito_pipeline::runner::{self, RunCtx, SseUpdate};
    use sendito_testkit::{http_echo, tls};

    fn context(root: &std::path::Path) -> RunCtx {
        RunCtx {
            root: root.into(),
            history: Some(sendito_pipeline::runner::HistorySink {
                path: root.join("history.jsonl"),
                max_entries: 100,
                max_body: 1024 * 1024,
            }),
            name: "events".into(),
            path: Some("events.toml".into()),
            mask_echoed_secrets: true,
            cookie_jar: true,
            ..Default::default()
        }
    }
    async fn collect(ctx: &RunCtx, req: &SseRequest) -> Vec<SseUpdate> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let (_control, controls) = tokio::sync::mpsc::unbounded_channel();
        let run = runner::exec_sse(ctx, req, Scope::new(), vec![], tx, controls);
        let mut updates = Vec::new();
        let collect = async {
            while let Some(update) = rx.recv().await {
                updates.push(update);
            }
        };
        tokio::join!(run, collect);
        updates
    }

    #[tokio::test]
    async fn sse_digest_and_last_event_scripts() {
        let server = http_echo::start().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let req = SseRequest {
            url: format!("{}/sse/digest", server.url()),
            auth: Auth::Digest {
                username: "user".into(),
                password: "pass".into(),
            },
            options: SseOptions {
                max_events: Some(2),
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
                    asserts: vec![
                        Assertion {
                            source: Source::Body,
                            expr: "$.n".into(),
                            op: AssertOp::Eq,
                            value: "2".into(),
                        },
                        Assertion {
                            source: Source::Status,
                            expr: String::new(),
                            op: AssertOp::Eq,
                            value: "200".into(),
                        },
                    ],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let updates = collect(&context(root.path()), &req).await;
        let SseUpdate::Done {
            events,
            scripts,
            error,
            ..
        } = updates.last().unwrap()
        else {
            panic!()
        };
        assert_eq!(*events, 2);
        assert!(error.is_none(), "{error:?}");
        assert_eq!(scripts.failed_asserts(), 0);
        assert_eq!(scripts.extracts[0].value.as_ref().unwrap().value, "2");
        assert_eq!(server.take().len(), 2);
    }

    #[tokio::test]
    async fn sse_redirect_cookies_and_extractions_remain_secret() {
        let server = http_echo::start().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let req = SseRequest {
            url: format!("{}/sse/redirect-cookie", server.url()),
            common: RequestCommon {
                scripts: Scripts {
                    extract: vec![Extract {
                        var: "cookie".into(),
                        source: Source::Body,
                        expr: "$.cookie".into(),
                        secret: false,
                    }],
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        };
        let updates = collect(&context(root.path()), &req).await;
        let SseUpdate::Done { scripts, error, .. } = updates.last().unwrap() else {
            panic!()
        };
        assert!(error.is_none(), "{error:?}");
        let cookie = scripts.extracts[0].value.as_ref().unwrap();
        assert!(cookie.secret);
        assert!(cookie.value.contains("stream-cookie-secret"));
        for update in &updates {
            if let SseUpdate::Event { event, .. } = update {
                assert!(!event.data.contains("stream-cookie-secret"));
            }
        }
        let captures = server.take();
        assert_eq!(
            captures[1].header("cookie").as_deref(),
            Some("stream-session=stream-cookie-secret")
        );
        assert!(
            !std::fs::read_to_string(root.path().join("history.jsonl"))
                .unwrap()
                .contains("stream-cookie-secret")
        );
    }

    #[tokio::test]
    async fn sse_cancel_stops_transport_when_the_consumer_is_slow() {
        let server = http_echo::start().await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let ctx = context(root.path());
        let request = SseRequest {
            url: format!("{}/sse/events", server.url()),
            ..Default::default()
        };
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let (control, controls) = tokio::sync::mpsc::unbounded_channel();
        let job = tokio::spawn(async move {
            runner::exec_sse(&ctx, &request, Scope::new(), vec![], tx, controls).await;
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            while rx.is_empty() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            control
                .send(runner::Control::Sse(runner::SseControl::Cancel))
                .unwrap();
            while !root.path().join("history.jsonl").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        drop(rx);
        job.await.unwrap();
    }

    #[tokio::test]
    async fn sse_tls_uses_shared_custom_ca_and_client_certificate() {
        let certs = tls::generate().unwrap();
        let server = http_echo::start_tls(&certs, true).await.unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut ctx = context(root.path());
        for (name, bytes) in [
            ("ca.pem", &certs.ca_pem),
            ("client.pem", &certs.client_pem),
            ("client-key.pem", &certs.client_key_pem),
        ] {
            std::fs::write(root.path().join(name), bytes).unwrap();
        }
        ctx.send.tls = sendito_http::TlsMaterial {
            ca: Some(root.path().join("ca.pem")),
            client_cert: Some(root.path().join("client.pem")),
            client_key: Some(root.path().join("client-key.pem")),
        };
        let req = SseRequest {
            url: format!("{}/sse/events", server.url()),
            options: SseOptions {
                max_events: Some(1),
                ..Default::default()
            },
            ..Default::default()
        };
        let updates = collect(&ctx, &req).await;
        assert!(
            matches!(
                updates.last(),
                Some(SseUpdate::Done {
                    events: 1,
                    error: None,
                    ..
                })
            ),
            "{updates:?}"
        );
    }
}
