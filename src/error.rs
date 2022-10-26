use thiserror::Error;

#[non_exhaustive]
#[derive(Debug, Error, Clone)]
pub enum VncError {
    #[error("Cannot start the vnc handle shake with the server")]
    Handleshake,
    #[error("Vnc Error with message: {0}")]
    Custom(String),
}
