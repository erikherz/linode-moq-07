mod announce;
mod announced;
mod error;
mod publisher;
mod reader;
mod subscribe;
mod subscribed;
mod subscriber;
mod track_status_requested;
mod writer;

pub use announce::*;
pub use announced::*;
pub use error::*;
pub use publisher::*;
pub use subscribe::*;
pub use subscribed::*;
pub use subscriber::*;
pub use track_status_requested::*;

use reader::*;
use writer::*;

// Type aliases for backwards compatibility with web_transport::Session (QUIC)
pub type QuicSession = Session<web_transport::Session>;
pub type QuicPublisher = Publisher<web_transport::Session>;
pub type QuicSubscriber = Subscriber<web_transport::Session>;
pub type QuicSubscribed = Subscribed<web_transport::Session>;
pub type QuicAnnounced = Announced<web_transport::Session>;
pub type QuicAnnounce = Announce<web_transport::Session>;
pub type QuicSubscribe = Subscribe<web_transport::Session>;
pub type QuicTrackStatusRequested = TrackStatusRequested<web_transport::Session>;

use std::marker::PhantomData;

use futures::{stream::FuturesUnordered, StreamExt};

use crate::message::Message;
use crate::transport;
use crate::watch::Queue;
use crate::{message, setup};

#[must_use = "run() must be called"]
pub struct Session<T: transport::Session> {
    transport: T,

    sender: Writer<T::SendStream>,
    recver: Reader<T::RecvStream>,

    publisher: Option<Publisher<T>>,
    subscriber: Option<Subscriber<T>>,

    outgoing: Queue<Message>,
}

impl<T: transport::Session> Session<T> {
    fn new(
        transport: T,
        sender: Writer<T::SendStream>,
        recver: Reader<T::RecvStream>,
        role: setup::Role,
    ) -> (Self, Option<Publisher<T>>, Option<Subscriber<T>>) {
        let outgoing = Queue::default().split();
        let publisher = role
            .is_publisher()
            .then(|| Publisher::new(outgoing.0.clone(), transport.clone()));
        let subscriber = role
            .is_subscriber()
            .then(|| Subscriber::new(outgoing.0, PhantomData));

        let session = Self {
            transport,
            sender,
            recver,
            publisher: publisher.clone(),
            subscriber: subscriber.clone(),
            outgoing: outgoing.1,
        };

        (session, publisher, subscriber)
    }

    pub async fn connect(session: T) -> Result<(Session<T>, Publisher<T>, Subscriber<T>), SessionError> {
        Self::connect_role(session, setup::Role::Both).await.map(
            |(session, publisher, subscriber)| (session, publisher.unwrap(), subscriber.unwrap()),
        )
    }

    pub async fn connect_role(
        mut session: T,
        role: setup::Role,
    ) -> Result<(Session<T>, Option<Publisher<T>>, Option<Subscriber<T>>), SessionError> {
        let control = session
            .open_bi()
            .await
            .map_err(SessionError::transport)?;
        let mut sender = Writer::new(control.0);
        let mut recver = Reader::new(control.1);

        let versions: setup::Versions = [setup::Version::DRAFT_07].into();

        let client = setup::Client {
            role,
            versions: versions.clone(),
            params: Default::default(),
        };

        log::debug!("sending client SETUP: {:?}", client);
        sender.encode(&client).await?;

        let server: setup::Server = recver.decode().await?;
        log::debug!("received server SETUP: {:?}", server);

        // Downgrade our role based on the server's role.
        let role = match server.role {
            setup::Role::Both => role,
            setup::Role::Publisher => match role {
                // Both sides are publishers only
                setup::Role::Publisher => {
                    return Err(SessionError::RoleIncompatible(server.role, role))
                }
                _ => setup::Role::Subscriber,
            },
            setup::Role::Subscriber => match role {
                // Both sides are subscribers only
                setup::Role::Subscriber => {
                    return Err(SessionError::RoleIncompatible(server.role, role))
                }
                _ => setup::Role::Publisher,
            },
        };

        Ok(Session::new(session, sender, recver, role))
    }

    pub async fn accept(
        session: T,
    ) -> Result<(Session<T>, Option<Publisher<T>>, Option<Subscriber<T>>), SessionError> {
        Self::accept_role(session, setup::Role::Both).await
    }

    pub async fn accept_role(
        mut session: T,
        role: setup::Role,
    ) -> Result<(Session<T>, Option<Publisher<T>>, Option<Subscriber<T>>), SessionError> {
        let control = session
            .accept_bi()
            .await
            .map_err(SessionError::transport)?;
        let mut sender = Writer::new(control.0);
        let mut recver = Reader::new(control.1);

        let client: setup::Client = recver.decode().await?;
        log::debug!("received client SETUP: {:?}", client);

        if !client.versions.contains(&setup::Version::DRAFT_07) {
            return Err(SessionError::Version(
                client.versions,
                [setup::Version::DRAFT_07].into(),
            ));
        }

        // Downgrade our role based on the client's role.
        let role = match client.role {
            setup::Role::Both => role,
            setup::Role::Publisher => match role {
                // Both sides are publishers only
                setup::Role::Publisher => {
                    return Err(SessionError::RoleIncompatible(client.role, role))
                }
                _ => setup::Role::Subscriber,
            },
            setup::Role::Subscriber => match role {
                // Both sides are subscribers only
                setup::Role::Subscriber => {
                    return Err(SessionError::RoleIncompatible(client.role, role))
                }
                _ => setup::Role::Publisher,
            },
        };

        let server = setup::Server {
            role,
            version: setup::Version::DRAFT_07,
            params: Default::default(),
        };

        log::debug!("sending server SETUP: {:?}", server);
        sender.encode(&server).await?;

        Ok(Session::new(session, sender, recver, role))
    }

    pub async fn run(self) -> Result<(), SessionError> {
        tokio::select! {
            res = Self::run_recv(self.recver, self.publisher, self.subscriber.clone()) => res,
            res = Self::run_send(self.sender, self.outgoing) => res,
            res = Self::run_streams(self.transport.clone(), self.subscriber.clone()) => res,
            res = Self::run_datagrams(self.transport, self.subscriber) => res,
        }
    }

    async fn run_send(
        mut sender: Writer<T::SendStream>,
        mut outgoing: Queue<message::Message>,
    ) -> Result<(), SessionError> {
        while let Some(msg) = outgoing.pop().await {
            log::debug!("sending message: {:?}", msg);
            sender.encode(&msg).await?;
        }

        Ok(())
    }

    async fn run_recv(
        mut recver: Reader<T::RecvStream>,
        mut publisher: Option<Publisher<T>>,
        mut subscriber: Option<Subscriber<T>>,
    ) -> Result<(), SessionError> {
        loop {
            let msg: message::Message = recver.decode().await?;
            log::debug!("received message: {:?}", msg);

            let msg = match TryInto::<message::Publisher>::try_into(msg) {
                Ok(msg) => {
                    subscriber
                        .as_mut()
                        .ok_or(SessionError::RoleViolation)?
                        .recv_message(msg)?;
                    continue;
                }
                Err(msg) => msg,
            };

            let msg = match TryInto::<message::Subscriber>::try_into(msg) {
                Ok(msg) => {
                    publisher
                        .as_mut()
                        .ok_or(SessionError::RoleViolation)?
                        .recv_message(msg)?;
                    continue;
                }
                Err(msg) => msg,
            };

            // TODO GOAWAY
            unimplemented!("unknown message context: {:?}", msg)
        }
    }

    async fn run_streams(
        mut transport: T,
        subscriber: Option<Subscriber<T>>,
    ) -> Result<(), SessionError> {
        let mut tasks = FuturesUnordered::new();

        // Limit concurrent stream tasks to prevent stack overflow from unbounded spawning
        const MAX_CONCURRENT_STREAMS: usize = 256;
        // After this many consecutive errors, terminate the session
        const MAX_CONSECUTIVE_ERRORS: usize = 100;

        let mut consecutive_errors: usize = 0;

        loop {
            tokio::select! {
                // Only accept new streams if we're under the limit
                res = transport.accept_uni(), if tasks.len() < MAX_CONCURRENT_STREAMS => {
                    let stream = res.map_err(SessionError::transport)?;
                    let subscriber = subscriber.clone().ok_or(SessionError::RoleViolation)?;

                    tasks.push(async move {
                        match Subscriber::recv_stream(subscriber, stream).await {
                            Ok(()) => false, // success
                            Err(err) => {
                                log::warn!("failed to serve stream: {}", err);
                                true // error
                            }
                        }
                    });
                },
                Some(was_error) = tasks.next(), if !tasks.is_empty() => {
                    if was_error {
                        consecutive_errors += 1;
                        if consecutive_errors >= MAX_CONSECUTIVE_ERRORS {
                            log::warn!("too many consecutive stream errors ({}), closing session", consecutive_errors);
                            return Err(SessionError::Serve(crate::serve::ServeError::Done));
                        }
                    } else {
                        consecutive_errors = 0;
                    }
                },
            };
        }
    }

    async fn run_datagrams(mut transport: T, mut subscriber: Option<Subscriber<T>>) -> Result<(), SessionError> {
        loop {
            // For WebSocket, recv_datagram blocks forever (pending), which is fine.
            // The select! in run() will handle other branches.
            let datagram = match transport.recv_datagram().await {
                Ok(d) => d,
                Err(_) => {
                    // Datagrams not supported or connection closed
                    // For WebSocket this blocks forever via pending(), so this path
                    // only happens on actual transport errors.
                    return Ok(());
                }
            };
            subscriber
                .as_mut()
                .ok_or(SessionError::RoleViolation)?
                .recv_datagram(datagram)?;
        }
    }
}
