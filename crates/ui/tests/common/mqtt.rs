//! Deterministic MQTT screens from pipeline updates.
use super::*;
use sendito_pipeline::runner::MqttUpdate;

pub fn demo(h: &mut Harness) {
    h.app.run_command("newmqtt");
    let tab = h.app.tab_mut().must();
    tab.ed.url.set_text("mqtts://broker.example.test");
    tab.ed.mqtt_client_id.set_text("sendito-demo");
    tab.ed.deadline.set_text("5000");
    tab.ed.mqtt_subs.push(
        true,
        vec![
            sendito_ui::form::Cell::Text(sendito_ui::editor::TextEditor::single(
                "sensors/+/temperature",
            )),
            sendito_ui::form::Cell::Choice(1),
        ],
    );
    tab.req_tab = 0;
    tab.refresh_model();
    let id = tab.id;
    let sendito_core::Request::Mqtt(request) = &tab.model else {
        panic!()
    };
    let resolved = sendito_core::resolve::resolve_mqtt(request, &sendito_core::Scope::new());
    let mut deliver = |update| {
        let (ack, _) = tokio::sync::oneshot::channel();
        h.app.on_msg(sendito_ui::app::AppMsg::Mqtt {
            tab: id,
            update,
            ack,
        });
    };
    deliver(MqttUpdate::Started {
        resolved: Box::new(resolved),
    });
    deliver(MqttUpdate::Connected {
        session_present: false,
        at: Duration::from_millis(12),
    });
    deliver(MqttUpdate::Subscribed {
        topic: "sensors/+/temperature".into(),
        qos: 1,
        at: Duration::from_millis(15),
    });
    for (index, room) in ["kitchen", "office", "garage"].iter().enumerate() {
        let payload = format!("{{\"room\":\"{room}\",\"celsius\":{}}}", 20 + index);
        deliver(MqttUpdate::Message {
            topic: format!("sensors/{room}/temperature"),
            qos: 1,
            retain: index == 0,
            duplicate: false,
            payload: payload.into_bytes(),
            at: Duration::from_millis(100 * (index as u64 + 1)),
        });
    }
    deliver(MqttUpdate::Done {
        messages: 3,
        duration: Duration::from_millis(5000),
        reason: "deadline".into(),
        error: None,
        scripts: Default::default(),
        scope_errors: Vec::new(),
    });
    h.app.status = None;
    h.app.focus = sendito_ui::app::Focus::ReqBody;
}
