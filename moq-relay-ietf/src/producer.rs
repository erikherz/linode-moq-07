use futures::{stream::FuturesUnordered, StreamExt};
use moq_transport::{
    serve::{ServeError, TracksReader},
    session::{Publisher, SessionError, Subscribed},
    transport,
};

use moq_transport::session::Subscriber;

use crate::{Locals, RemotesConsumer};

// Type alias for upstream subscriber (always QUIC to Cloudflare)
type UpstreamSubscriber = Subscriber<web_transport::Session>;

#[derive(Clone)]
pub struct Producer<T: transport::Session> {
    remote: Publisher<T>,
    locals: Locals,
    remotes: Option<RemotesConsumer>,
    /// Optional upstream subscriber (e.g., Cloudflare) to query for unknown streams
    upstream: Option<UpstreamSubscriber>,
}

impl<T: transport::Session> Producer<T> {
    pub fn new(
        remote: Publisher<T>,
        locals: Locals,
        remotes: Option<RemotesConsumer>,
        upstream: Option<UpstreamSubscriber>,
    ) -> Self {
        Self {
            remote,
            locals,
            remotes,
            upstream,
        }
    }

    pub async fn announce(&mut self, tracks: TracksReader) -> Result<(), SessionError> {
        self.remote.announce(tracks).await
    }

    pub async fn run(mut self) -> Result<(), SessionError> {
        let mut tasks = FuturesUnordered::new();

        loop {
            tokio::select! {
                Some(subscribe) = self.remote.subscribed() => {
                    let this = self.clone();

                    tasks.push(async move {
                        let info = subscribe.clone();
                        log::info!("serving subscribe: {:?}", info);

                        if let Err(err) = this.serve(subscribe).await {
                            log::warn!("failed serving subscribe: {:?}, error: {}", info, err)
                        }
                    })
                },
                _= tasks.next(), if !tasks.is_empty() => {},
                else => return Ok(()),
            };
        }
    }

    async fn serve(self, subscribe: Subscribed<T>) -> Result<(), anyhow::Error> {
        // Try local announces first
        if let Some(mut local) = self.locals.route(&subscribe.namespace) {
            if let Some(track) = local.subscribe(&subscribe.name) {
                log::info!("serving from local: {:?}", track.info);
                return Ok(subscribe.serve(track).await?);
            }
        }

        // Try known remotes (via API)
        if let Some(remotes) = &self.remotes {
            if let Some(remote) = remotes.route(&subscribe.namespace).await? {
                if let Some(track) =
                    remote.subscribe(subscribe.namespace.clone(), subscribe.name.clone())?
                {
                    log::info!("serving from remote: {:?} {:?}", remote.info, track.info);

                    // NOTE: Depends on drop(track) being called afterwards
                    return Ok(subscribe.serve(track.reader).await?);
                }
            }
        }

        // Try upstream (e.g., Cloudflare) for streams we don't know about
        if let Some(mut upstream) = self.upstream.clone() {
            log::info!(
                "trying upstream for unknown stream: {:?} {:?}",
                subscribe.namespace,
                subscribe.name
            );

            // Create a Track to receive data from upstream
            let track = moq_transport::serve::Track::new(
                subscribe.namespace.clone(),
                subscribe.name.clone(),
            );
            let (writer, reader) = track.produce();

            // Subscribe to the upstream - this will populate the writer with data
            match upstream.subscribe(writer).await {
                Ok(()) => {
                    log::info!("serving from upstream: {:?}", subscribe.name);
                    return Ok(subscribe.serve(reader).await?);
                }
                Err(err) => {
                    log::warn!("upstream subscribe failed: {:?}", err);
                }
            }
        }

        Err(ServeError::NotFound.into())
    }
}
