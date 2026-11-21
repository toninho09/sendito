use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto");
    let fds = protox::compile(["sendito/test/v1/echo.proto"], ["proto"])?;
    let out = PathBuf::from(std::env::var("OUT_DIR")?);
    std::fs::write(
        out.join("echo_descriptor.bin"),
        prost::Message::encode_to_vec(&fds),
    )?;
    tonic_prost_build::configure()
        .build_client(false)
        .codec_path("crate::grpc_echo::RawCodec")
        .compile_fds(fds)?;
    Ok(())
}
