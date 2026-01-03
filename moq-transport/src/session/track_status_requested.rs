use super::{Publisher, SessionError};
use crate::coding::Tuple;
use crate::message;
use crate::transport;

#[derive(Debug, Clone)]
pub struct TrackStatusRequestedInfo {
    pub namespace: Tuple,
    pub track: String,
}

pub struct TrackStatusRequested<T: transport::Session> {
    publisher: Publisher<T>,
    // msg: message::TrackStatusRequest, // TODO: See if we actually need this
    pub info: TrackStatusRequestedInfo,
}

impl<T: transport::Session> TrackStatusRequested<T> {
    pub fn new(publisher: Publisher<T>, msg: message::TrackStatusRequest) -> Self {
        let namespace = msg.track_namespace.clone();
        let track = msg.track_name.clone();
        Self {
            publisher,
            info: TrackStatusRequestedInfo { namespace, track },
        }
    }

    pub async fn respond(&mut self, status: message::TrackStatus) -> Result<(), SessionError> {
        self.publisher.send_message(status);
        Ok(())
    }
}
