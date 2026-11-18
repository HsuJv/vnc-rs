use super::VncClient;

impl VncClient {
    /// Last validated ExtendedDesktopSize layout; None means support is unconfirmed.
    pub fn desktop_layout(&self) -> Option<crate::DesktopLayout> {
        self.desktop.layout()
    }

    /// Request a negotiated single-screen resize and wait for the server result.
    /// Available on native targets with a Tokio time driver; wasm supports observation only.
    /// Continue draining events concurrently. Each queue/confirmation wait is bounded
    /// to five seconds. Cancellation after dispatch makes later requests uncertain;
    /// reconnect and observe before retrying. Dispatch includes one incremental one-pixel
    /// update request so servers can deliver the confirmation.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn resize_desktop(
        &self,
        width: u16,
        height: u16,
    ) -> Result<crate::DesktopLayout, crate::ResizeError> {
        self.desktop.resize(width, height).await
    }
}
