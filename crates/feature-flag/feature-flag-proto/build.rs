use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let descriptor_path =
        PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("featureflag_descriptor.bin");

    tonic_prost_build::configure()
        .build_server(true)
        .build_client(true)
        .file_descriptor_set_path(&descriptor_path)
        .compile_protos(
            &[
                "proto/featureflag/v1/common.proto",
                "proto/featureflag/v1/evaluation.proto",
                "proto/featureflag/v1/admin.proto",
            ],
            &["proto"],
        )?;

    println!("cargo:rerun-if-changed=proto");
    Ok(())
}
