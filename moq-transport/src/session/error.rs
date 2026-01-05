use crate::{coding, serve, setup};

#[derive(thiserror::Error, Debug, Clone)]
pub enum SessionError {
    #[error("session closed")]
    Closed,

    #[error("transport error: {0}")]
    Transport(String),

    #[error("encode error: {0}")]
    Encode(#[from] coding::EncodeError),

    #[error("decode error: {0}")]
    Decode(#[from] coding::DecodeError),

    // TODO move to a ConnectError
    #[error("unsupported versions: client={0:?} server={1:?}")]
    Version(setup::Versions, setup::Versions),

    // TODO move to a ConnectError
    #[error("incompatible roles: client={0:?} server={1:?}")]
    RoleIncompatible(setup::Role, setup::Role),

    /// The role negiotiated in the handshake was violated. For example, a publisher sent a SUBSCRIBE, or a subscriber sent an OBJECT.
    #[error("role violation")]
    RoleViolation,

    /// Some VarInt was too large and we were too lazy to handle it
    #[error("varint bounds exceeded")]
    BoundsExceeded(#[from] coding::BoundsExceeded),

    /// A duplicate ID was used
    #[error("duplicate")]
    Duplicate,

    #[error("internal error")]
    Internal,

    #[error("serve error: {0}")]
    Serve(#[from] serve::ServeError),

    #[error("wrong size")]
    WrongSize,

    #[error("datagrams not supported")]
    DatagramsNotSupported,
}

impl SessionError {
    /// Create a transport error from any error type
    pub fn transport<E: std::error::Error>(err: E) -> Self {
        SessionError::Transport(err.to_string())
    }
}

// Keep backwards compatibility with web_transport error types
impl From<web_transport::SessionError> for SessionError {
    fn from(err: web_transport::SessionError) -> Self {
        SessionError::Transport(err.to_string())
    }
}

impl From<web_transport::WriteError> for SessionError {
    fn from(err: web_transport::WriteError) -> Self {
        SessionError::Transport(err.to_string())
    }
}

impl From<web_transport::ReadError> for SessionError {
    fn from(err: web_transport::ReadError) -> Self {
        SessionError::Transport(err.to_string())
    }
}

impl SessionError {
    /// An integer code that is sent over the wire.
    pub fn code(&self) -> u64 {
        match self {
            Self::Closed => 499,
            Self::RoleIncompatible(..) => 406,
            Self::RoleViolation => 405,
            Self::Transport(_) => 503,
            Self::Version(..) => 406,
            Self::Decode(_) => 400,
            Self::Encode(_) => 500,
            Self::BoundsExceeded(_) => 500,
            Self::Duplicate => 409,
            Self::Internal => 500,
            Self::WrongSize => 400,
            Self::DatagramsNotSupported => 501,
            Self::Serve(err) => err.code(),
        }
    }
}

impl From<SessionError> for serve::ServeError {
    fn from(err: SessionError) -> Self {
        match err {
            SessionError::Serve(err) => err,
            _ => serve::ServeError::Internal(err.to_string()),
        }
    }
}
