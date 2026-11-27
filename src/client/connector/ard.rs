use super::VncConnector;
use crate::VncError;
use std::future::Future;
use tokio::io::{AsyncRead, AsyncWrite};

impl<S, F> VncConnector<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + Sync + 'static,
    F: Future<Output = Result<String, VncError>> + Send + Sync + 'static,
{
    /// Enable Apple Remote Desktop (RFB security type 30) authentication.
    ///
    /// When set, the client answers Apple's `RFB 003.889` version with the
    /// 3.8 handshake so the server offers its security-type list, and signs
    /// in with the macOS account `username` and `password` when type 30 is
    /// available. This bypasses the login window that plain VNC
    /// authentication shows on macOS 10.7 and later.
    ///
    /// When the server does not offer ARD the connection fails with
    /// [VncError::ArdNotOffered] unless a legacy method could actually
    /// complete: an unauthenticated session, or VNC authentication with
    /// credentials supplied via `set_auth_method` (with `auth-vnc`).
    ///
    /// The ARD flow requires the 3.8 handshake. If [Self::set_version]
    /// limited the client to an older version, the connection to an Apple
    /// server fails with [VncError::ArdVersionConflict] instead of silently
    /// raising that limit.
    pub fn set_ard_credentials(
        mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        self.auth.set_ard_credentials(username, password);
        self
    }
}
