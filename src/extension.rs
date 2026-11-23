//! Hooks for protocol extensions that live outside RFC 6143 (file transfer,
//! vendor messages, ...).
//!
//! The core engine only understands the server messages defined by the RFC.
//! Anything else used to tear the connection down. An [`ExtensionHandler`]
//! registered with [`crate::VncConnector::set_extension_handler`] claims a
//! server message type and consumes its payload from the same stream the
//! framebuffer decoder uses, so ordering is preserved. Client-side extension
//! messages are written with [`crate::X11Event::Raw`] through the ordinary
//! input queue, which keeps a single writer on the socket.

use crate::VncError;
use std::{future::Future, pin::Pin};
use tokio::io::AsyncRead;

/// Byte stream positioned right after the extension message type byte.
pub type ExtensionStream<'a> = &'a mut (dyn AsyncRead + Unpin + Send);

/// Future returned by [`ExtensionHandler::read`].
pub type ExtensionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Option<ExtensionEvent>, VncError>> + Send + 'a>>;

/// An extension message decoded by an [`ExtensionHandler`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionEvent {
    /// Server message type this payload arrived with.
    pub kind: u8,
    /// Handler-defined payload.
    pub data: Vec<u8>,
}

/// Consumes server messages of types the core protocol does not know.
pub trait ExtensionHandler: Send + Sync + 'static {
    /// Whether this handler owns server message type `kind`.
    ///
    /// Core types (0 FramebufferUpdate, 1 SetColorMapEntries, 2 Bell,
    /// 3 ServerCutText) are never offered to handlers. A type that no handler
    /// claims is a protocol error, exactly as before.
    fn claims(&self, kind: u8) -> bool;

    /// Read the remainder of one `kind` message from `stream`.
    ///
    /// The RFB wire format has no generic length prefix, so the handler **must**
    /// read exactly the bytes of the message, and must bound every length it
    /// reads from the peer. Returning an error closes the session.
    /// Returning `Ok(None)` consumes the message without reporting an event.
    fn read<'a>(&'a mut self, kind: u8, stream: ExtensionStream<'a>) -> ExtensionFuture<'a>;
}
