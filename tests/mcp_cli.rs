//! Headless MCP CLI integration against the local echo server.

use std::process::Command;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn run_reports_success_tool_error_and_failures() -> Result<(), Box<dyn std::error::Error>> {
    let echo = sendito_testkit::mcp_echo::start().await?;
    let dir = tempfile::tempdir()?;
    std::fs::create_dir(dir.path().join(".sendito"))?;
    std::fs::write(
        dir.path().join(".sendito/config.toml"),
        format!("[mcp.servers.echo]\nurl = \"{}/mcp\"\n", echo.url()),
    )?;
    for (name, tool) in [
        ("ok", "add"),
        ("tool-error", "fail-test"),
        ("rpc-error", "unknown"),
    ] {
        std::fs::write(
            dir.path().join(format!("{name}.toml")),
            format!("kind = \"mcp\"\nserver = \"echo\"\ntool = \"{tool}\"\n"),
        )?;
    }
    std::fs::write(
        dir.path().join("assertion.toml"),
        "kind = \"mcp\"\nserver = \"echo\"\ntool = \"add\"\n\n[[scripts.assert]]\nfrom = \"status\"\nop = \"eq\"\nvalue = \"1\"\n",
    )?;
    let path = dir.path().to_path_buf();
    let outputs = tokio::task::spawn_blocking(move || {
        ["ok", "tool-error", "assertion", "rpc-error", "missing"].map(|name| {
            Command::new(env!("CARGO_BIN_EXE_sendito"))
                .current_dir(&path)
                .env("XDG_CONFIG_HOME", path.join("config"))
                .args(["run", name, "--no-history"])
                .output()
        })
    })
    .await?;
    let [ok, tool_error, assertion, rpc_error, usage] = outputs;
    let (ok, tool_error, assertion, rpc_error, usage) =
        (ok?, tool_error?, assertion?, rpc_error?, usage?);
    assert_eq!(ok.status.code(), Some(0));
    assert_eq!(tool_error.status.code(), Some(1));
    assert_eq!(assertion.status.code(), Some(1));
    assert_eq!(rpc_error.status.code(), Some(2));
    assert_eq!(usage.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&tool_error.stdout).contains("isError"));
    Ok(())
}
