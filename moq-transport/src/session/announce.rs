use std::{collections::VecDeque, ops};

use crate::coding::Tuple;
use crate::transport;
use crate::watch::State;
use crate::{message, serve::ServeError};

use super::{Publisher, Subscribed, TrackStatusRequested};

#[derive(Debug, Clone)]
pub struct AnnounceInfo {
    pub namespace: Tuple,
}

struct AnnounceState<T: transport::Session> {
    subscribers: VecDeque<Subscribed<T>>,
    track_statuses_requested: VecDeque<TrackStatusRequested<T>>,
    ok: bool,
    closed: Result<(), ServeError>,
}

impl<T: transport::Session> Default for AnnounceState<T> {
    fn default() -> Self {
        Self {
            subscribers: Default::default(),
            track_statuses_requested: Default::default(),
            ok: false,
            closed: Ok(()),
        }
    }
}

impl<T: transport::Session> Drop for AnnounceState<T> {
    fn drop(&mut self) {
        for subscriber in self.subscribers.drain(..) {
            subscriber.close(ServeError::NotFound).ok();
        }
    }
}

#[must_use = "unannounce on drop"]
pub struct Announce<T: transport::Session> {
    publisher: Publisher<T>,
    state: State<AnnounceState<T>>,

    pub info: AnnounceInfo,
}

impl<T: transport::Session> Announce<T> {
    pub(super) fn new(mut publisher: Publisher<T>, namespace: Tuple) -> (Announce<T>, AnnounceRecv<T>) {
        let info = AnnounceInfo {
            namespace: namespace.clone(),
        };

        publisher.send_message(message::Announce {
            namespace,
            params: Default::default(),
        });

        let (send, recv) = State::default().split();

        let send = Self {
            publisher,
            info,
            state: send,
        };
        let recv = AnnounceRecv { state: recv };

        (send, recv)
    }

    // Run until we get an error
    pub async fn closed(&self) -> Result<(), ServeError> {
        loop {
            {
                let state = self.state.lock();
                state.closed.clone()?;

                match state.modified() {
                    Some(notified) => notified,
                    None => return Ok(()),
                }
            }
            .await;
        }
    }

    pub async fn subscribed(&self) -> Result<Option<Subscribed<T>>, ServeError> {
        loop {
            {
                let state = self.state.lock();
                if !state.subscribers.is_empty() {
                    return Ok(state
                        .into_mut()
                        .and_then(|mut state| state.subscribers.pop_front()));
                }

                state.closed.clone()?;
                match state.modified() {
                    Some(notified) => notified,
                    None => return Ok(None),
                }
            }
            .await;
        }
    }

    pub async fn track_status_requested(&self) -> Result<Option<TrackStatusRequested<T>>, ServeError> {
        loop {
            {
                let state = self.state.lock();
                if !state.track_statuses_requested.is_empty() {
                    return Ok(state
                        .into_mut()
                        .and_then(|mut state| state.track_statuses_requested.pop_front()));
                }

                state.closed.clone()?;
                match state.modified() {
                    Some(notified) => notified,
                    None => return Ok(None),
                }
            }
            .await;
        }
    }

    // Wait until an OK is received
    pub async fn ok(&self) -> Result<(), ServeError> {
        loop {
            {
                let state = self.state.lock();
                if state.ok {
                    return Ok(());
                }
                state.closed.clone()?;

                match state.modified() {
                    Some(notified) => notified,
                    None => return Ok(()),
                }
            }
            .await;
        }
    }
}

impl<T: transport::Session> Drop for Announce<T> {
    fn drop(&mut self) {
        if self.state.lock().closed.is_err() {
            return;
        }

        self.publisher.send_message(message::Unannounce {
            namespace: self.namespace.clone(),
        });
    }
}

impl<T: transport::Session> ops::Deref for Announce<T> {
    type Target = AnnounceInfo;

    fn deref(&self) -> &Self::Target {
        &self.info
    }
}

pub(super) struct AnnounceRecv<T: transport::Session> {
    state: State<AnnounceState<T>>,
}

impl<T: transport::Session> AnnounceRecv<T> {
    pub fn recv_ok(&mut self) -> Result<(), ServeError> {
        if let Some(mut state) = self.state.lock_mut() {
            if state.ok {
                return Err(ServeError::Duplicate);
            }

            state.ok = true;
        }

        Ok(())
    }

    pub fn recv_error(self, err: ServeError) -> Result<(), ServeError> {
        let state = self.state.lock();
        state.closed.clone()?;

        let mut state = state.into_mut().ok_or(ServeError::Done)?;
        state.closed = Err(err);

        Ok(())
    }

    pub fn recv_subscribe(&mut self, subscriber: Subscribed<T>) -> Result<(), ServeError> {
        let mut state = self.state.lock_mut().ok_or(ServeError::Done)?;
        state.subscribers.push_back(subscriber);

        Ok(())
    }

    pub fn recv_track_status_requested(
        &mut self,
        track_status_requested: TrackStatusRequested<T>,
    ) -> Result<(), ServeError> {
        let mut state = self.state.lock_mut().ok_or(ServeError::Done)?;
        state
            .track_statuses_requested
            .push_back(track_status_requested);
        Ok(())
    }
}
