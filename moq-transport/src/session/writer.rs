use std::io;

use crate::coding::{Encode, EncodeError};
use crate::transport;

use super::SessionError;
use bytes::Buf;

pub struct Writer<S: transport::SendStream> {
    stream: S,
    buffer: bytes::BytesMut,
}

impl<S: transport::SendStream> Writer<S> {
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            buffer: Default::default(),
        }
    }

    pub async fn encode<T: Encode>(&mut self, msg: &T) -> Result<(), SessionError> {
        self.buffer.clear();
        msg.encode(&mut self.buffer)?;

        while !self.buffer.is_empty() {
            self.stream
                .write_buf(&mut self.buffer)
                .await
                .map_err(SessionError::transport)?;
        }

        Ok(())
    }

    pub async fn write(&mut self, buf: &[u8]) -> Result<(), SessionError> {
        let mut cursor = io::Cursor::new(buf);

        while cursor.has_remaining() {
            let size = self
                .stream
                .write_buf(&mut cursor)
                .await
                .map_err(SessionError::transport)?;
            if size == 0 {
                return Err(EncodeError::More(cursor.remaining()).into());
            }
        }

        Ok(())
    }

    /// Set the stream priority
    pub fn set_priority(&mut self, priority: i32) {
        self.stream.set_priority(priority);
    }

    /// Finish the stream gracefully (send FIN).
    /// This should be called when done writing to prevent RESET from being sent.
    pub fn finish(&mut self) -> Result<(), SessionError> {
        self.stream.finish().map_err(SessionError::transport)
    }
}
