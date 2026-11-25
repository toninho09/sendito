//! SSE response updates. Event storage and UI delivery stay bounded.
use super::*;

impl App {
    pub(super) fn on_sse(&mut self, tab: u64, update: runner::SseUpdate) {
        let keep = self.project.config.ui.stream_messages_kept;
        let Some(t) = self.tab_by_id(tab) else { return };
        let response = t.response.get_or_insert_with(Response::new);
        let follow_log = response.messages.cursor + 2 >= response.messages.row_count();
        let mut runtime = Vec::new();
        let mut status = None;
        match update {
            runner::SseUpdate::Started {
                resolved,
                response: head,
            } => {
                response.summary.status = "200 · streaming".into();
                response.summary.ok = true;
                response.summary.code = Some(200);
                response.stream.opened_at = Some(head.timing.total);
                let mut text = format!(
                    "GET {}\n\nHTTP {} {}\n",
                    resolved.url, head.status, head.reason
                );
                for (name, value) in &head.headers {
                    text.push_str(&format!("{name}: {value}\n"));
                }
                response.headers = crate::tab::wrapped(Doc::text(&text));
                response.messages.doc.append_text("stream opened\n");
            }
            runner::SseUpdate::Event { event, at, size } => {
                if response.messages.doc.len() > 1024 * 1024 {
                    response.messages =
                        crate::tab::wrapped(Doc::text("earlier log entries removed\n"));
                }
                response.messages.doc.append_text(&format!(
                    "+{:.3}s event={} id={} retry={:?}\n",
                    at.as_secs_f64(),
                    event.event,
                    event.id,
                    event.retry_ms
                ));
                response.push_sse(event, at, size, keep);
                response.summary.size = Some(response.stream.bytes);
            }
            runner::SseUpdate::Done {
                events,
                duration,
                reason,
                error,
                scripts,
                scope_errors,
            } => {
                t.running = None;
                response.stream.closed_at = Some(duration);
                response.summary.ok =
                    error.is_none() && reason != "cancelled" && scripts.failed_asserts() == 0;
                response.summary.status = format!("{events} events · {reason}");
                response.summary.error = error.clone();
                response.summary.time_ms = Some(duration.as_secs_f64() * 1000.0);
                response.messages.doc.append_text(&format!(
                    "{reason}\n{}\n{}",
                    error.unwrap_or_default(),
                    messages_text(&scripts, &scope_errors, &[])
                ));
                runtime = scripts.updates();
                status = Some((
                    response.summary.status.clone(),
                    if response.summary.ok {
                        StatusKind::Ok
                    } else {
                        StatusKind::Error
                    },
                ));
            }
        }
        response.timing = crate::tab::wrapped(Doc::text(&format!(
            "Events received {}\nBytes received {}\nFirst event {:?}\nDuration {:?}\n",
            response.stream.received,
            response.stream.bytes,
            response.stream.first_at,
            response.stream.closed_at
        )));
        if follow_log {
            response.messages.bottom();
        }
        self.apply_updates(runtime);
        if let Some((text, kind)) = status {
            self.set_status(text, kind);
        }
    }
}
