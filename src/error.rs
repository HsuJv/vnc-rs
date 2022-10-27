use thiserror::Error;

#[non_exhaustive]
#[derive(Debug, Error, Clone)]
pub enum VncError {
    #[error("Auth is required but no password provided")]
    NoPassword,
    #[error("No vnc encoding selected")]
    NoEncoding,
    #[error("Wrong password")]
    WrongPassword,
    #[error("Connect error with unknown reason")]
    ConnectError,
    #[error("Unknown pixel format")]
    WrongPixelFormat,
    #[error("Unkonw server message")]
    WrongServerMessage,
    #[error("Vnc Error with message: {0}")]
    Custom(String),
}
