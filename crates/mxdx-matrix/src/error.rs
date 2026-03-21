use std::fmt;

#[derive(Debug)]
pub enum MatrixClientError {
    Sdk(matrix_sdk::Error),
    Http(matrix_sdk::HttpError),
    Registration(String),
    RoomNotFound(String),
    KeyExchangeTimeout(String),
    RoomCreationTimeout(String),
    Other(anyhow::Error),
}

impl fmt::Display for MatrixClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Sdk(e) => write!(f, "Matrix SDK error: {e}"),
            Self::Http(e) => write!(f, "Matrix HTTP error: {e}"),
            Self::Registration(e) => write!(f, "Registration error: {e}"),
            Self::RoomNotFound(id) => write!(f, "Room not found: {id}"),
            Self::KeyExchangeTimeout(msg) => write!(f, "Key exchange timeout: {msg}"),
            Self::RoomCreationTimeout(msg) => write!(
                f,
                "Room creation timeout (server may be rate-limiting): {msg}"
            ),
            Self::Other(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for MatrixClientError {}

impl From<matrix_sdk::Error> for MatrixClientError {
    fn from(e: matrix_sdk::Error) -> Self {
        Self::Sdk(e)
    }
}

impl From<matrix_sdk::HttpError> for MatrixClientError {
    fn from(e: matrix_sdk::HttpError) -> Self {
        Self::Http(e)
    }
}

impl From<anyhow::Error> for MatrixClientError {
    fn from(e: anyhow::Error) -> Self {
        Self::Other(e)
    }
}

impl From<matrix_sdk::ClientBuildError> for MatrixClientError {
    fn from(e: matrix_sdk::ClientBuildError) -> Self {
        Self::Other(e.into())
    }
}

pub type Result<T> = std::result::Result<T, MatrixClientError>;
