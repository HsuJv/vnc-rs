use thiserror::Error;

#[non_exhaustive]
#[derive(Debug, Error, Clone, Copy)]
pub enum VncError {
    #[error("Error Test")]
    ErrorInit,
}

pub fn test() -> Result<(), VncError> {
    Err(VncError::ErrorInit)
}
