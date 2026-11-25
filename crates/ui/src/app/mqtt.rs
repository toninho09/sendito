//! MQTT session updates: the connection, the messages, the timing and the log.
use super::*;
use crate::tab::MqttMsg;

impl App {
    /// Publishes the selected row of the Publish table (the first row when
    /// the selection is on the empty "add" row) on the running session.
    pub(super) fn mqtt_publish_selected(&mut self) {
        let end = self.keymap.hint(Action::EndStream, self.vim);
        let Some(t) = self.tab_mut() else { return };
        let table = &t.ed.mqtt_publish;
        let row = table
            .rows
            .get(table.sel_row)
            .filter(|r| !crate::tab::mqtt_publish_row(r).topic.trim().is_empty())
            .or_else(|| table.values().into_iter().next());
        let Some(message) = row.map(crate::tab::mqtt_publish_row) else {
            self.set_status(
                "add a row to the Publish tab first: topic and payload",
                StatusKind::Warn,
            );
            return;
        };
        let topic = message.topic.clone();
        if let Some(control) = t.running.as_ref().and_then(|r| r.control.as_ref()) {
            let _ = control.send(Control::Mqtt(runner::MqttControl::Publish(message)));
        }
        self.set_status(
            format!("publishing to {topic} · {end} disconnects"),
            StatusKind::Ok,
        );
    }

    pub(super) fn on_mqtt(&mut self, tab: u64, update: runner::MqttUpdate) {
        let keep = self.project.config.ui.stream_messages_kept;
        let Some(t) = self.tab_by_id(tab) else { return };
        let response = t.response.get_or_insert_with(Response::new);
        let follow_log = response.messages.cursor + 2 >= response.messages.row_count();
        let mut runtime = Vec::new();
        let mut status = None;
        let log = |response: &mut Response, line: String| {
            if response.messages.doc.len() > 1024 * 1024 {
                response.messages = crate::tab::wrapped(Doc::text("earlier log entries removed\n"));
            }
            response.messages.doc.append_text(&line);
        };
        match update {
            runner::MqttUpdate::Started { resolved } => {
                response.summary.status = "connecting".into();
                let user = resolved.username.as_deref().unwrap_or("(none)");
                let mut text = format!(
                    "Broker     {}\nClient ID  {}\nProtocol   {}\nUser name  {user}\nClean      {}\nKeep alive {} s\n",
                    resolved.url,
                    resolved.client_id,
                    resolved.protocol.as_str(),
                    resolved.clean_session,
                    resolved.keep_alive_s,
                );
                if let Some(w) = &resolved.last_will {
                    text.push_str(&format!("Last Will  {} (QoS {})\n", w.topic, w.qos));
                }
                response.headers = crate::tab::wrapped(Doc::text(&text));
                log(response, format!("connecting to {}\n", resolved.url));
            }
            runner::MqttUpdate::Connected {
                session_present,
                at,
            } => {
                response.summary.status = "connected".into();
                response.summary.ok = true;
                response.stream.opened_at = Some(at);
                let present = if session_present { "yes" } else { "no" };
                response.headers.doc.append_text(&format!(
                    "CONNACK    accepted (session present: {present})\n"
                ));
                log(
                    response,
                    format!(
                        "+{:.3}s connected (session present: {present})\n",
                        at.as_secs_f64()
                    ),
                );
                if let Some(r) = &mut t.running {
                    r.streaming_input = true;
                }
            }
            runner::MqttUpdate::Subscribed { topic, qos, at } => {
                log(
                    response,
                    format!("+{:.3}s subscribed {topic} (QoS {qos})\n", at.as_secs_f64()),
                );
            }
            runner::MqttUpdate::SubscribeRefused { topic, at } => {
                log(
                    response,
                    format!("+{:.3}s subscription refused {topic}\n", at.as_secs_f64()),
                );
                status = Some((format!("subscription refused: {topic}"), StatusKind::Error));
            }
            runner::MqttUpdate::Published { topic, qos, at } => {
                response.stream.sent += 1;
                log(
                    response,
                    format!("+{:.3}s published {topic} (QoS {qos})\n", at.as_secs_f64()),
                );
            }
            runner::MqttUpdate::PublishFailed { error } => {
                log(response, format!("publish failed: {error}\n"));
                status = Some((format!("not published: {error}"), StatusKind::Error));
            }
            runner::MqttUpdate::Message {
                topic,
                qos,
                retain,
                duplicate,
                payload,
                at,
            } => {
                let flags = format!(
                    "{}{}",
                    if retain { " retain" } else { "" },
                    if duplicate { " dup" } else { "" }
                );
                log(
                    response,
                    format!(
                        "+{:.3}s message {topic} qos={qos}{flags} ({} bytes)\n",
                        at.as_secs_f64(),
                        payload.len()
                    ),
                );
                response.push_mqtt(MqttMsg::new(at, topic, qos, retain, &payload), keep);
                response.summary.size = Some(response.stream.bytes);
            }
            runner::MqttUpdate::Done {
                messages,
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
                response.summary.status = format!("{messages} messages · {reason}");
                response.summary.error = error.clone();
                response.summary.time_ms = Some(duration.as_secs_f64() * 1000.0);
                if let Some(e) = &error
                    && response.mqtt_messages.is_empty()
                {
                    response.set_body_wrapped(format!("Session failed:\n\n{e}\n"));
                }
                log(
                    response,
                    format!(
                        "{reason}\n{}\n{}",
                        error.unwrap_or_default(),
                        messages_text(&scripts, &scope_errors, &[])
                    ),
                );
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
            "Connected after {:?}\nMessages received {}\nBytes received {}\nPublished {}\nFirst message {:?}\nDuration {:?}\n",
            response.stream.opened_at,
            response.stream.received,
            response.stream.bytes,
            response.stream.sent,
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
