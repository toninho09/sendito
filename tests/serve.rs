//! `sendito serve`: Sendito's own MCP client talks to the server, which sends
//! requests to the local echo server. Cancellation is driven with raw NDJSON.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use sendito_mcp::{Options, ToolResult, Transport, call_tool, list_catalog};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

type Res = Result<(), Box<dyn std::error::Error>>;

const SECRET: &str = "s3cret-token-42";

/// A collection with an environment (one plain, one secret variable) and
/// one request that sends the secret to the echo server.
fn collection(base: &str) -> std::io::Result<tempfile::TempDir> {
    let dir = tempfile::tempdir()?;
    let meta = dir.path().join(".sendito");
    std::fs::create_dir_all(meta.join("environments"))?;
    std::fs::write(
        meta.join("environments/dev.toml"),
        format!("[variables]\nbase = \"{base}\"\ntoken = {{ source = \"local\" }}\n"),
    )?;
    std::fs::write(
        meta.join("secrets.toml"),
        format!("[dev]\ntoken = \"{SECRET}\"\n"),
    )?;
    std::fs::create_dir(dir.path().join("Users"))?;
    std::fs::write(
        dir.path().join("Users/echo.toml"),
        "kind = \"http\"\nmethod = \"GET\"\nurl = \"{{base}}/echo?key={{token}}\"\n\n\
         [[headers]]\nname = \"Authorization\"\nvalue = \"Bearer {{token}}\"\n\n\
         [[scripts.assert]]\nfrom = \"status\"\nop = \"eq\"\nvalue = \"200\"\n",
    )?;
    Ok(dir)
}

fn transport(dir: &Path, extra: &[&str]) -> Transport {
    let mut args = vec![
        "serve".to_string(),
        dir.display().to_string(),
        "--env".into(),
        "dev".into(),
        "--no-history".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    Transport::Stdio {
        command: env!("CARGO_BIN_EXE_sendito").into(),
        args,
        env: vec![
            ("SENDITO_KEYCHAIN".into(), "memory".into()),
            (
                "XDG_CONFIG_HOME".into(),
                dir.join("config").display().to_string(),
            ),
        ],
    }
}

async fn call(t: &Transport, tool: &str, args: Value) -> Result<ToolResult, sendito_mcp::McpError> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    call_tool(t, tool, &args.to_string(), &Options::default(), tx).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_agent_lists_runs_and_writes_requests() -> Res {
    let echo = sendito_testkit::http_echo::start().await?;
    let dir = collection(&echo.url())?;
    let t = transport(dir.path(), &[]);

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let catalog = list_catalog(&t, &Options::default(), tx).await?;
    let names: Vec<&str> = catalog.tools.iter().map(|t| t.name.as_str()).collect();
    for name in [
        "list_requests",
        "run_request",
        "send_request",
        "write_request",
        "delete",
    ] {
        assert!(names.contains(&name), "{names:?}");
    }
    assert_eq!(catalog.resources[0].uri, "sendito://format");

    let r = call(&t, "list_requests", json!({})).await?;
    let s = r.structured.clone().unwrap_or_default();
    assert_eq!(s["requests"][0]["path"], "Users/echo.toml", "{s}");

    // The secret reaches the server, and the echo of it comes back masked.
    let r = call(&t, "run_request", json!({"path": "Users/echo"})).await?;
    assert!(!r.is_error, "{}", r.text());
    let s = r.structured.clone().unwrap_or_default();
    assert_eq!(s["outcome"], "pass", "{s}");
    assert_eq!(s["http"]["status"], 200);
    let body = s["body"].as_str().unwrap_or_default();
    assert!(body.contains("Bearer "), "{body}");
    assert!(!r.raw.to_string().contains(SECRET), "{}", r.raw);
    // The final URL holds the secret in its query: it comes back masked.
    let url = s["http"]["url"].as_str().unwrap_or_default();
    assert!(url.contains("/echo?key="), "{s}");
    let sent = echo.last().ok_or("the echo server got nothing")?;
    assert_eq!(
        sent.header("authorization"),
        Some(format!("Bearer {SECRET}"))
    );

    let r = call(&t, "list_environments", json!({})).await?;
    assert!(!r.raw.to_string().contains(SECRET), "{}", r.raw);

    // A request given as TOML, with a $prompt value.
    let toml = "kind = \"http\"\nmethod = \"POST\"\nurl = \"{{base}}/echo\"\n\n[body]\ntype = \"raw\"\ncontent_type = \"text/plain\"\ntext = \"id={{ $prompt('Id') }}\"\n";
    let r = call(
        &t,
        "send_request",
        json!({"toml": toml, "prompts": {"Id": "7"}}),
    )
    .await?;
    let s = r.structured.clone().unwrap_or_default();
    assert_eq!(s["outcome"], "pass", "{s}");
    assert!(
        s["body"].as_str().unwrap_or_default().contains("id=7"),
        "{s}"
    );

    // Without the value the request is not sent, and the agent reads why.
    let r = call(&t, "send_request", json!({"toml": toml})).await?;
    let s = r.structured.clone().unwrap_or_default();
    assert_eq!(s["outcome"], "error", "{s}");
    assert!(
        s["error"].as_str().unwrap_or_default().contains("Id"),
        "{s}"
    );

    let r = call(
        &t,
        "write_request",
        json!({"path": "Admin/new", "toml": toml}),
    )
    .await?;
    assert!(!r.is_error, "{}", r.text());
    assert!(dir.path().join("Admin/new.toml").is_file());
    let r = call(&t, "run_folder", json!({"folder": "Users"})).await?;
    let s = r.structured.clone().unwrap_or_default();
    assert_eq!(
        (s["total"].clone(), s["passed"].clone()),
        (json!(1), json!(1)),
        "{s}"
    );
    let r = call(&t, "delete", json!({"path": "Admin"})).await?;
    assert!(!r.is_error, "{}", r.text());
    assert!(!dir.path().join("Admin").exists());

    let r = call(&t, "read_request", json!({"path": ".sendito/secrets.toml"})).await?;
    assert!(r.is_error);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_only_offers_no_tool_that_writes() -> Res {
    let dir = collection("http://127.0.0.1:9")?;
    let t = transport(dir.path(), &["--read-only"]);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let catalog = list_catalog(&t, &Options::default(), tx).await?;
    let names: Vec<&str> = catalog.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"run_request"), "{names:?}");
    for name in ["write_request", "create_folder", "move", "delete"] {
        assert!(!names.contains(&name), "{names:?}");
    }
    let refused = call(&t, "delete", json!({"path": "Users"})).await;
    assert!(refused.is_err(), "{refused:?}");
    assert!(dir.path().join("Users/echo.toml").is_file());
    Ok(())
}

type Lines = tokio::io::Lines<BufReader<tokio::process::ChildStdout>>;

/// `sendito serve` driven with raw NDJSON, after the handshake.
async fn raw_server(
    dir: &Path,
) -> Result<(tokio::process::Child, tokio::process::ChildStdin, Lines), Box<dyn std::error::Error>>
{
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_sendito"))
        .args([
            "serve",
            &dir.display().to_string(),
            "--env",
            "dev",
            "--no-history",
        ])
        .env("SENDITO_KEYCHAIN", "memory")
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdin = child.stdin.take().ok_or("no stdin")?;
    let mut lines = BufReader::new(child.stdout.take().ok_or("no stdout")?).lines();
    let msgs = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    ];
    for m in msgs {
        stdin.write_all(format!("{m}\n").as_bytes()).await?;
    }
    let first: Value = serde_json::from_str(&lines.next_line().await?.ok_or("closed")?)?;
    assert_eq!(first["id"], 1);
    Ok((child, stdin, lines))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_folder_run_reports_progress_and_the_failures() -> Res {
    let echo = sendito_testkit::http_echo::start().await?;
    let dir = collection(&echo.url())?;
    std::fs::write(
        dir.path().join("Users/fail.toml"),
        "kind = \"http\"\nmethod = \"GET\"\nurl = \"{{base}}/echo\"\n\n\
         [[scripts.assert]]\nfrom = \"status\"\nop = \"eq\"\nvalue = \"201\"\n",
    )?;
    let (_child, mut stdin, mut lines) = raw_server(dir.path()).await?;
    let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{
        "name":"run_folder",
        "arguments":{"folder":"Users","failures":true},
        "_meta":{"progressToken":"t"},
    }});
    stdin.write_all(format!("{call}\n").as_bytes()).await?;
    let mut progress = Vec::new();
    let result = loop {
        let line = tokio::time::timeout(Duration::from_secs(20), lines.next_line()).await??;
        let msg: Value = serde_json::from_str(&line.ok_or("closed")?)?;
        if msg["method"] == "notifications/progress" {
            progress.push(msg["params"].clone());
        } else {
            break msg;
        }
    };
    assert_eq!(progress.len(), 2, "{progress:?}");
    assert_eq!(progress[1]["progressToken"], "t");
    assert_eq!(
        (
            progress[1]["progress"].clone(),
            progress[1]["total"].clone()
        ),
        (json!(2), json!(2))
    );
    let s = &result["result"]["structuredContent"];
    assert_eq!(s["passed"], 1, "{s}");
    let failures = s["failures"].as_array().ok_or("no failures")?;
    assert_eq!(failures.len(), 1, "{s}");
    assert_eq!(failures[0]["path"], "Users/fail.toml", "{s}");
    assert!(failures[0]["body"].is_string(), "{s}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_send_offers_no_tool_that_sends() -> Res {
    let dir = collection("http://127.0.0.1:9")?;
    let t = transport(dir.path(), &["--no-send"]);
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let catalog = list_catalog(&t, &Options::default(), tx).await?;
    let names: Vec<&str> = catalog.tools.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"write_request"), "{names:?}");
    for name in ["run_request", "run_folder", "send_request"] {
        assert!(!names.contains(&name), "{names:?}");
    }
    assert!(
        call(&t, "run_request", json!({"path": "Users/echo"}))
            .await
            .is_err()
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cancelled_call_gets_no_answer_and_the_server_goes_on() -> Res {
    // A server that accepts and never answers: the send waits until cancelled.
    let silent = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let addr = silent.local_addr()?;
    tokio::spawn(async move {
        let mut kept = Vec::new();
        while let Ok((sock, _)) = silent.accept().await {
            kept.push(sock);
        }
    });
    let dir = collection(&format!("http://{addr}"))?;
    let (mut child, mut stdin, mut lines) = raw_server(dir.path()).await?;
    let call = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"run_request","arguments":{"path":"Users/echo"}}});
    stdin.write_all(format!("{call}\n").as_bytes()).await?;
    tokio::time::sleep(Duration::from_millis(300)).await;
    // A second call with the id of a call in flight is refused.
    stdin.write_all(format!("{call}\n").as_bytes()).await?;
    let refused: Value = serde_json::from_str(&lines.next_line().await?.ok_or("closed")?)?;
    assert_eq!(refused["id"], 2, "{refused}");
    assert_eq!(refused["error"]["code"], -32600, "{refused}");
    let cancel =
        json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}});
    stdin.write_all(format!("{cancel}\n").as_bytes()).await?;
    stdin
        .write_all(format!("{}\n", json!({"jsonrpc":"2.0","id":3,"method":"ping"})).as_bytes())
        .await?;
    let next = tokio::time::timeout(Duration::from_secs(10), lines.next_line()).await??;
    let next: Value = serde_json::from_str(&next.ok_or("closed")?)?;
    assert_eq!(next["id"], 3, "the cancelled call must not answer: {next}");
    drop(stdin);
    let status = tokio::time::timeout(Duration::from_secs(10), child.wait()).await??;
    assert!(status.success());
    let rest = lines.next_line().await?;
    assert!(rest.is_none(), "{rest:?}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grpc_and_websocket_answers_come_back_as_messages() -> Res {
    let grpc = sendito_testkit::grpc_echo::start()
        .await
        .map_err(|e| e.to_string())?;
    let ws = sendito_testkit::ws_echo::start().await?;
    let dir = collection("http://127.0.0.1:9")?;
    let t = transport(dir.path(), &[]);
    let unary = format!(
        "kind = \"grpc\"\ntarget = \"{}\"\nservice = \"sendito.test.v1.Echo\"\nmethod = \"Unary\"\nmessage = '{{\"text\": \"hello\"}}'\n",
        grpc.target()
    );
    let r = call(&t, "send_request", json!({"toml": unary})).await?;
    let s = r.structured.clone().unwrap_or_default();
    assert_eq!(s["outcome"], "pass", "{s}");
    assert!(
        s["messages"][0]
            .as_str()
            .unwrap_or_default()
            .contains("hello"),
        "{s}"
    );

    let echo = format!(
        "kind = \"websocket\"\nurl = \"{}\"\nmessages = '[\"ping-1\"]'\n",
        ws.url("/echo")
    );
    let r = call(&t, "send_request", json!({"toml": echo})).await?;
    let s = r.structured.clone().unwrap_or_default();
    assert!(
        s["messages"]
            .as_array()
            .is_some_and(|m| m.iter().any(|m| m == "ping-1")),
        "{s}"
    );
    Ok(())
}
