//! SSE wire captures and parsed-event goldens.
use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sse_corpus() {
    let e = setup().await;
    let p = project(&e);
    let names = cases(e.dir.path(), "sse");
    assert_eq!(names.len(), 13);
    let mut failures = Vec::new();
    let mut passed = 0;
    for file in &names {
        let case = file.trim_end_matches(".toml");
        let rel = format!("sse/{file}");
        let (mut scope, errors) = runner::build_scope(&p.envs, Some("corpus"), &Default::default());
        assert!(errors.is_empty());
        let chain = p.coll.chain(&rel).must();
        let Request::Sse(req) = runner::with_folders(&p.coll.load(&rel).must(), &chain, &mut scope)
        else {
            panic!("not SSE")
        };
        e.http.take();
        let run_ctx = ctx(&e, &p, &rel);
        let (tx, mut rx) = tokio::sync::mpsc::channel(64);
        let (_control, controls) = tokio::sync::mpsc::unbounded_channel();
        let run = runner::exec_sse(&run_ctx, &req, scope, vec![], tx, controls);
        let mut events = String::new();
        let mut failed = false;
        let collect = async {
            while let Some(update) = rx.recv().await {
                match update {
                    runner::SseUpdate::Event { event, .. } => {
                        events.push_str(&format!("### event {}\n", serde_json::json!({"event":event.event,"data":event.data,"id":event.id,"retry_ms":event.retry_ms})));
                    }
                    runner::SseUpdate::Done {
                        events: count,
                        reason,
                        error,
                        ..
                    } => {
                        failed = error.is_some();
                        events.push_str(&format!("### done: {count} events, {reason}\n"));
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
        assert_eq!(
            failed,
            ["08-html", "09-compressed", "10-status", "11-large"].contains(&case),
            "{case}"
        );
        let captures = e.http.take();
        assert!(!captures.is_empty());
        for capture in &captures {
            assert_eq!(capture.method(), "GET");
            assert!(capture.body().is_empty());
            assert!(capture.header("last-event-id").is_none());
        }
        if case == "12-bearer" {
            assert_eq!(
                captures[0].header("authorization"),
                Some(format!("Bearer {}", e.secrets["local_token"]))
            );
        }
        let mut dump = String::new();
        for capture in captures {
            dump.push_str(&normalize_http(&capture, e.http.addr.port(), &e.secrets));
        }
        dump.push_str(&events);
        check("sse", case, &dump, &mut failures);
        passed += usize::from(failures.len() == before);
    }
    write_summary("sse", names.len(), passed, &failures);
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
