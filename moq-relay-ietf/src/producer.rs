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
        // Use tokio::spawn instead of FuturesUnordered to avoid deep drop chains
        loop {
            match self.remote.subscribed().await {
                Some(subscribe) => {
                    let this = self.clone();
                    tokio::spawn(async move {
                        let info = subscribe.clone();
                        log::info!("serving subscribe: {:?}", info);

                        if let Err(err) = this.serve(subscribe).await {
                            log::warn!("failed serving subscribe: {:?}, error: {}", info, err)
                        }
                    });
                }
                None => return Ok(()),
            }
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

            // Spawn the upstream subscribe in background - it runs until the track ends
            // The writer will receive data from upstream, reader will provide it to our subscriber
            let track_name = subscribe.name.clone();
            tokio::spawn(async move {
                if let Err(err) = upstream.subscribe(writer).await {
                    log::warn!("upstream subscribe ended: {:?} - {:?}", track_name, err);
                }
            });

            log::info!("serving from upstream: {:?}", subscribe.name);
            return Ok(subscribe.serve(reader).await?);
        }

        Err(ServeError::NotFound.into())
    }
}
