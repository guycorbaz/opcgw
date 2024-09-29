use std::io::Result;

fn main() -> Result<()> {
    tonic_build::configure()
        .build_server(true)
        .compile_protos(
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
            ],
            &["proto"],
        )?;
    Ok(())
}
