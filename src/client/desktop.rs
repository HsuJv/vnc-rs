//! Desktop layout coordination, update batches and resize requests.

use crate::client::{connection::pack_screen, messages::ClientMsg};
use crate::{
    DesktopLayout, DesktopReason, DesktopStatus, DesktopUpdate, ResizeError, VncError, VncEvent,
};
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc, Mutex, MutexGuard,
    },
};
use tokio::sync::{mpsc::Sender, oneshot};
#[cfg(not(target_arch = "wasm32"))]
use tokio::time::{timeout, Duration};

#[derive(Clone, Default)]
pub(crate) struct Desktop {
    state: Arc<DesktopState>,
}

impl Desktop {
    pub(crate) fn close(&self) {
        self.state.close();
    }

    pub(crate) fn legacy_resize(&self) {
        self.state.legacy_resize();
    }

    pub(crate) fn batch(&self) -> Batch<'_> {
        Batch {
            desktop: self,
            updates: crate::desktop::UpdateBatch::default(),
            framebuffer_changed: false,
        }
    }

    pub(in crate::client) fn client(&self, _input: &Sender<ClientMsg>) -> Client {
        Client {
            desktop: self.clone(),
            #[cfg(not(target_arch = "wasm32"))]
            input: _input.clone(),
        }
    }
}

#[derive(Clone)]
pub(crate) struct Client {
    desktop: Desktop,
    #[cfg(not(target_arch = "wasm32"))]
    input: Sender<ClientMsg>,
}

impl Client {
    pub(in crate::client) fn layout(&self) -> Option<DesktopLayout> {
        self.desktop.state.layout()
    }
    #[cfg(not(target_arch = "wasm32"))]
    pub(in crate::client) async fn resize(
        &self,
        width: u16,
        height: u16,
    ) -> Result<DesktopLayout, crate::ResizeError> {
        self.desktop.state.request(&self.input, width, height).await
    }
}

pub(crate) struct Batch<'a> {
    desktop: &'a Desktop,
    updates: crate::desktop::UpdateBatch,
    framebuffer_changed: bool,
}

impl Batch<'_> {
    pub(crate) fn before_pixels(&mut self) -> Result<(), VncError> {
        if !self.updates.is_empty() {
            return Err(VncError::InvalidImageData);
        }
        self.framebuffer_changed = true;
        Ok(())
    }

    pub(crate) async fn extended<F, Fut>(
        &mut self,
        update: DesktopUpdate,
        screen: &mut (u16, u16),
        shared: &AtomicU32,
        output: &F,
    ) -> Result<(), VncError>
    where
        F: Fn(VncEvent) -> Fut,
        Fut: Future<Output = Result<(), VncError>>,
    {
        if update.reason == DesktopReason::ThisClient || !self.updates.is_empty() {
            // Keep client acknowledgements atomic until the full message is validated.
            if self.framebuffer_changed {
                return Err(VncError::InvalidImageData);
            }
            self.updates.push(update)?;
        } else {
            // Server announcements can precede pixels in the same update.
            if let Some(layout) = &update.layout {
                *screen = (layout.width, layout.height);
                shared.store(pack_screen(*screen), Ordering::Release);
            }
            self.desktop.state.observe(&update);
            output(VncEvent::DesktopUpdate(update)).await?;
        }
        Ok(())
    }

    pub(crate) async fn finish<F, Fut>(
        self,
        screen: &mut (u16, u16),
        shared: &AtomicU32,
        output: &F,
    ) -> Result<(), VncError>
    where
        F: Fn(VncEvent) -> Fut,
        Fut: Future<Output = Result<(), VncError>>,
    {
        for update in self.updates.into_updates() {
            if let Some(layout) = &update.layout {
                *screen = (layout.width, layout.height);
                shared.store(pack_screen(*screen), Ordering::Release);
            }
            self.desktop.state.observe(&update);
            output(VncEvent::DesktopUpdate(update)).await?;
        }
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
const RESIZE_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
struct DesktopState {
    inner: Mutex<State>,
}

#[derive(Default)]
struct State {
    layout: Option<DesktopLayout>,
    pending: Option<Pending>,
    uncertain: bool,
    closed: bool,
    #[cfg(not(target_arch = "wasm32"))]
    busy: bool,
}

struct Pending {
    requested: DesktopLayout,
    reply: oneshot::Sender<Result<DesktopLayout, ResizeError>>,
}

impl DesktopState {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn layout(&self) -> Option<DesktopLayout> {
        self.lock().layout.clone()
    }

    fn observe(&self, update: &DesktopUpdate) {
        let mut state = self.lock();
        if let Some(layout) = &update.layout {
            state.layout = Some(layout.clone());
        }
        if update.reason != DesktopReason::ThisClient {
            return;
        }
        let Some(pending) = state.pending.take() else {
            return;
        };
        let result = match update.status {
            DesktopStatus::Success if update.layout.as_ref() == Some(&pending.requested) => {
                Ok(pending.requested)
            }
            DesktopStatus::Success | DesktopStatus::Forwarded => {
                state.uncertain = true;
                Err(ResizeError::Uncertain)
            }
            status => Err(ResizeError::Denied(status)),
        };
        let _ = pending.reply.send(result);
    }

    fn legacy_resize(&self) {
        let mut state = self.lock();
        state.layout = None;
        if let Some(pending) = state.pending.take() {
            state.uncertain = true;
            let _ = pending.reply.send(Err(ResizeError::Uncertain));
        }
    }

    fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        if let Some(pending) = state.pending.take() {
            state.uncertain = true;
            let _ = pending.reply.send(Err(ResizeError::Uncertain));
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    async fn request(
        self: &Arc<Self>,
        input: &Sender<ClientMsg>,
        width: u16,
        height: u16,
    ) -> Result<DesktopLayout, ResizeError> {
        // Reserve before recording a pending mutation. Cancellation while waiting
        // for capacity cannot leave a command queued for later transmission.
        let permit = timeout(RESIZE_TIMEOUT, input.reserve())
            .await
            .map_err(|_| ResizeError::DispatchTimeout)?
            .map_err(|_| ResizeError::Disconnected)?;
        let receiver = {
            let mut state = self.lock();
            if state.closed {
                return Err(ResizeError::Disconnected);
            }
            if state.uncertain {
                return Err(ResizeError::Uncertain);
            }
            if state.busy {
                return Err(ResizeError::Busy);
            }
            let current = state.layout.as_ref().ok_or(ResizeError::Unsupported)?;
            let requested = current.resized(width, height)?;
            if *current == requested {
                return Ok(requested);
            }
            let (reply, receiver) = oneshot::channel();
            state.busy = true;
            state.pending = Some(Pending {
                requested: requested.clone(),
                reply,
            });
            permit.send(ClientMsg::SetDesktopSize(requested));
            receiver
        };
        let mut guard = PendingGuard {
            state: Arc::clone(self),
            completed: false,
        };
        let result = match timeout(RESIZE_TIMEOUT, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(ResizeError::Uncertain),
            Err(_) => return Err(ResizeError::Timeout),
        };
        guard.completed = true;
        result
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct PendingGuard {
    state: Arc<DesktopState>,
    completed: bool,
}
#[cfg(not(target_arch = "wasm32"))]
impl Drop for PendingGuard {
    fn drop(&mut self) {
        let mut state = self.state.lock();
        state.busy = false;
        if !self.completed {
            state.pending = None;
            state.uncertain = true;
        }
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use crate::ScreenLayout;

    #[tokio::test]
    async fn full_queue_timeout_does_not_dispatch_or_poison_a_retry() {
        let state = Arc::new(DesktopState::default());
        state.lock().layout = Some(DesktopLayout {
            width: 10,
            height: 10,
            screens: vec![ScreenLayout {
                id: 7,
                x: 0,
                y: 0,
                width: 10,
                height: 10,
                flags: 0,
            }],
        });
        let (input, mut receiver) = tokio::sync::mpsc::channel(1);
        input.send(ClientMsg::KeyEvent(0, false)).await.unwrap();
        assert_eq!(
            state.request(&input, 20, 20).await,
            Err(ResizeError::DispatchTimeout)
        );
        {
            let inner = state.lock();
            assert!(inner.pending.is_none());
            assert!(!inner.busy && !inner.uncertain);
        }
        assert!(matches!(
            receiver.recv().await,
            Some(ClientMsg::KeyEvent(0, false))
        ));
        assert!(receiver.try_recv().is_err());
        let retry_state = Arc::clone(&state);
        let retry = tokio::spawn(async move { retry_state.request(&input, 20, 20).await });
        let Some(ClientMsg::SetDesktopSize(requested)) = receiver.recv().await else {
            panic!("retry did not dispatch resize");
        };
        state.observe(&DesktopUpdate {
            reason: DesktopReason::ThisClient,
            status: DesktopStatus::Success,
            layout: Some(requested.clone()),
        });
        assert_eq!(retry.await.unwrap(), Ok(requested));
        let inner = state.lock();
        assert!(inner.pending.is_none());
        assert!(!inner.busy && !inner.uncertain);
    }
}
