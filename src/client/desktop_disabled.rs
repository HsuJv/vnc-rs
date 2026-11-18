//! Zero-sized desktop coordination when resize support is disabled.

use super::messages::ClientMsg;
use crate::{VncError, VncEvent};
use std::{future::Future, sync::atomic::AtomicU32};
use tokio::sync::mpsc::Sender;

#[derive(Clone, Default)]
pub(crate) struct Desktop {}

impl Desktop {
    pub(crate) fn close(&self) {}

    pub(crate) fn legacy_resize(&self) {}

    pub(crate) fn batch(&self) -> Batch {
        Batch
    }

    pub(in crate::client) fn client(&self, _input: &Sender<ClientMsg>) -> Client {
        Client {}
    }
}

#[derive(Clone)]
pub(crate) struct Client {}

pub(crate) struct Batch;

impl Batch {
    pub(crate) fn before_pixels(&mut self) -> Result<(), VncError> {
        Ok(())
    }

    pub(crate) async fn finish<F, Fut>(
        self,
        _screen: &mut (u16, u16),
        _shared: &AtomicU32,
        _output: &F,
    ) -> Result<(), VncError>
    where
        F: Fn(VncEvent) -> Fut,
        Fut: Future<Output = Result<(), VncError>>,
    {
        Ok(())
    }
}
