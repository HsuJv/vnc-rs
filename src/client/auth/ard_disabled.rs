use super::SecurityType;
use crate::{VncError, VncVersion};
use tokio::io::{AsyncRead, AsyncWrite};

pub(super) struct Config;

impl Config {
    pub(super) fn new() -> Self {
        Self
    }

    pub(super) fn negotiate_version(
        &self,
        client: VncVersion,
        server: VncVersion,
        _: &[u8; 12],
    ) -> Result<VncVersion, VncError> {
        Ok(if client < server { client } else { server })
    }

    pub(super) async fn authenticate<S>(
        &mut self,
        _: &mut S,
        _: &[SecurityType],
        _: bool,
    ) -> Result<bool, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        Ok(false)
    }
}
