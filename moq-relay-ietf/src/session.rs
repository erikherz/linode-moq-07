use moq_transport::session::SessionError;
use moq_transport::transport;
use tokio_util::sync::CancellationToken;

use crate::{Consumer, Producer};

pub struct Session<T: transport::Session> {
    pub session: moq_transport::session::Session<T>,
    pub producer: Option<Producer<T>>,
    pub consumer: Option<Consumer<T>>,
}

impl<T: transport::Session> Session<T> {
    pub async fn run(self) -> Result<(), SessionError> {
        let Session { session, producer, consumer } = self;

        // Use CancellationToken to signal all tasks to stop
        let cancel = CancellationToken::new();

        // Spawn each component as a separate task so their cleanup happens on independent stacks
        // This is critical for WebSocket sessions which can overflow the stack during drop
        let cancel_session = cancel.clone();
        let session_handle = tokio::spawn(async move {
            tokio::select! {
                biased;
                _ = cancel_session.cancelled() => Err(SessionError::Closed),
                res = session.run() => res,
            }
        });

        let cancel_producer = cancel.clone();
        let producer_handle = tokio::spawn(async move {
            match producer {
                Some(p) => {
                    tokio::select! {
                        biased;
                        _ = cancel_producer.cancelled() => Err(SessionError::Closed),
                        res = p.run() => res,
                    }
                }
                None => {
                    cancel_producer.cancelled().await;
                    Err(SessionError::Closed)
                }
            }
        });

        let cancel_consumer = cancel.clone();
        let consumer_handle = tokio::spawn(async move {
            match consumer {
                Some(c) => {
                    tokio::select! {
                        biased;
                        _ = cancel_consumer.cancelled() => Err(SessionError::Closed),
                        res = c.run() => res,
                    }
                }
                None => {
                    cancel_consumer.cancelled().await;
                    Err(SessionError::Closed)
                }
            }
        });

        // Wait for any component to complete
        let result = tokio::select! {
            res = session_handle => res.unwrap_or(Err(SessionError::Closed)),
            res = producer_handle => res.unwrap_or(Err(SessionError::Closed)),
            res = consumer_handle => res.unwrap_or(Err(SessionError::Closed)),
        };

        // Signal all tasks to stop - they will cleanup on their own stacks
        cancel.cancel();

        // Give spawned tasks time to notice cancellation and cleanup
        tokio::task::yield_now().await;

        result
    }
}
