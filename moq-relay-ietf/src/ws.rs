//! WebSocket server for Safari WebTransport support.
//!
//! This module provides a TCP/TLS WebSocket listener that can accept
//! WebTransport connections from browsers that don't support native WebTransport.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// WebSocket server configuration.
pub struct WsServerConfig {
    /// Address to bind to (TCP).
    pub bind: SocketAddr,
    /// TLS configuration (shared with QUIC).
    pub tls: rustls::ServerConfig,
}

/// A WebSocket server for WebTransport connections.
pub struct WsServer {
    listener: TcpListener,
    tls_acceptor: TlsAcceptor,
}

impl WsServer {
    /// Create a new WebSocket server.
    pub async fn new(config: WsServerConfig) -> anyhow::Result<Self> {
        let listener = TcpListener::bind(config.bind)
            .await
            .context("failed to bind TCP listener for WebSocket")?;

        log::info!("WebSocket server listening on {}", config.bind);

        let tls_acceptor = TlsAcceptor::from(Arc::new(config.tls));

        Ok(Self {
            listener,
            tls_acceptor,
        })
    }

    /// Get the local address the server is bound to.
    pub fn local_addr(&self) -> anyhow::Result<SocketAddr> {
        self.listener
            .local_addr()
            .context("failed to get local address")
    }

    /// Accept a new WebSocket WebTransport connection.
    ///
    /// This performs:
    /// 1. TCP accept
    /// 2. TLS handshake
    /// 3. WebSocket upgrade with "webtransport" subprotocol
    /// 4. Returns a web_transport_ws::Session
    pub async fn accept(&self) -> Option<web_transport_ws::Session> {
        loop {
            match self.accept_inner().await {
                Ok(session) => return Some(session),
                Err(e) => {
                    log::warn!("failed to accept WebSocket connection: {}", e);
                    continue;
                }
            }
        }
    }

    async fn accept_inner(&self) -> anyhow::Result<web_transport_ws::Session> {
        // Accept TCP connection
        let (tcp_stream, peer_addr) = self
            .listener
            .accept()
            .await
            .context("failed to accept TCP connection")?;

        log::debug!("accepted TCP connection from {}", peer_addr);

        // Perform TLS handshake
        let tls_stream = self
            .tls_acceptor
            .accept(tcp_stream)
            .await
            .context("TLS handshake failed")?;

        log::debug!("TLS handshake complete for {}", peer_addr);

        // Accept WebSocket with "webtransport" subprotocol
        let session = web_transport_ws::Session::accept(tls_stream)
            .await
            .context("WebSocket/WebTransport handshake failed")?;

        log::info!("WebSocket WebTransport session established from {}", peer_addr);

        Ok(session)
    }
}
