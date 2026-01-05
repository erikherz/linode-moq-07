use moq_transport::{
    serve::Tracks,
    session::{Announced, SessionError, Subscriber},
    transport,
};
use web_transport;

use crate::{Api, Locals, Producer};

// Type alias for QUIC Producer (used for forwarding to other relays)
type QuicProducer = Producer<web_transport::Session>;

#[derive(Clone)]
pub struct Consumer<T: transport::Session> {
    remote: Subscriber<T>,
    locals: Locals,
    api: Option<Api>,
    forward: Option<QuicProducer>, // Forward all announcements to this subscriber (always over QUIC)
}

impl<T: transport::Session> Consumer<T> {
    pub fn new(
        remote: Subscriber<T>,
        locals: Locals,
        api: Option<Api>,
        forward: Option<QuicProducer>,
    ) -> Self {
        Self {
            remote,
            locals,
            api,
            forward,
        }
    }

    pub async fn run(mut self) -> Result<(), SessionError> {
        // Use tokio::spawn instead of FuturesUnordered to avoid deep drop chains
        loop {
            match self.remote.announced().await {
                Some(announce) => {
                    let this = self.clone();
                    tokio::spawn(async move {
                        let info = announce.clone();
                        log::info!("serving announce: {:?}", info);

                        if let Err(err) = this.serve(announce).await {
                            log::warn!("failed serving announce: {:?}, error: {}", info, err)
                        }
                    });
                }
                None => return Ok(()),
            }
        }
    }

    async fn serve(mut self, mut announce: Announced<T>) -> Result<(), anyhow::Error> {
        let (_, mut request, reader) = Tracks::new(announce.namespace.clone()).produce();

        if let Some(api) = self.api.as_ref() {
            let mut refresh = api.set_origin(reader.namespace.to_utf8_path()).await?;
            tokio::spawn(async move {
                if let Err(err) = refresh.run().await {
                    log::warn!("failed refreshing origin: {}", err);
                }
            });
        }

        // Register the local tracks, unregister on drop
        let _register = self.locals.register(reader.clone()).await?;

        announce.ok()?;

        if let Some(mut forward) = self.forward.clone() {
            let reader_clone = reader.clone();
            tokio::spawn(async move {
                log::info!("forwarding announce: {:?}", reader_clone.info);
                if let Err(err) = forward.announce(reader_clone).await {
                    log::warn!("failed forwarding announce: {}", err);
                }
            });
        }

        loop {
            tokio::select! {
                // If the announce is closed, return the error
                Err(err) = announce.closed() => return Err(err.into()),

                // Wait for the next subscriber and serve the track.
                Some(track) = request.next() => {
                    let mut remote = self.remote.clone();
                    tokio::spawn(async move {
                        let info = track.clone();
                        log::info!("forwarding subscribe: {:?}", info);

                        if let Err(err) = remote.subscribe(track).await {
                            log::warn!("failed forwarding subscribe: {:?}, error: {}", info, err)
                        }
                    });
                },
                else => return Ok(()),
            }
        }
    }
}
