use std::ops;

use crate::coding::Encode;
use crate::serve::{ServeError, TrackReaderMode};
use crate::transport;
use crate::watch::State;
use crate::{data, message, serve};

use super::{Publisher, SessionError, SubscribeInfo, Writer};

#[derive(Debug)]
struct SubscribedState {
    max_group_id: Option<(u64, u64)>,
    closed: Result<(), ServeError>,
}

impl SubscribedState {
    fn update_max_group_id(&mut self, group_id: u64, object_id: u64) -> Result<(), ServeError> {
        if let Some((max_group, max_object)) = self.max_group_id {
            if group_id >= max_group && object_id >= max_object {
                self.max_group_id = Some((group_id, object_id));
            }
        }

        Ok(())
    }
}

impl Default for SubscribedState {
    fn default() -> Self {
        Self {
            max_group_id: None,
            closed: Ok(()),
        }
    }
}

pub struct Subscribed<T: transport::Session> {
    publisher: Publisher<T>,
    state: State<SubscribedState>,
    msg: message::Subscribe,
    ok: bool,

    pub info: SubscribeInfo,
}

impl<T: transport::Session> Subscribed<T> {
    pub(super) fn new(publisher: Publisher<T>, msg: message::Subscribe) -> (Self, SubscribedRecv) {
        let (send, recv) = State::default().split();
        let info = SubscribeInfo {
            namespace: msg.track_namespace.clone(),
            name: msg.track_name.clone(),
        };

        let send = Self {
            publisher,
            state: send,
            msg,
            info,
            ok: false,
        };

        // Prevents updates after being closed
        let recv = SubscribedRecv { state: recv };

        (send, recv)
    }

    pub async fn serve(mut self, track: serve::TrackReader) -> Result<(), SessionError> {
        let res = self.serve_inner(track).await;
        if let Err(err) = &res {
            self.close(err.clone().into())?;
        }

        res
    }

    async fn serve_inner(&mut self, track: serve::TrackReader) -> Result<(), SessionError> {
        let latest = track.latest();
        self.state
            .lock_mut()
            .ok_or(ServeError::Cancel)?
            .max_group_id = latest;

        self.publisher.send_message(message::SubscribeOk {
            id: self.msg.id,
            expires: None,
            group_order: message::GroupOrder::Descending, // TODO: resolve correct value from publisher / subscriber prefs
            latest,
        });

        self.ok = true; // So we sent SubscribeDone on drop

        match track.mode().await? {
            // TODO cancel track/datagrams on closed
            TrackReaderMode::Stream(stream) => self.serve_track(stream).await,
            TrackReaderMode::Subgroups(subgroups) => self.serve_subgroups(subgroups).await,
            TrackReaderMode::Datagrams(datagrams) => self.serve_datagrams(datagrams).await,
        }
    }

    pub fn close(self, err: ServeError) -> Result<(), ServeError> {
        let state = self.state.lock();
        state.closed.clone()?;

        let mut state = state.into_mut().ok_or(ServeError::Done)?;
        state.closed = Err(err);

        Ok(())
    }

    pub async fn closed(&self) -> Result<(), ServeError> {
        loop {
            {
                let state = self.state.lock();
                state.closed.clone()?;

                match state.modified() {
                    Some(notify) => notify,
                    None => return Ok(()),
                }
            }
            .await;
        }
    }
}

impl<T: transport::Session> ops::Deref for Subscribed<T> {
    type Target = SubscribeInfo;

    fn deref(&self) -> &Self::Target {
        &self.info
    }
}

impl<T: transport::Session> Drop for Subscribed<T> {
    fn drop(&mut self) {
        let state = self.state.lock();
        let err = state
            .closed
            .as_ref()
            .err()
            .cloned()
            .unwrap_or(ServeError::Done);
        let max_group_id = state.max_group_id;
        drop(state); // Important to avoid a deadlock

        if self.ok {
            self.publisher.send_message(message::SubscribeDone {
                id: self.msg.id,
                last: max_group_id,
                code: err.code(),
                reason: err.to_string(),
            });
        } else {
            self.publisher.send_message(message::SubscribeError {
                id: self.msg.id,
                alias: 0,
                code: err.code(),
                reason: err.to_string(),
            });
        };
    }
}

impl<T: transport::Session> Subscribed<T> {
    async fn serve_track(&mut self, mut track: serve::StreamReader) -> Result<(), SessionError> {
        let mut stream = self.publisher.open_uni().await?;

        // TODO figure out u32 vs u64 priority
        transport::SendStream::set_priority(&mut stream, track.priority as i32);

        let mut writer = Writer::new(stream);

        let header: data::Header = data::TrackHeader {
            subscribe_id: self.msg.id,
            track_alias: self.msg.track_alias,
            publisher_priority: track.priority,
        }
        .into();

        writer.encode(&header).await?;

        log::trace!("sent track header: {:?}", header);

        // Use explicit loops with boxed futures to reduce state machine depth
        loop {
            let next_group = Box::pin(track.next());
            match next_group.await? {
                Some(mut group) => {
                    loop {
                        let next_object = Box::pin(group.next());
                        match next_object.await? {
                            Some(mut object) => {
                                let header = data::TrackObject {
                                    group_id: object.group_id,
                                    object_id: object.object_id,
                                    size: object.size,
                                    status: object.status,
                                };

                                self.state
                                    .lock_mut()
                                    .ok_or(ServeError::Done)?
                                    .update_max_group_id(object.group_id, object.object_id)?;

                                writer.encode(&header).await?;

                                log::trace!("sent track object: {:?}", header);

                                loop {
                                    let read_chunk = Box::pin(object.read());
                                    match read_chunk.await? {
                                        Some(chunk) => {
                                            writer.write(&chunk).await?;
                                            log::trace!("sent track payload: {:?}", chunk.len());
                                        }
                                        None => break,
                                    }
                                }

                                log::trace!("sent track done");
                            }
                            None => break,
                        }
                    }
                }
                None => break,
            }
        }

        // Finish the stream gracefully to prevent RESET from being sent
        writer.finish()?;

        Ok(())
    }

    async fn serve_subgroups(
        &mut self,
        mut subgroups: serve::SubgroupsReader,
    ) -> Result<(), SessionError> {
        // Track consecutive failures to detect connection close
        let mut consecutive_failures: usize = 0;
        const MAX_CONSECUTIVE_FAILURES: usize = 3;

        loop {
            // Check failures early - connection likely closed
            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                log::warn!("too many consecutive subgroup failures, stopping");
                return Ok(());
            }

            // Box the futures to move state machines to heap, reducing stack depth during drop
            let next_fut = Box::pin(subgroups.next());
            let closed_fut = Box::pin(self.closed());

            tokio::select! {
                res = next_fut => match res {
                    Ok(Some(subgroup)) => {
                        let header = data::SubgroupHeader {
                            subscribe_id: self.msg.id,
                            track_alias: self.msg.track_alias,
                            group_id: subgroup.group_id,
                            subgroup_id: subgroup.subgroup_id,
                            publisher_priority: subgroup.priority,
                        };

                        let info = subgroup.info.clone();
                        let publisher = self.publisher.clone();
                        let state = self.state.clone();

                        tokio::spawn(async move {
                            if let Err(err) = Self::serve_subgroup(header, subgroup, publisher, state).await {
                                log::warn!("failed to serve subgroup: {:?}, error: {}", info, err);
                            }
                        });

                        // Reset failures since we successfully spawned
                        consecutive_failures = 0;
                    },
                    Ok(None) => return Ok(()),
                    Err(err) => {
                        consecutive_failures += 1;
                        if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                            log::warn!("too many consecutive subgroup failures, stopping");
                            return Ok(());
                        }
                        log::warn!("subgroup read error: {}", err);
                    }
                },
                res = closed_fut => return Ok(res?),
            }
        }
    }

    async fn serve_subgroup(
        header: data::SubgroupHeader,
        mut subgroup: serve::SubgroupReader,
        mut publisher: Publisher<T>,
        state: State<SubscribedState>,
    ) -> Result<(), SessionError> {
        let mut stream = publisher.open_uni().await?;

        // TODO figure out u32 vs u64 priority
        transport::SendStream::set_priority(&mut stream, subgroup.priority as i32);

        let mut writer = Writer::new(stream);

        let header: data::Header = header.into();
        writer.encode(&header).await?;

        log::trace!("sent group: {:?}", header);

        // Use explicit loop with boxed future to reduce state machine depth
        loop {
            let next_object = Box::pin(subgroup.next());
            match next_object.await? {
                Some(mut object) => {
                    let header = data::SubgroupObject {
                        object_id: object.object_id,
                        size: object.size,
                        status: object.status,
                    };

                    writer.encode(&header).await?;

                    state
                        .lock_mut()
                        .ok_or(ServeError::Done)?
                        .update_max_group_id(subgroup.group_id, object.object_id)?;

                    log::trace!("sent group object: {:?}", header);

                    // Inner loop with boxed futures
                    loop {
                        let read_chunk = Box::pin(object.read());
                        match read_chunk.await? {
                            Some(chunk) => {
                                writer.write(&chunk).await?;
                                log::trace!("sent group payload: {:?}", chunk.len());
                            }
                            None => break,
                        }
                    }

                    log::trace!("sent group done");
                }
                None => break,
            }
        }

        // Finish the stream gracefully to prevent RESET from being sent
        writer.finish()?;

        Ok(())
    }

    async fn serve_datagrams(
        &mut self,
        mut datagrams: serve::DatagramsReader,
    ) -> Result<(), SessionError> {
        // Track whether we've tried and failed to use datagrams
        let mut use_stream_fallback = false;

        // Use explicit loop with boxed future to reduce state machine depth
        loop {
            let read_fut = Box::pin(datagrams.read());
            let datagram = match read_fut.await? {
                Some(d) => d,
                None => break,
            };
            if !use_stream_fallback {
                // Try sending as datagram first
                let dg = data::Datagram {
                    subscribe_id: self.msg.id,
                    track_alias: self.msg.track_alias,
                    group_id: datagram.group_id,
                    object_id: datagram.object_id,
                    publisher_priority: datagram.priority,
                    object_status: datagram.status,
                    payload_len: datagram.payload.len() as u64,
                    payload: datagram.payload.clone(),
                };

                let mut buffer = bytes::BytesMut::with_capacity(dg.payload.len() + 100);
                dg.encode(&mut buffer)?;

                match self.publisher.send_datagram(buffer.into()) {
                    Ok(()) => {
                        log::trace!("sent datagram: {:?}", dg);
                        self.state
                            .lock_mut()
                            .ok_or(ServeError::Done)?
                            .update_max_group_id(datagram.group_id, datagram.object_id)?;
                        continue;
                    }
                    Err(_) => {
                        // Datagrams not supported (e.g., WebSocket transport)
                        // Fall back to sending as subgroup streams
                        log::info!("datagrams not supported, falling back to subgroup streams");
                        use_stream_fallback = true;
                    }
                }
            }

            // Fallback: send each datagram as a single-object subgroup stream
            let mut stream = self.publisher.open_uni().await?;
            transport::SendStream::set_priority(&mut stream, datagram.priority as i32);

            let mut writer = Writer::new(stream);

            // Write subgroup header
            let header: data::Header = data::SubgroupHeader {
                subscribe_id: self.msg.id,
                track_alias: self.msg.track_alias,
                group_id: datagram.group_id,
                subgroup_id: 0, // Each datagram becomes subgroup 0 of its group
                publisher_priority: datagram.priority,
            }
            .into();

            writer.encode(&header).await?;

            // Write the object
            let object = data::SubgroupObject {
                object_id: datagram.object_id,
                size: datagram.payload.len(),
                status: datagram.status,
            };

            writer.encode(&object).await?;
            writer.write(&datagram.payload).await?;

            // Finish the stream gracefully to prevent RESET from being sent
            writer.finish()?;

            log::trace!("sent datagram as subgroup stream: group={} object={}", datagram.group_id, datagram.object_id);

            self.state
                .lock_mut()
                .ok_or(ServeError::Done)?
                .update_max_group_id(datagram.group_id, datagram.object_id)?;
        }

        Ok(())
    }
}

pub(super) struct SubscribedRecv {
    state: State<SubscribedState>,
}

impl SubscribedRecv {
    pub fn recv_unsubscribe(&mut self) -> Result<(), ServeError> {
        let state = self.state.lock();
        state.closed.clone()?;

        if let Some(mut state) = state.into_mut() {
            state.closed = Err(ServeError::Cancel);
        }

        Ok(())
    }
}
