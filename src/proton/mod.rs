use std::error::Error;
use std::fmt;
use std::time::Duration;

pub const STREAM_EVENT: u8 = 1;
pub const STREAM_STATE_COMMIT: u8 = 2;
pub const STREAM_ACTION: u8 = 3;
pub const MAX_BIDIRECTIONAL_STREAMS: u32 = 3;
pub const MAX_CONNECTIONS: u32 = 1;

// Protocol timeouts
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(5);
pub const STARTUP_DELAY: Duration = Duration::from_secs(10); // 2 * IDLE_TIMEOUT
pub const STREAM_TIMEOUT: Duration = Duration::from_secs(300); // 5 minutes

#[derive(Debug)]
pub enum ProtonError {
    IoError(std::io::Error),
    ConnectionError,
    InvalidStream,
}

impl fmt::Display for ProtonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtonError::IoError(e) => write!(f, "IO error: {}", e),
            ProtonError::ConnectionError => write!(f, "Connection error"),
            ProtonError::InvalidStream => write!(f, "Invalid stream"),
        }
    }
}

impl Error for ProtonError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ProtonError::IoError(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for ProtonError {
    fn from(error: std::io::Error) -> Self {
        ProtonError::IoError(error)
    }
}

impl From<quinn::ConnectError> for ProtonError {
    fn from(_: quinn::ConnectError) -> Self {
        ProtonError::ConnectionError
    }
}

impl From<quinn::ConnectionError> for ProtonError {
    fn from(_: quinn::ConnectionError) -> Self {
        ProtonError::ConnectionError
    }
}

impl From<quinn::WriteError> for ProtonError {
    fn from(_: quinn::WriteError) -> Self {
        ProtonError::ConnectionError
    }
}

impl From<tokio::time::error::Elapsed> for ProtonError {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        ProtonError::ConnectionError
    }
}

impl From<quinn::ReadExactError> for ProtonError {
    fn from(_: quinn::ReadExactError) -> Self {
        ProtonError::ConnectionError
    }
}

mod client;
mod server;

pub use client::ProtonClient;
pub use server::ProtonServer;
