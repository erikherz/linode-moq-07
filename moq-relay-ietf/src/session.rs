use moq_transport::session::SessionError;
use moq_transport::transport;

use crate::{Consumer, Producer};

pub struct Session<T: transport::Session> {
    pub session: moq_transport::session::Session<T>,
    pub producer: Option<Producer<T>>,
    pub consumer: Option<Consumer<T>>,
}

impl<T: transport::Session> Session<T> {
    pub async fn run(self) -> Result<(), SessionError> {
        let Session { session, producer, consumer } = self;

        // Spawn each component as a separate task so their cleanup happens on independent stacks
        // This is critical for WebSocket sessions which can overflow the stack during drop
        let session_handle = tokio::spawn(async move {
            session.run().await
        });

        let producer_handle = tokio::spawn(async move {
            match producer {
                Some(p) => p.run().await,
                None => std::future::pending().await,
            }
        });

        let consumer_handle = tokio::spawn(async move {
            match consumer {
                Some(c) => c.run().await,
                None => std::future::pending().await,
            }
        });

        // Wait for any component to complete
        // Don't abort the other tasks - let them fail naturally when connection closes
        // Aborting can cause stack overflow during the future drop
        let result = tokio::select! {
            res = session_handle => res.unwrap_or(Err(SessionError::Closed)),
            res = producer_handle => res.unwrap_or(Err(SessionError::Closed)),
            res = consumer_handle => res.unwrap_or(Err(SessionError::Closed)),
        };

        // Let remaining tasks run to completion or fail naturally
        // They will detect the closed connection and clean up on their own time

        result
    }
}
