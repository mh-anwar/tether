use std::io;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("i/o: {0}")]
    Io(#[from] io::Error),
    #[error("malformed packet: {0}")]
    Json(#[from] serde_json::Error),
    #[error("tls: {0}")]
    Tls(#[from] rustls::Error),
    #[error("certificate: {0}")]
    Cert(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("timed out")]
    Timeout,
    #[error("unknown device {0}")]
    UnknownDevice(String),
    #[error("device {0} is not connected")]
    NotConnected(String),
    #[error("device {0} is not paired")]
    NotPaired(String),
}

impl From<tokio::time::error::Elapsed> for Error {
    fn from(_: tokio::time::error::Elapsed) -> Self {
        Error::Timeout
    }
}

pub type Result<T> = std::result::Result<T, Error>;
