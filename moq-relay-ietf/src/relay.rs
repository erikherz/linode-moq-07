use std::net;

use anyhow::Context;

use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
use moq_native_ietf::quic;
use url::Url;

use crate::ws::{WsServer, WsServerConfig};
use crate::{Api, Consumer, Locals, Producer, Remotes, RemotesConsumer, RemotesProducer, Session};

pub struct RelayConfig {
    /// Listen on this address (used for both QUIC/UDP and WebSocket/TCP)
    pub bind: net::SocketAddr,

    /// The TLS configuration.
    pub tls: moq_native_ietf::tls::Config,

    /// Forward all announcements to the (optional) URL.
    pub announce: Option<Url>,

    /// Connect to the HTTP moq-api at this URL.
    pub api: Option<Url>,

    /// Our hostname which we advertise to other origins.
    /// We use QUIC, so the certificate must be valid for this address.
    pub node: Option<Url>,
}

pub struct Relay {
    quic: quic::Endpoint,
    ws: Option<WsServer>,
    announce: Option<Url>,
    locals: Locals,
    api: Option<Api>,
    remotes: Option<(RemotesProducer, RemotesConsumer)>,
}

impl Relay {
    // Create a QUIC endpoint that can be used for both clients and servers.
    pub async fn new(config: RelayConfig) -> anyhow::Result<Self> {
        // Clone TLS config for WebSocket server before it's consumed by QUIC
        let ws_tls_config = config.tls.server.clone();

        let quic = quic::Endpoint::new(quic::Config {
            bind: config.bind,
            tls: config.tls,
        })?;

        // Create WebSocket server on the same bind address (different protocol: TCP vs UDP)
        let ws = if let Some(tls) = ws_tls_config {
            match WsServer::new(WsServerConfig {
                bind: config.bind,
                tls,
            })
            .await
            {
                Ok(server) => {
                    log::info!(
                        "WebSocket server listening on {} (for Safari)",
                        config.bind
                    );
                    Some(server)
                }
                Err(e) => {
                    log::warn!("Failed to start WebSocket server: {}. WebSocket connections will not be accepted.", e);
                    None
                }
            }
        } else {
            None
        };

        let api = if let (Some(url), Some(node)) = (config.api, config.node) {
            log::info!("using moq-api: url={} node={}", url, node);
            Some(Api::new(url, node))
        } else {
            None
        };

        let locals = Locals::new();

        let remotes = api.clone().map(|api| {
            Remotes {
                api,
                quic: quic.client.clone(),
            }
            .produce()
        });

        Ok(Self {
            quic,
            ws,
            announce: config.announce,
            api,
            locals,
            remotes,
        })
    }

    pub async fn run(self) -> anyhow::Result<()> {
        let mut tasks = FuturesUnordered::new();

        let remotes = self.remotes.map(|(producer, consumer)| {
            tasks.push(producer.run().boxed());
            consumer
        });

        let forward = if let Some(url) = &self.announce {
            log::info!("forwarding announces to {}", url);
            let session = self
                .quic
                .client
                .connect(url)
                .await
                .context("failed to establish forward connection")?;
            let (session, publisher, subscriber) =
                moq_transport::session::Session::connect(session)
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
                consumer: Some(Consumer::new(subscriber, self.locals.clone(), None, None)),
            };

            let forward = session.producer.clone();

            tasks.push(async move { session.run().await.context("forwarding failed") }.boxed());

            forward
        } else {
            None
        };

        let mut server = self.quic.server.context("missing TLS certificate")?;
        log::info!("QUIC server listening on {}", server.local_addr()?);

        // WebSocket server (optional - for Safari support)
        let ws_server = self.ws;

        loop {
            tokio::select! {
                // Accept QUIC/WebTransport connections (Chrome, Firefox)
                res = server.accept() => {
                    let conn = res.context("failed to accept QUIC connection")?;

                    let locals = self.locals.clone();
                    let remotes = remotes.clone();
                    let forward = forward.clone();
                    let api = self.api.clone();

                    tasks.push(async move {
                        let (session, publisher, subscriber) = match moq_transport::session::Session::accept(conn).await {
                            Ok(session) => session,
                            Err(err) => {
                                log::warn!("failed to accept MoQ session: {}", err);
                                return Ok(());
                            }
                        };

                        let session = Session {
                            session,
                            producer: publisher.map(|publisher| Producer::new(publisher, locals.clone(), remotes)),
                            consumer: subscriber.map(|subscriber| Consumer::new(subscriber, locals, api, forward)),
                        };

                        if let Err(err) = session.run().await {
                            log::warn!("failed to run MoQ session: {}", err);
                        }

                        Ok(())
                    }.boxed());
                },
                // Accept WebSocket/WebTransport connections (Safari)
                Some(ws_session) = async {
                    match &ws_server {
                        Some(ws) => ws.accept().await,
                        None => std::future::pending().await,
                    }
                } => {
                    log::info!("accepted WebSocket connection from Safari client");

                    // TODO: Full MoQ session handling over WebSocket
                    // For now, the WebSocket session is established but not fully integrated
                    // with the MoQ protocol. This requires making moq-transport generic
                    // over the transport type (see moq_transport::transport module).
                    //
                    // The ws_adapter module provides WsSession which implements the
                    // transport::Session trait, but moq_transport::session::Session
                    // currently only accepts web_transport::Session (QUIC).
                    //
                    // Next steps:
                    // 1. Make moq_transport::session::Session generic over transport::Session
                    // 2. Use WsSession with the generic session handler
                    // 3. Full parity between QUIC and WebSocket connections

                    let _ws_session = crate::ws_adapter::WsSession::new(ws_session);
                    log::debug!("WebSocket session wrapper created, awaiting full MoQ integration");
                },
                res = tasks.next(), if !tasks.is_empty() => res.unwrap()?,
            }
        }
    }
}
