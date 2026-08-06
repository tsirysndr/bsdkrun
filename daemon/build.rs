use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("cargo:rerun-if-changed=proto/bsdkrun.proto");
    let out_dir = PathBuf::from(std::env::var("OUT_DIR")?);
    tonic_prost_build::configure()
        .build_client(true)
        .build_server(true)
        // Emitted so the daemon can serve gRPC reflection, which is what lets
        // `grpcurl`/Postman discover the API without a local copy of the proto.
        .file_descriptor_set_path(out_dir.join("bsdkrun_descriptor.bin"))
        .compile_protos(&["proto/bsdkrun.proto"], &["proto"])?;
    Ok(())
}
