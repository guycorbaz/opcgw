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
                "proto/chirpstack/api/application.proto",
                "proto/chirpstack/api/user.proto",
                "proto/chirpstack/api/internal.proto",
                "proto/chirpstack/api/multicast_group.proto",
                "proto/chirpstack/api/device_profile.proto",
                "proto/chirpstack/api/device_profile_template.proto",
                "proto/chirpstack/api/gateway.proto",
                "proto/chirpstack/api/tenant.proto",
                "proto/chirpstack/api/relay.proto",
                "proto/chirpstack/common/common.proto",
                "proto/chirpstack/gw/gw.proto",
                "proto/chirpstack/integration/integration.proto",
                "proto/chirpstack/google/api/annotations.proto",
                "proto/chirpstack/google/api/http.proto",
            ],
            &["proto"],
        )?;
    Ok(())
}
