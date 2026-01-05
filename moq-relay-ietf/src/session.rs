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

        // Use Box::pin to heap-allocate futures and reduce stack usage
        let session_fut = Box::pin(session.run());
        let producer_fut = Box::pin(async {
            match producer {
                Some(p) => p.run().await,
                None => std::future::pending().await,
            }
        });
        let consumer_fut = Box::pin(async {
            match consumer {
                Some(c) => c.run().await,
                None => std::future::pending().await,
            }
        });

        let result = tokio::select! {
            res = session_fut => res,
            res = producer_fut => res,
            res = consumer_fut => res,
        };

        // Yield to allow cleanup to happen incrementally
        tokio::task::yield_now().await;

        result
    }
}
