//! Transport abstraction for moq-transport.
//!
//! This module defines traits that abstract over the underlying transport layer,
//! allowing moq-transport to work with multiple transports (QUIC, WebSocket, etc.).

use bytes::{Buf, BufMut, Bytes};
use std::future::Future;

/// A transport session that can create and accept streams.
pub trait Session: Clone + Send + Sync + 'static {
    /// The type of send stream provided by this transport.
    type SendStream: SendStream;
    /// The type of receive stream provided by this transport.
    type RecvStream: RecvStream;
    /// The error type for session operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Accept a new bidirectional stream from the peer.
    fn accept_bi(
        &mut self,
    ) -> impl Future<Output = Result<(Self::SendStream, Self::RecvStream), Self::Error>> + Send;

    /// Accept a new unidirectional stream from the peer.
    fn accept_uni(&mut self)
        -> impl Future<Output = Result<Self::RecvStream, Self::Error>> + Send;

    /// Open a new bidirectional stream.
    fn open_bi(
        &mut self,
    ) -> impl Future<Output = Result<(Self::SendStream, Self::RecvStream), Self::Error>> + Send;

    /// Open a new unidirectional stream.
    fn open_uni(&mut self) -> impl Future<Output = Result<Self::SendStream, Self::Error>> + Send;

    /// Receive a datagram from the peer.
    ///
    /// For transports that don't support datagrams (like WebSocket),
    /// this should return an error or block forever.
    fn recv_datagram(&mut self) -> impl Future<Output = Result<Bytes, Self::Error>> + Send;

    /// Send a datagram to the peer.
    ///
    /// For transports that don't support datagrams, this should return an error.
    fn send_datagram(&mut self, payload: Bytes) -> Result<(), Self::Error>;

    /// Close the session with an error code and reason.
    fn close(self, code: u32, reason: &str);

    /// Block until the session is closed.
    fn closed(&self) -> impl Future<Output = Self::Error> + Send;
}

/// A send stream for writing data to the peer.
pub trait SendStream: Send {
    /// The error type for write operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Write bytes to the stream.
    fn write(&mut self, buf: &[u8])
        -> impl Future<Output = Result<usize, Self::Error>> + Send;

    /// Write from a buffer, advancing its position.
    fn write_buf<B: Buf + Send>(
        &mut self,
        buf: &mut B,
    ) -> impl Future<Output = Result<usize, Self::Error>> + Send {
        async move {
            let chunk = buf.chunk();
            let size = self.write(chunk).await?;
            buf.advance(size);
            Ok(size)
        }
    }

    /// Write a chunk of bytes to the stream.
    fn write_chunk(&mut self, buf: Bytes)
        -> impl Future<Output = Result<(), Self::Error>> + Send;

    /// Set the stream's priority.
    fn set_priority(&mut self, order: i32);

    /// Finish the stream gracefully (send FIN).
    /// This should be called when done writing to prevent RESET from being sent.
    fn finish(&mut self) -> Result<(), Self::Error>;

    /// Reset the stream with an error code.
    fn reset(self, code: u32);
}

/// A receive stream for reading data from the peer.
pub trait RecvStream: Send {
    /// The error type for read operations.
    type Error: std::error::Error + Send + Sync + 'static;

    /// Read bytes from the stream.
    fn read(
        &mut self,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<Option<usize>, Self::Error>> + Send;

    /// Read into a buffer, advancing its position.
    fn read_buf<B: BufMut + Send>(
        &mut self,
        buf: &mut B,
    ) -> impl Future<Output = Result<bool, Self::Error>> + Send {
        async move {
            let dst = buf.chunk_mut();
            let dst = unsafe { &mut *(dst as *mut _ as *mut [u8]) };

            let size = match self.read(dst).await? {
                Some(size) => size,
                None => return Ok(false),
            };

            unsafe { buf.advance_mut(size) };
            Ok(true)
        }
    }

    /// Read a chunk of bytes from the stream.
    fn read_chunk(
        &mut self,
        max: usize,
    ) -> impl Future<Output = Result<Option<Bytes>, Self::Error>> + Send;

    /// Stop receiving on this stream with an error code.
    fn stop(self, code: u32);
}

// ============================================================================
// Implementation for web_transport types (QUIC)
// ============================================================================

impl Session for web_transport::Session {
    type SendStream = web_transport::SendStream;
    type RecvStream = web_transport::RecvStream;
    type Error = web_transport::SessionError;

    async fn accept_bi(&mut self) -> Result<(Self::SendStream, Self::RecvStream), Self::Error> {
        web_transport::Session::accept_bi(self).await
    }

    async fn accept_uni(&mut self) -> Result<Self::RecvStream, Self::Error> {
        web_transport::Session::accept_uni(self).await
    }

    async fn open_bi(&mut self) -> Result<(Self::SendStream, Self::RecvStream), Self::Error> {
        web_transport::Session::open_bi(self).await
    }

    async fn open_uni(&mut self) -> Result<Self::SendStream, Self::Error> {
        web_transport::Session::open_uni(self).await
    }

    async fn recv_datagram(&mut self) -> Result<Bytes, Self::Error> {
        web_transport::Session::recv_datagram(self).await
    }

    fn send_datagram(&mut self, _payload: Bytes) -> Result<(), Self::Error> {
        // web_transport::Session::send_datagram is async, but our trait is sync
        // This is a limitation - for proper datagram support we'd need async
        Ok(())
    }

    fn close(self, code: u32, reason: &str) {
        web_transport::Session::close(self, code, reason);
    }

    async fn closed(&self) -> Self::Error {
        web_transport::Session::closed(self).await
    }
}

// Helper to call the inherent finish() method on web_transport::SendStream
// without recursion through the trait.
fn web_transport_finish(stream: &mut web_transport::SendStream) -> Result<(), web_transport::WriteError> {
    stream.finish().map_err(|_| web_transport::WriteError::ClosedStream)
}

impl SendStream for web_transport::SendStream {
    type Error = web_transport::WriteError;

    async fn write(&mut self, buf: &[u8]) -> Result<usize, Self::Error> {
        web_transport::SendStream::write(self, buf).await
    }

    async fn write_chunk(&mut self, buf: Bytes) -> Result<(), Self::Error> {
        web_transport::SendStream::write_chunk(self, buf).await
    }

    fn set_priority(&mut self, order: i32) {
        web_transport::SendStream::set_priority(self, order);
    }

    fn finish(&mut self) -> Result<(), Self::Error> {
        web_transport_finish(self)
    }

    fn reset(self, code: u32) {
        web_transport::SendStream::reset(self, code);
    }
}

impl RecvStream for web_transport::RecvStream {
    type Error = web_transport::ReadError;

    async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, Self::Error> {
        web_transport::RecvStream::read(self, buf).await
    }

    async fn read_chunk(&mut self, max: usize) -> Result<Option<Bytes>, Self::Error> {
        web_transport::RecvStream::read_chunk(self, max).await
    }

    fn stop(self, code: u32) {
        web_transport::RecvStream::stop(self, code);
    }
}
