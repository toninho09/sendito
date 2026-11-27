//! Deterministic SSE screens from pipeline updates.
use super::*;
use sendito_pipeline::runner::SseUpdate;

pub fn demo(h: &mut Harness) {
    h.app.run_command("newsse");
    let tab = h.app.tab_mut().must();
    tab.ed.url.set_text("https://example.test/events");
    tab.ed.max_events.set_text("3");
    tab.ed.deadline.set_text("5000");
    tab.req_tab = 4;
    tab.refresh_model();
    let id = tab.id;
    let sendito_core::Request::Sse(request) = &tab.model else {
        panic!()
    };
    let resolved = sendito_core::resolve::resolve_sse(request, &sendito_core::Scope::new()).http;
    let response = sendito_pipeline::http::HttpResponse {
        status: 200,
        reason: "OK".into(),
        version: "HTTP/1.1".into(),
        url: "https://example.test/events".into(),
        remote_addr: None,
        headers: vec![("Content-Type".into(), "text/event-stream".into())],
        body: Vec::new(),
        truncated: false,
        decoded: None,
        timing: sendito_pipeline::http::Timing {
            total: Duration::from_millis(10),
            ..Default::default()
        },
    };
    let mut deliver = |update| {
        let (ack, _) = tokio::sync::oneshot::channel();
        h.app.on_msg(sendito_ui::app::AppMsg::Sse {
            tab: id,
            update,
            ack,
        });
    };
    deliver(SseUpdate::Started {
        resolved: Box::new(resolved),
        response: Box::new(response),
    });
    for index in 1..=3 {
        let data = format!("{{\"count\":{index},\"state\":\"ready\"}}");
        deliver(SseUpdate::Event {
            size: data.len(),
            event: sendito_pipeline::http::sse::SseEvent {
                event: "update".into(),
                data,
                id: index.to_string(),
                retry_ms: Some(1000),
            },
            at: Duration::from_millis(100 * index),
        });
    }
    deliver(SseUpdate::Done {
        events: 3,
        duration: Duration::from_millis(350),
        reason: "max events".into(),
        error: None,
        scripts: Default::default(),
        scope_errors: Vec::new(),
        hint: None,
    });
    h.app.status = None;
    h.app.focus = sendito_ui::app::Focus::ReqBody;
}
