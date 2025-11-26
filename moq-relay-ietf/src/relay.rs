use std::{net, path::PathBuf, sync::Arc};

use anyhow::Context;

use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
use moq_native_ietf::quic;
use url::Url;

use crate::{
    Api, Consumer, Coordinator, Locals, Producer, Remotes, RemotesConsumer, RemotesProducer,
    Session,
};

/// Configuration for the relay.
pub struct RelayConfig {
    /// Listen on this address
    pub bind: net::SocketAddr,

    /// The TLS configuration.
    pub tls: moq_native_ietf::tls::Config,

    /// Directory to write qlog files (one per connection)
    pub qlog_dir: Option<PathBuf>,

    /// Directory to write mlog files (one per connection)
    pub mlog_dir: Option<PathBuf>,

    /// Forward all announcements to the (optional) URL.
    pub announce: Option<Url>,

    /// Connect to the HTTP moq-api at this URL.
    pub api: Option<Url>,

    /// Our hostname which we advertise to other origins.
    /// We use QUIC, so the certificate must be valid for this address.
    pub node: Option<Url>,

    /// The coordinator for namespace/track registration and discovery.
    pub coordinator: Arc<dyn Coordinator>,
}

/// MoQ Relay server.
pub struct Relay {
    quic: quic::Endpoint,
    announce_url: Option<Url>,
    mlog_dir: Option<PathBuf>,
    locals: Locals,
    api: Option<Api>,
    remotes: Option<(RemotesProducer, RemotesConsumer)>,
    coordinator: Arc<dyn Coordinator>,
}

impl Relay {
    pub fn new(config: RelayConfig) -> anyhow::Result<Self> {
        // Create a QUIC endpoint that can be used for both clients and servers.
        let quic = quic::Endpoint::new(quic::Config {
            bind: config.bind,
            qlog_dir: config.qlog_dir,
            tls: config.tls,
        })?;

        // Validate mlog directory if provided
        if let Some(mlog_dir) = &config.mlog_dir {
            if !mlog_dir.exists() {
                anyhow::bail!("mlog directory does not exist: {}", mlog_dir.display());
            }
            if !mlog_dir.is_dir() {
                anyhow::bail!("mlog path is not a directory: {}", mlog_dir.display());
            }
            log::info!("mlog output enabled: {}", mlog_dir.display());
        }

        // Create an API client if we have the necessary configuration
        let api = if let (Some(url), Some(node)) = (config.api, config.node) {
            log::info!("using moq-api: url={} node={}", url, node);
            Some(Api::new(url, node))
        } else {
            None
        };

        let locals = Locals::new();

        // Create remotes if we have an API client
        let remotes = api.clone().map(|api| {
            Remotes {
                api,
                quic: quic.client.clone(),
            }
            .produce()
        });

        Ok(Self {
            quic,
            announce_url: config.announce,
            mlog_dir: config.mlog_dir,
            api,
            locals,
            remotes,
            coordinator: config.coordinator,
        })
    }

    /// Run the relay server.
    pub async fn run(self) -> anyhow::Result<()> {
        let mut tasks = FuturesUnordered::new();

        // Start the remotes producer task, if any
        let remotes = self.remotes.map(|(producer, consumer)| {
            tasks.push(producer.run().boxed());
            consumer
        });

        // Start the forwarder, if any
        let forward_producer = if let Some(url) = &self.announce_url {
            log::info!("forwarding announces to {}", url);

            // Establish a QUIC connection to the forward URL
            let (session, _quic_client_initial_cid) = self
                .quic
                .client
                .connect(url)
                .await
                .context("failed to establish forward connection")?;

            // Create the MoQ session over the connection
            let (session, publisher, subscriber) =
                moq_transport::session::Session::connect(session, None)
                    .await
                    .context("failed to establish forward session")?;

            // Create a normal looking session, except we never forward or register announces.
            let session = Session {
                session,
                producer: Some(Producer::new(
                    publisher,
                    self.locals.clone(),
                    remotes.clone(),
                )),
                consumer: Some(Consumer::new(
                    subscriber,
                    self.locals.clone(),
                    self.coordinator.clone(),
                    None,
                )),
            };

            let forward_producer = session.producer.clone();

            tasks.push(async move { session.run().await.context("forwarding failed") }.boxed());

            forward_producer
        } else {
            None
        };

        // Start the QUIC server loop
        let mut server = self.quic.server.context("missing TLS certificate")?;
        log::info!("listening on {}", server.local_addr()?);

        loop {
            tokio::select! {
                // Accept a new QUIC connection
                res = server.accept() => {
                    let (conn, connection_id) = res.context("failed to accept QUIC connection")?;

                    // Construct mlog path from connection ID if mlog directory is configured
                    let mlog_path = self.mlog_dir.as_ref()
                        .map(|dir| dir.join(format!("{}_server.mlog", connection_id)));

                    let locals = self.locals.clone();
                    let remotes = remotes.clone();
                    let forward = forward_producer.clone();
                    let coordinator = self.coordinator.clone();

                    // Spawn a new task to handle the connection
                    tasks.push(async move {

                        // Create the MoQ session over the connection (setup handshake etc)
                        let (session, publisher, subscriber) = match moq_transport::session::Session::accept(conn, mlog_path).await {
                            Ok(session) => session,
                            Err(err) => {
                                log::warn!("failed to accept MoQ session: {}", err);
                                return Ok(());
                            }
                        };

                        // Create our MoQ relay session
                        let session = Session {
                            session,
                            producer: publisher.map(|publisher| Producer::new(publisher, locals.clone(), remotes)),
                            consumer: subscriber.map(|subscriber| Consumer::new(subscriber, locals, coordinator, forward)),
                        };

                        if let Err(err) = session.run().await {
                            log::warn!("failed to run MoQ session: {}", err);
                        }

                        Ok(())
                    }.boxed());
                },
                res = tasks.next(), if !tasks.is_empty() => res.unwrap()?,
            }
        }
    }
}
