//! GraphQL over WebSocket: one subscription per session (id `1`) in
//! `graphql-transport-ws` or the legacy `graphql-ws`
//! (subscriptions-transport-ws). The WebSocket loop of `exec_ws` sends what
//! this state machine returns.

use serde_json::{Value, json};

use sendito_core::model::{WsGraphql, WsGraphqlProtocol, graphql_document_json};

/// The id of the one subscription of a session.
const ID: &str = "1";

/// What the session does after a server message.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Step {
    /// Send this text frame.
    Send(String),
    /// Start the close handshake (1000).
    Close,
    /// The operation failed with this message.
    Error(String),
}

/// The state of the GraphQL protocol in one session.
pub(super) struct GraphqlWs {
    protocol: WsGraphqlProtocol,
    /// `{query, variables, operationName}`.
    document: Value,
    acked: bool,
    done: bool,
    /// The payload of the last `next` (legacy: `data`) message, as JSON text:
    /// the body the scripts read.
    pub(super) payload: Option<Vec<u8>>,
}

impl GraphqlWs {
    /// The state and the `connection_init` frame, or why the operation is
    /// not valid.
    pub(super) fn new(g: &WsGraphql) -> Result<(Self, String), String> {
        let doc = graphql_document_json(&g.query, &g.variables, &g.operation_name)?;
        let document: Value = serde_json::from_str(&doc).map_err(|e| e.to_string())?;
        let mut init = json!({ "type": "connection_init" });
        if !g.init_payload.trim().is_empty() {
            let payload: Value = serde_json::from_str(&g.init_payload)
                .map_err(|e| format!("init_payload must be a JSON object: {e}"))?;
            if !payload.is_object() {
                return Err("init_payload must be a JSON object".into());
            }
            init["payload"] = payload;
        }
        let state = Self {
            protocol: g.protocol,
            document,
            acked: false,
            done: false,
            payload: None,
        };
        Ok((state, init.to_string()))
    }

    /// The server accepted the connection (`connection_ack`).
    pub(super) fn acked(&self) -> bool {
        self.acked
    }

    /// The frame that ends a running subscription: `complete` (legacy: `stop`).
    pub(super) fn stop_frame(&self) -> Option<String> {
        let kind = match self.protocol {
            WsGraphqlProtocol::TransportWs => "complete",
            WsGraphqlProtocol::Legacy => "stop",
        };
        (self.acked && !self.done).then(|| json!({ "id": ID, "type": kind }).to_string())
    }

    /// The steps after the text frame `text` from the server.
    pub(super) fn on_text(&mut self, text: &str) -> Vec<Step> {
        let Ok(v) = serde_json::from_str::<Value>(text) else {
            return Vec::new();
        };
        let ours = v.get("id").and_then(Value::as_str) == Some(ID);
        let payload = || v.get("payload").map(Value::to_string).unwrap_or_default();
        match v.get("type").and_then(Value::as_str) {
            Some("connection_ack") if !self.acked => {
                self.acked = true;
                let kind = match self.protocol {
                    WsGraphqlProtocol::TransportWs => "subscribe",
                    WsGraphqlProtocol::Legacy => "start",
                };
                let frame = json!({ "id": ID, "type": kind, "payload": self.document });
                vec![Step::Send(frame.to_string())]
            }
            Some("ping") => vec![Step::Send(json!({ "type": "pong" }).to_string())],
            Some("next" | "data") if ours => {
                self.payload = Some(payload().into_bytes());
                Vec::new()
            }
            Some("error") if ours => {
                self.done = true;
                vec![
                    Step::Error(format!("GraphQL error: {}", payload())),
                    Step::Close,
                ]
            }
            Some("connection_error") => {
                self.done = true;
                vec![
                    Step::Error(format!("GraphQL connection_error: {}", payload())),
                    Step::Close,
                ]
            }
            Some("complete") if ours => {
                self.done = true;
                vec![Step::Close]
            }
            // `ka` (legacy keep-alive), `pong` and other ids need no answer.
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn op(protocol: WsGraphqlProtocol) -> WsGraphql {
        WsGraphql {
            query: "subscription { tick }".into(),
            variables: r#"{"n":2}"#.into(),
            operation_name: String::new(),
            protocol,
            init_payload: r#"{"token":"t"}"#.into(),
        }
    }

    #[test]
    fn transport_ws_subscribes_after_the_ack_and_closes_on_complete() {
        let (mut g, init) = GraphqlWs::new(&op(WsGraphqlProtocol::TransportWs)).unwrap();
        assert_eq!(
            init,
            r#"{"type":"connection_init","payload":{"token":"t"}}"#
        );
        assert_eq!(g.stop_frame(), None);
        let steps = g.on_text(r#"{"type":"connection_ack"}"#);
        assert_eq!(
            steps,
            [Step::Send(
                r#"{"id":"1","type":"subscribe","payload":{"query":"subscription { tick }","variables":{"n":2}}}"#
                    .into()
            )]
        );
        assert_eq!(
            g.on_text(r#"{"type":"ping"}"#),
            [Step::Send(r#"{"type":"pong"}"#.into())]
        );
        assert!(
            g.on_text(r#"{"id":"1","type":"next","payload":{"data":{"tick":1}}}"#)
                .is_empty()
        );
        assert_eq!(
            g.payload.as_deref(),
            Some(br#"{"data":{"tick":1}}"#.as_slice())
        );
        assert_eq!(
            g.stop_frame().as_deref(),
            Some(r#"{"id":"1","type":"complete"}"#)
        );
        assert_eq!(g.on_text(r#"{"id":"1","type":"complete"}"#), [Step::Close]);
        assert_eq!(g.stop_frame(), None);
    }

    #[test]
    fn legacy_uses_start_data_and_stop() {
        let (mut g, _) = GraphqlWs::new(&op(WsGraphqlProtocol::Legacy)).unwrap();
        let steps = g.on_text(r#"{"type":"connection_ack"}"#);
        assert!(matches!(&steps[..], [Step::Send(s)] if s.contains(r#""type":"start""#)));
        assert!(g.on_text(r#"{"type":"ka"}"#).is_empty());
        g.on_text(r#"{"id":"1","type":"data","payload":{"data":{"tick":2}}}"#);
        assert!(g.payload.is_some());
        assert_eq!(
            g.stop_frame().as_deref(),
            Some(r#"{"id":"1","type":"stop"}"#)
        );
        let steps = g.on_text(r#"{"id":"1","type":"error","payload":{"message":"no"}}"#);
        assert_eq!(steps.len(), 2);
        assert!(matches!(&steps[0], Step::Error(e) if e.contains("no")));
    }

    #[test]
    fn bad_variables_and_init_payload_are_refused() {
        let mut g = op(WsGraphqlProtocol::TransportWs);
        g.init_payload = "[1]".into();
        assert!(GraphqlWs::new(&g).is_err());
        g.init_payload.clear();
        g.variables = "nope".into();
        assert!(GraphqlWs::new(&g).is_err());
    }
}
