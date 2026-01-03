use futures::{stream::FuturesUnordered, FutureExt, StreamExt};
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
        let mut tasks = FuturesUnordered::new();
        tasks.push(self.session.run().boxed());

        if let Some(producer) = self.producer {
            tasks.push(producer.run().boxed());
        }

        if let Some(consumer) = self.consumer {
            tasks.push(consumer.run().boxed());
        }

        tasks.select_next_some().await
    }
}
