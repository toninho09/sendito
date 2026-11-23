//! Starts the echo servers for demos and manual tests: HTTP, gRPC (with
//! reflection), WebSocket and MCP (Streamable HTTP + SSE fallback).
//!
//! Usage: echo-servers [HTTP_ADDR] [GRPC_ADDR] [WS_ADDR] [MCP_ADDR]
//! (defaults 127.0.0.1:8787, 127.0.0.1:50051, 127.0.0.1:8788, 127.0.0.1:8789)

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let http = args.next().unwrap_or_else(|| "127.0.0.1:8787".into());
    let grpc = args.next().unwrap_or_else(|| "127.0.0.1:50051".into());
    let ws = args.next().unwrap_or_else(|| "127.0.0.1:8788".into());
    let mcp = args.next().unwrap_or_else(|| "127.0.0.1:8789".into());
    let h = sendito_testkit::http_echo::start_on(&http).await?;
    let g = sendito_testkit::grpc_echo::start_on(&grpc, true).await?;
    let w = sendito_testkit::ws_echo::start_on(&ws).await?;
    let m = sendito_testkit::mcp_echo::start_on(&mcp).await?;
    println!(
        "HTTP echo on {}\ngRPC echo (with reflection) on {}\nWS echo on {} (any path)\nMCP echo on {}/mcp and {}/mcp/sse",
        h.url(),
        g.target(),
        w.url(""),
        m.url(),
        m.url()
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
