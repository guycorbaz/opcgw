use std::env;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    tonic_build::configure()
        .build_server(false)
        .file_descriptor_set_path(out_dir.join("chirpstack_descriptor.bin"))
        .compile(
            &[
                "proto/chirpstack/api/device.proto",
                "proto/chirpstack/common/common.proto",
                "proto/google/api/annotations.proto",
            ],
            &["proto"],
        )?;
    Ok(())
}
