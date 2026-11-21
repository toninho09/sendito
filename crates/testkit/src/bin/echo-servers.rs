//! Starts the HTTP echo server and the gRPC echo server for demos and manual tests.
//!
//! Usage: echo-servers [HTTP_ADDR] [GRPC_ADDR]   (defaults 127.0.0.1:8787, 127.0.0.1:50051)

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut args = std::env::args().skip(1);
    let http = args.next().unwrap_or_else(|| "127.0.0.1:8787".into());
    let grpc = args.next().unwrap_or_else(|| "127.0.0.1:50051".into());
    let h = sendito_testkit::http_echo::start_on(&http).await?;
    let g = sendito_testkit::grpc_echo::start_on(&grpc, true).await?;
    println!(
        "HTTP echo on {} · gRPC echo (with reflection) on {}",
        h.url(),
        g.target()
    );
    tokio::signal::ctrl_c().await?;
    Ok(())
}
