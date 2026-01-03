use clap::Parser;

mod api;
mod consumer;
mod local;
mod producer;
mod relay;
mod remote;
mod session;
mod web;
mod ws;
mod ws_adapter;

pub use api::*;
pub use consumer::*;
pub use local::*;
pub use producer::*;
pub use relay::*;
pub use remote::*;
pub use session::*;
pub use web::*;

use std::net;
use url::Url;

#[derive(Parser, Clone)]
pub struct Cli {
    /// Listen on this address
    #[arg(long, default_value = "[::]:443")]
    pub bind: net::SocketAddr,

    /// The TLS configuration.
    #[command(flatten)]
    pub tls: moq_native_ietf::tls::Args,

    /// Forward all announces to the provided server for authentication/routing.
    /// If not provided, the relay accepts every unique announce.
    #[arg(long)]
    pub announce: Option<Url>,

    /// The URL of the moq-api server in order to run a cluster.
    /// Must be used in conjunction with --node to advertise the origin
    #[arg(long)]
    pub api: Option<Url>,

    /// The hostname that we advertise to other origins.
    /// The provided certificate must be valid for this address.
    #[arg(long)]
    pub node: Option<Url>,

    /// Enable development mode.
    /// This hosts an HTTP web server to serve the fingerprint of the certificate.
    #[arg(long)]
    pub dev: bool,

    /// Bind address for the development HTTP server (used with --dev).
    /// Defaults to port 80 on the same IP as --bind.
    #[arg(long)]
    pub dev_bind: Option<net::SocketAddr>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::init();

    // Disable tracing so we don't get a bunch of Quinn spam.
    let tracer = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(tracing::Level::WARN)
        .finish();
    tracing::subscriber::set_global_default(tracer).unwrap();

    let cli = Cli::parse();
    let tls = cli.tls.load()?;

    if tls.server.is_none() {
        anyhow::bail!("missing TLS certificates");
    }

    // Create a QUIC server for media (with optional WebSocket support for Safari).
    let relay = Relay::new(RelayConfig {
        tls: tls.clone(),
        bind: cli.bind,
        node: cli.node,
        api: cli.api,
        announce: cli.announce,
    })
    .await?;

    if cli.dev {
        // Create an HTTP web server for development.
        // Currently this only contains the certificate fingerprint.
        let dev_bind = cli.dev_bind.unwrap_or_else(|| {
            // Default to port 80 on the same IP as --bind
            net::SocketAddr::new(cli.bind.ip(), 80)
        });

        let web = Web::new(WebConfig {
            bind: dev_bind,
            fingerprints: tls.fingerprints,
        });

        tokio::spawn(async move {
            web.run().await.expect("failed to run dev web server");
        });
    }

    relay.run().await
}
