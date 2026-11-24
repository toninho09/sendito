//! JSON-RPC 2.0 messages and MCP protocol constants.
//!
//! Messages are exchanged as single-line JSON: NDJSON on stdio and one object
//! per HTTP POST (batching was removed from the spec).

use serde_json::{Value, json};

/// The protocol version Sendito proposes in `initialize`.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

/// Versions the server may answer with that we know how to speak. A different
/// one logs a warning and the session continues.
pub const SUPPORTED_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];

/// A JSON-RPC request. `id` starts at 1 and grows by one per request.
pub fn request(id: u64, method: &str, params: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// A JSON-RPC notification (no `id`, no response expected).
pub fn notification(method: &str) -> Value {
    json!({ "jsonrpc": "2.0", "method": method })
}

/// The `initialize` parameters: our latest protocol version, no client
/// capabilities (we only call tools) and the client name and version.
pub fn initialize_params() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": { "name": "sendito", "version": env!("CARGO_PKG_VERSION") }
    })
}

/// `true` when `v` is a response to the request with this `id`. Notifications
/// have no `id`, and a request from the server (such as `ping`) has a
/// `method`: both are logged, not matched.
pub fn is_response_to(v: &Value, id: u64) -> bool {
    v.get("id") == Some(&json!(id))
        && v.get("method").is_none()
        && (v.get("result").is_some() || v.get("error").is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_server_request_with_the_same_id_is_not_the_response() {
        assert!(!is_response_to(
            &json!({"jsonrpc":"2.0","id":2,"method":"ping"}),
            2
        ));
        assert!(!is_response_to(&json!({"jsonrpc":"2.0","id":2}), 2));
        assert!(is_response_to(
            &json!({"jsonrpc":"2.0","id":2,"result":{}}),
            2
        ));
        assert!(is_response_to(
            &json!({"jsonrpc":"2.0","id":2,"error":{}}),
            2
        ));
    }

    #[test]
    fn matches_only_the_same_id() {
        let msg = json!({ "jsonrpc": "2.0", "id": 2, "result": {} });
        assert!(is_response_to(&msg, 2));
        assert!(!is_response_to(&msg, 3));
        let note = json!({ "jsonrpc": "2.0", "method": "notifications/message" });
        assert!(!is_response_to(&note, 1));
    }
}
