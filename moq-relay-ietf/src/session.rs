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
        // Use tokio::select! instead of FuturesUnordered to avoid potential
        // stack overflow when dropping complex futures
        tokio::select! {
            res = self.session.run() => res,
            res = async {
                match self.producer {
                    Some(p) => p.run().await,
                    None => std::future::pending().await,
                }
            } => res,
            res = async {
                match self.consumer {
                    Some(c) => c.run().await,
                    None => std::future::pending().await,
                }
            } => res,
        }
    }
}
