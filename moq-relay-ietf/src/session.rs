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

        // Use AbortHandle to abort tasks without needing to own JoinHandle
        let session_abort = session_handle.abort_handle();
        let producer_abort = producer_handle.abort_handle();
        let consumer_abort = consumer_handle.abort_handle();

        // Wait for any component to complete
        let result = tokio::select! {
            res = session_handle => res.unwrap_or(Err(SessionError::Closed)),
            res = producer_handle => res.unwrap_or(Err(SessionError::Closed)),
            res = consumer_handle => res.unwrap_or(Err(SessionError::Closed)),
        };

        // Abort remaining tasks - their cleanup happens on their own stacks when aborted
        // This is the key to preventing stack overflow: abort() drops the future on the task's stack
        session_abort.abort();
        producer_abort.abort();
        consumer_abort.abort();

        result
    }
}
