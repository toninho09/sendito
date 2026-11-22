//! End-to-end tests of the public API against the testkit echo server (HTTP)
//! and real child processes (stdio).

use std::time::Duration;

use sendito_mcp::{Event, McpError, Options, Transport};
use tokio::sync::mpsc;

fn opts(secs: u64) -> Options {
    Options {
        timeout: Duration::from_secs(secs),
    }
}

fn http_transport(url: String) -> Transport {
    Transport::Http {
        url,
        headers: Vec::new(),
        verify_tls: false,
        auth: None,
    }
}

fn drain(rx: &mut mpsc::UnboundedReceiver<Event>) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(e) = rx.try_recv() {
        out.push(e);
    }
    out
}

#[tokio::test]
async fn http_json_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let echo = sendito_testkit::mcp_echo::start().await?;
    let t = http_transport(format!("{}/mcp", echo.url()));
    let (tx, mut rx) = mpsc::unbounded_channel();

    let tools = sendito_mcp::list_tools(&t, &opts(5), tx.clone()).await?;
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].name, "add");
    assert_eq!(
        tools[0].input_schema,
        serde_json::json!({
            "type": "object",
            "properties": { "a": { "type": "number" }, "b": { "type": "number" } }
        })
    );

    let result = sendito_mcp::call_tool(&t, "add", r#"{"a":1,"b":2}"#, &opts(5), tx).await?;
    assert!(!result.is_error);
    assert_eq!(result.text(), "called add {\"a\":1,\"b\":2}");
    assert_eq!(
        result.structured,
        Some(serde_json::json!({ "a": 1, "b": 2 }))
    );

    // The wire: each entry point opens its own session (initialize,
    // initialized, the call), and the session header rides every later POST.
    let captured = echo.captured();
    let methods: Vec<String> = captured.iter().map(|c| c.method_of_body()).collect();
    assert_eq!(
        methods,
        [
            "initialize",
            "notifications/initialized",
            "tools/list",
            "initialize",
            "notifications/initialized",
            "tools/call"
        ]
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
    );
    let sid = captured[2].header("mcp-session-id");
    assert!(sid.as_deref().is_some_and(|s| s.starts_with("sess-")));
    assert_eq!(captured[0].header("mcp-protocol-version"), None);
    for request in &captured[1..3] {
        assert_eq!(request.header("mcp-session-id"), sid);
        assert_eq!(
            request.header("mcp-protocol-version").as_deref(),
            Some("2025-06-18")
        );
    }
    assert!(
        captured[0]
            .body
            .contains("\"clientInfo\":{\"name\":\"sendito\"")
    );

    // The events: the conversation, then a good Finished.
    let events = drain(&mut rx);
    let sent: Vec<String> = events
        .iter()
        .filter_map(|e| match e {
            Event::Sent { json, .. } => Some(json.clone()),
            _ => None,
        })
        .collect();
    assert!(sent[0].contains("\"method\":\"initialize\""));
    assert!(sent.iter().any(|j| j.contains("notifications/initialized")));
    assert!(sent.iter().any(|j| j.contains("\"method\":\"tools/call\"")));
    match events.last() {
        Some(Event::Finished { ok: true, .. }) => {}
        other => panic!("expected a good Finished, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn http_sse_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
    let echo = sendito_testkit::mcp_echo::start().await?;
    let t = http_transport(format!("{}/mcp?format=sse", echo.url()));
    let (tx, _rx) = mpsc::unbounded_channel();
    let result = sendito_mcp::call_tool(&t, "greet", r#"{"name":"Ana"}"#, &opts(5), tx).await?;
    assert_eq!(result.text(), "called greet {\"name\":\"Ana\"}");
    Ok(())
}

#[tokio::test]
async fn http_json_rpc_error() -> Result<(), Box<dyn std::error::Error>> {
    let echo = sendito_testkit::mcp_echo::start().await?;
    let t = http_transport(format!("{}/mcp", echo.url()));
    let (tx, _rx) = mpsc::unbounded_channel();
    match sendito_mcp::call_tool(&t, "nope", "{}", &opts(5), tx).await {
        Err(McpError::Rpc { code, message }) => {
            assert_eq!(code, -32602);
            assert_eq!(message, "unknown tool");
        }
        other => panic!("expected an Rpc error, got {other:?}"),
    }
    Ok(())
}

#[tokio::test]
async fn http_tool_error_is_a_result() -> Result<(), Box<dyn std::error::Error>> {
    let echo = sendito_testkit::mcp_echo::start().await?;
    let t = http_transport(format!("{}/mcp", echo.url()));
    let (tx, _rx) = mpsc::unbounded_channel();
    let result = sendito_mcp::call_tool(&t, "fail-bomb", "{}", &opts(5), tx).await?;
    assert!(result.is_error);
    assert_eq!(result.text(), "boom");
    Ok(())
}

/// A server that accepts and reads but never answers.
async fn silent_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    let addr = listener
        .local_addr()
        .unwrap_or_else(|e| panic!("{e}"))
        .to_string();
    tokio::spawn(async move {
        let mut tmp = [0u8; 4096];
        use tokio::io::AsyncReadExt;
        while let Ok((mut sock, _)) = listener.accept().await {
            tokio::spawn(async move {
                loop {
                    match sock.read(&mut tmp).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
            });
        }
    });
    format!("http://{addr}/mcp")
}

#[tokio::test]
async fn http_timeout_covers_the_whole_call() -> Result<(), Box<dyn std::error::Error>> {
    let url = silent_server().await;
    let t = http_transport(url);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let o = Options {
        timeout: Duration::from_millis(300),
    };
    match sendito_mcp::call_tool(&t, "add", "{}", &o, tx).await {
        Err(McpError::Timeout(d)) => assert_eq!(d, o.timeout),
        other => panic!("expected a Timeout, got {other:?}"),
    }
    let events = drain(&mut rx);
    match events.last() {
        Some(Event::Finished {
            ok: false, message, ..
        }) if message.contains("timeout") => {}
        other => panic!("expected a timeout Finished, got {other:?}"),
    }
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn stdio_success_sends_exact_lines() -> Result<(), Box<dyn std::error::Error>> {
    let script = r#"
while IFS= read -r line; do
  printf 'wire:%s\n' "$line" >&2
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18"}}' ;;
    *'"method":"tools/call"'*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"content":[]}}'; exit ;;
  esac
done
"#;
    let transport = Transport::Stdio {
        command: "sh".into(),
        args: vec!["-c".into(), script.into()],
        env: Vec::new(),
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let result = sendito_mcp::call_tool(&transport, "add", r#"{"a":1}"#, &opts(5), tx).await?;
    assert!(!result.is_error);
    let mut lines = Vec::new();
    while let Ok(Some(event)) = tokio::time::timeout(Duration::from_secs(1), rx.recv()).await {
        if let Event::Log { line, .. } = event
            && let Some(wire) = line.strip_prefix("wire:")
        {
            lines.push(wire.to_string());
            if lines.len() == 3 {
                break;
            }
        }
    }
    assert_eq!(lines, vec![
        format!(r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18","capabilities":{{}},"clientInfo":{{"name":"sendito","version":"{}"}}}}}}"#, env!("CARGO_PKG_VERSION")),
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string(),
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"add","arguments":{"a":1}}}"#.to_string(),
    ]);
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn stdio_forwards_stderr_and_env_then_times_out() -> Result<(), Box<dyn std::error::Error>> {
    let t = Transport::Stdio {
        command: "sh".into(),
        args: vec![
            "-c".into(),
            "echo \"$SF_MCP_ECHO\" >&2; exec sleep 30".into(),
        ],
        env: vec![("SF_MCP_ECHO".into(), "it-works".into())],
    };
    let (tx, mut rx) = mpsc::unbounded_channel();
    let o = Options {
        timeout: Duration::from_millis(500),
    };
    match sendito_mcp::call_tool(&t, "add", "{}", &o, tx).await {
        Err(McpError::Timeout(_)) => {}
        other => panic!("expected a Timeout, got {other:?}"),
    }
    let logs: Vec<String> = drain(&mut rx)
        .into_iter()
        .filter_map(|e| match e {
            Event::Log { line, .. } => Some(line),
            _ => None,
        })
        .collect();
    assert!(logs.iter().any(|l| l == "it-works"));
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
async fn stdio_reports_a_closed_server() -> Result<(), Box<dyn std::error::Error>> {
    let t = Transport::Stdio {
        command: "sh".into(),
        args: vec!["-c".into(), "exit 0".into()],
        env: Vec::new(),
    };
    let (tx, _rx) = mpsc::unbounded_channel();
    let r = sendito_mcp::call_tool(&t, "add", "{}", &opts(5), tx).await;
    assert!(r.is_err());
    Ok(())
}
