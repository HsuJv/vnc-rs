use super::SecurityType;
use crate::{VncError, VncVersion};
use std::{future::Future, marker::PhantomData};
use tokio::io::{AsyncRead, AsyncWrite};

pub(super) struct Config<F>(PhantomData<F>);

impl<F> Config<F> {
    pub(super) fn new() -> Self {
        Self(PhantomData)
    }
    pub(super) fn available(&self, _: &[SecurityType]) -> bool {
        false
    }
}

impl<F> Config<F>
where
    F: Future<Output = Result<String, VncError>>,
{
    pub(super) async fn authenticate<S>(
        &mut self,
        _: &mut S,
        _: VncVersion,
        _: &[SecurityType],
    ) -> Result<(), VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        Err(VncError::General(
            "No supported authentication method offered".to_owned(),
        ))
    }
}
