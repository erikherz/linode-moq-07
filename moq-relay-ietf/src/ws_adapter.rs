//! WebSocket transport implementation for moq-transport.
//!
//! This module provides wrapper types that implement the moq_transport::transport traits
//! for web_transport_ws types, enabling WebSocket-based MoQ connections.

use bytes::Bytes;
use moq_transport::transport;
use thiserror::Error;
use web_transport_trait as wt;

/// Error type for WebSocket transport operations.
#[derive(Error, Debug, Clone)]
pub enum WsError {
    #[error("WebSocket error: {0}")]
    Ws(String),
    #[error("connection closed")]
    Closed,
    #[error("datagrams not supported")]
    DatagramsNotSupported,
}

impl From<web_transport_ws::Error> for WsError {
    fn from(e: web_transport_ws::Error) -> Self {
        WsError::Ws(e.to_string())
    }
}

// ============================================================================
// Wrapper types (newtype pattern for orphan rules)
// ============================================================================

/// WebSocket session wrapper that implements moq_transport::transport::Session.
#[derive(Clone)]
pub struct WsSession(pub web_transport_ws::Session);

impl WsSession {
    pub fn new(session: web_transport_ws::Session) -> Self {
        Self(session)
    }
}

/// WebSocket send stream wrapper.
pub struct WsSendStream(pub web_transport_ws::SendStream);

/// WebSocket receive stream wrapper.
pub struct WsRecvStream(pub web_transport_ws::RecvStream);

// ============================================================================
// WebSocket Session implementation
// ============================================================================

impl transport::Session for WsSession {
    type SendStream = WsSendStream;
    type RecvStream = WsRecvStream;
    type Error = WsError;

    async fn accept_bi(&mut self) -> Result<(Self::SendStream, Self::RecvStream), Self::Error> {
        wt::Session::accept_bi(&self.0)
            .await
            .map(|(s, r)| (WsSendStream(s), WsRecvStream(r)))
            .map_err(WsError::from)
    }

    async fn accept_uni(&mut self) -> Result<Self::RecvStream, Self::Error> {
        wt::Session::accept_uni(&self.0)
            .await
            .map(WsRecvStream)
            .map_err(WsError::from)
    }

    async fn open_bi(&mut self) -> Result<(Self::SendStream, Self::RecvStream), Self::Error> {
        wt::Session::open_bi(&self.0)
            .await
            .map(|(s, r)| (WsSendStream(s), WsRecvStream(r)))
            .map_err(WsError::from)
    }

    async fn open_uni(&mut self) -> Result<Self::SendStream, Self::Error> {
        wt::Session::open_uni(&self.0)
            .await
            .map(WsSendStream)
            .map_err(WsError::from)
    }

    async fn recv_datagram(&mut self) -> Result<Bytes, Self::Error> {
        // WebSocket doesn't support datagrams - block forever
        // MoQ will use streams instead
        std::future::pending().await
    }

    fn send_datagram(&mut self, _payload: Bytes) -> Result<(), Self::Error> {
        Err(WsError::DatagramsNotSupported)
    }

    fn close(self, code: u32, reason: &str) {
        wt::Session::close(&self.0, code, reason);
    }

    async fn closed(&self) -> Self::Error {
        WsError::from(wt::Session::closed(&self.0).await)
    }
}

// ============================================================================
// WebSocket SendStream implementation
// ============================================================================

impl transport::SendStream for WsSendStream {
    type Error = WsError;

    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        wt::SendStream::write(&mut self.0, buf)
            .await
            .map_err(WsError::from)
    }

    async fn write_chunk(&mut self, buf: Bytes) -> Result<(), Self::Error> {
        wt::SendStream::write_chunk(&mut self.0, buf)
            .await
            .map_err(WsError::from)
    }

    fn set_priority(&mut self, order: i32) {
        // web_transport_ws uses u8 for priority
        wt::SendStream::set_priority(&mut self.0, order.clamp(0, 255) as u8);
    }

    fn reset(mut self, code: u32) {
        wt::SendStream::reset(&mut self.0, code);
    }
}

// ============================================================================
// WebSocket RecvStream implementation
// ============================================================================

impl transport::RecvStream for WsRecvStream {
    type Error = WsError;

    async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        wt::RecvStream::read(&mut self.0, buf)
            .await
            .map_err(WsError::from)
    }

    async fn read_chunk(&mut self, max: usize) -> Result<Option<Bytes>, Self::Error> {
        wt::RecvStream::read_chunk(&mut self.0, max)
            .await
            .map_err(WsError::from)
    }

    fn stop(mut self, code: u32) {
        wt::RecvStream::stop(&mut self.0, code);
    }
}
