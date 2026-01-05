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
        // Guard against deep recursion - just skip message sending if stack is getting deep
        thread_local! {
            static DROP_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
        }

        let depth = DROP_DEPTH.with(|d| {
            let current = d.get();
            d.set(current + 1);
            current
        });

        // If we're more than 10 levels deep, skip the message to prevent stack overflow
        if depth > 10 {
            log::warn!("Subscribed::drop recursion depth {} - skipping message", depth);
            DROP_DEPTH.with(|d| d.set(depth));
            return;
        }

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

        DROP_DEPTH.with(|d| d.set(depth));
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

        while let Some(mut group) = track.next().await? {
            while let Some(mut object) = group.next().await? {
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

                while let Some(chunk) = object.read().await? {
                    writer.write(&chunk).await?;
                    log::trace!("sent track payload: {:?}", chunk.len());
                }

                log::trace!("sent track done");
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

        // Track spawned tasks so we can abort them on exit
        let mut task_handles: Vec<tokio::task::AbortHandle> = Vec::new();

        let result = loop {
            // Check failures early - connection likely closed
            if consecutive_failures >= MAX_CONSECUTIVE_FAILURES {
                log::warn!("too many consecutive subgroup failures, stopping");
                break Ok(());
            }

            tokio::select! {
                res = subgroups.next() => match res {
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

                        // Spawn each subgroup on its own task so cleanup happens on that task's stack
                        let handle = tokio::spawn(async move {
                            Self::serve_subgroup(header, subgroup, publisher, state).await
                        });

                        let abort_handle = handle.abort_handle();
                        task_handles.push(abort_handle);

                        // Check result without blocking - if the connection is dead, we'll find out soon
                        // Use a small timeout to detect failures quickly
                        match tokio::time::timeout(std::time::Duration::from_millis(10), handle).await {
                            Ok(Ok(Ok(()))) => {
                                consecutive_failures = 0;
                            }
                            Ok(Ok(Err(err))) => {
                                consecutive_failures += 1;
                                log::warn!("failed to serve group: {:?}, error: {}", info, err);
                            }
                            Ok(Err(_join_err)) => {
                                consecutive_failures += 1;
                                log::warn!("failed to serve group: {:?}, error: task panicked", info);
                            }
                            Err(_timeout) => {
                                // Task is still running, that's fine
                                consecutive_failures = 0;
                            }
                        }
                    },
                    Ok(None) => break Ok(()),
                    Err(err) => break Err(err.into()),
                },
                res = self.closed() => break Ok(res?),
            }
        };

        // Abort all spawned tasks - their cleanup happens on their own stacks
        for handle in task_handles {
            handle.abort();
        }

        result
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

        while let Some(mut object) = subgroup.next().await? {
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

            while let Some(chunk) = object.read().await? {
                writer.write(&chunk).await?;
                log::trace!("sent group payload: {:?}", chunk.len());
            }

            log::trace!("sent group done");
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

        while let Some(datagram) = datagrams.read().await? {
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
