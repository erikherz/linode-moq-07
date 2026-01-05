use std::net;

use anyhow::Context;

use moq_native_ietf::quic;
use web_transport;
use url::Url;

use crate::ws::{WsServer, WsServerConfig};
use crate::ws_adapter::WsSession;
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
        let remotes = self.remotes.map(|(producer, consumer)| {
            tokio::spawn(async move {
                if let Err(err) = producer.run().await {
                    log::warn!("remotes producer error: {}", err);
                }
            });
            consumer
        });

        let (forward, upstream) = if let Some(url) = &self.announce {
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
            // This session doesn't need an upstream since it IS the upstream.
            // Clone the subscriber before moving it into Consumer - we need it for upstream queries
            let upstream_subscriber = subscriber.clone();

            let session = Session {
                session,
                producer: Some(Producer::new(
                    publisher,
                    self.locals.clone(),
                    remotes.clone(),
                    None, // No upstream for the Cloudflare connection itself
                )),
                consumer: Some(Consumer::new(subscriber, self.locals.clone(), None, None)),
            };

            let forward = session.producer.clone();

            tokio::spawn(async move {
                if let Err(err) = session.run().await {
                    log::warn!("forwarding session error: {}", err);
                }
            });

            (forward, Some(upstream_subscriber))
        } else {
            (None, None)
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
                    let upstream = upstream.clone();
                    let api = self.api.clone();

                    tokio::spawn(async move {
                        if let Err(err) = Self::handle_quic_session(conn, locals, remotes, forward, upstream, api).await {
                            log::warn!("QUIC session error: {}", err);
                        }
                    });
                },
                // Accept WebSocket/WebTransport connections (Safari)
                Some(ws_conn) = async {
                    match &ws_server {
                        Some(ws) => ws.accept().await,
                        None => std::future::pending().await,
                    }
                } => {
                    log::info!("accepted WebSocket connection from Safari client");

                    let locals = self.locals.clone();
                    let remotes = remotes.clone();
                    let forward = forward.clone();
                    let upstream = upstream.clone();
                    let api = self.api.clone();

                    // Wrap the WebSocket session in our adapter
                    let ws_session = WsSession::new(ws_conn);

                    tokio::spawn(async move {
                        if let Err(err) = Self::handle_ws_session(ws_session, locals, remotes, forward, upstream, api).await {
                            log::warn!("WebSocket session error: {}", err);
                        }
                    });
                },
            }
        }
    }

    /// Handle a QUIC connection (Chrome, Firefox)
    async fn handle_quic_session(
        conn: web_transport::Session,
        locals: Locals,
        remotes: Option<RemotesConsumer>,
        forward: Option<Producer<web_transport::Session>>,
        upstream: Option<moq_transport::session::Subscriber<web_transport::Session>>,
        api: Option<Api>,
    ) -> anyhow::Result<()> {
        let (session, publisher, subscriber) = match moq_transport::session::Session::accept(conn).await {
            Ok(session) => session,
            Err(err) => {
                log::warn!("failed to accept MoQ session (QUIC): {}", err);
                return Ok(());
            }
        };

        let session = Session {
            session,
            // Pass upstream so this Producer can fetch unknown streams from Cloudflare
            producer: publisher.map(|publisher| Producer::new(publisher, locals.clone(), remotes, upstream)),
            consumer: subscriber.map(|subscriber| Consumer::new(subscriber, locals, api, forward)),
        };

        if let Err(err) = session.run().await {
            log::warn!("failed to run MoQ session (QUIC): {}", err);
        }

        Ok(())
    }

    /// Handle a WebSocket connection (Safari)
    async fn handle_ws_session(
        ws_session: WsSession,
        locals: Locals,
        remotes: Option<RemotesConsumer>,
        forward: Option<Producer<web_transport::Session>>,
        upstream: Option<moq_transport::session::Subscriber<web_transport::Session>>,
        api: Option<Api>,
    ) -> anyhow::Result<()> {
        let (session, publisher, subscriber) = match moq_transport::session::Session::accept(ws_session).await {
            Ok(session) => session,
            Err(err) => {
                log::warn!("failed to accept MoQ session (WebSocket): {}", err);
                return Ok(());
            }
        };

        let session = Session {
            session,
            // Pass upstream so this Producer can fetch unknown streams from Cloudflare
            producer: publisher.map(|publisher| Producer::new(publisher, locals.clone(), remotes, upstream)),
            consumer: subscriber.map(|subscriber| Consumer::new(subscriber, locals, api, forward)),
        };

        if let Err(err) = session.run().await {
            log::warn!("failed to run MoQ session (WebSocket): {}", err);
        }

        // Yield to allow any pending cleanup tasks to run
        tokio::task::yield_now().await;

        Ok(())
    }
}
