// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) [2024] [Guy Corbaz]
//
// Story 7-2 (AC#7): minimal OPC UA client used as a manual smoke test
// against a locally-running gateway. Connects to one of the three
// configured endpoints with a username/password and exits with status 0
// on session activation, non-zero otherwise.
//
// Usage:
//   cargo run --example opcua_client_smoke -- \
//       --endpoint none --user opcua-user --password "$OPCGW_OPCUA__USER_PASSWORD"
//   cargo run --example opcua_client_smoke -- \
//       --endpoint sign --user opcua-user --password "$OPCGW_OPCUA__USER_PASSWORD"
//   cargo run --example opcua_client_smoke -- \
//       --endpoint sign-encrypt --user opcua-user --password "$OPCGW_OPCUA__USER_PASSWORD"
//
// See `docs/security.md` "Verifying OPC UA security" for the full recipe.

use std::path::PathBuf;
use std::time::Duration;

use clap::{Parser, ValueEnum};
use opcua::client::{ClientBuilder, IdentityToken, Password as ClientPassword};
use opcua::types::{EndpointDescription, MessageSecurityMode, UserTokenPolicy};

#[derive(Parser, Debug)]
#[command(version, about = "OPC UA security smoke-test client (Story 7-2 AC#7)")]
struct Args {
    /// Which endpoint to attempt: `none`, `sign`, or `sign-encrypt`.
    #[arg(long)]
    endpoint: EndpointKind,

    /// Username to submit during authentication.
    #[arg(long)]
    user: String,

    /// Password to submit during authentication.
    #[arg(long)]
    password: String,

    /// Server URL.
    #[arg(long, default_value = "opc.tcp://127.0.0.1:4855/")]
    url: String,

    /// Client PKI directory (auto-created on first run).
    #[arg(long, default_value = "./pki-smoke-client")]
    pki_dir: PathBuf,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum EndpointKind {
    /// `None` security policy / `None` security mode.
    None,
    /// `Basic256` security policy / `Sign` security mode.
    Sign,
    /// `Basic256` security policy / `SignAndEncrypt` security mode.
    SignEncrypt,
}

impl EndpointKind {
    fn policy(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Sign | Self::SignEncrypt => "Basic256",
        }
    }

    fn mode(self) -> MessageSecurityMode {
        match self {
            Self::None => MessageSecurityMode::None,
            Self::Sign => MessageSecurityMode::Sign,
            Self::SignEncrypt => MessageSecurityMode::SignAndEncrypt,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Sign => "Basic256/Sign",
            Self::SignEncrypt => "Basic256/SignAndEncrypt",
        }
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> std::process::ExitCode {
    let args = Args::parse();

    let mut client = match ClientBuilder::new()
        .application_name("opcgw-client-smoke")
        .application_uri("urn:opcgw:smoke-client")
        .product_uri("urn:opcgw:smoke-client")
        .create_sample_keypair(true)
        .trust_server_certs(true)
        .verify_server_certs(false)
        .session_retry_limit(0)
        .session_timeout(5_000)
        .pki_dir(&args.pki_dir)
        .client()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("smoke: failed to build OPC UA client: {e:?}");
            return std::process::ExitCode::from(2);
        }
    };

    let endpoint: EndpointDescription = (
        args.url.as_str(),
        args.endpoint.policy(),
        args.endpoint.mode(),
        UserTokenPolicy::anonymous(),
    )
        .into();

    let identity = IdentityToken::UserName(
        args.user.clone(),
        ClientPassword(args.password.clone()),
    );

    println!(
        "smoke: connecting to {} with endpoint={} as user={}",
        args.url,
        args.endpoint.label(),
        args.user
    );

    let connect = match client
        .connect_to_matching_endpoint(endpoint, identity)
        .await
    {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("smoke: connect_to_matching_endpoint failed: {e:?}");
            return std::process::ExitCode::from(1);
        }
    };

    let (session, event_loop) = connect;
    session.disable_reconnects();
    let handle = event_loop.spawn();

    let connected = tokio::time::timeout(
        Duration::from_secs(5),
        session.wait_for_connection(),
    )
    .await
    .unwrap_or(false);

    let _ = session.disconnect().await;
    handle.abort();
    let _ = handle.await;

    if connected {
        println!("Session established on endpoint={}", args.endpoint.label());
        std::process::ExitCode::from(0)
    } else {
        eprintln!(
            "smoke: session did NOT activate within 5s — check log/opc_ua.log for `event=\"opcua_auth_failed\"`"
        );
        std::process::ExitCode::from(1)
    }
}
