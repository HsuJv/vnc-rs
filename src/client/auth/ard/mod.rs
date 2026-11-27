use super::{AuthResult, SecurityType};
use crate::{VncError, VncVersion};
use protocol::{ArdCredentials, APPLE_RFB_VERSION};
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::info;

mod protocol;

pub(super) struct Config {
    credentials: Option<ArdCredentials>,
}

impl Config {
    pub(super) fn new() -> Self {
        Self { credentials: None }
    }

    pub(super) fn negotiate_version(
        &self,
        client: VncVersion,
        server: VncVersion,
        raw: &[u8; 12],
    ) -> Result<VncVersion, VncError> {
        if self.credentials.is_some() && raw == APPLE_RFB_VERSION {
            // ARD requires the 3.8 handshake, never silently raise the client's limit.
            if client < VncVersion::RFB38 {
                return Err(VncError::ArdVersionConflict(client));
            }
            Ok(VncVersion::RFB38)
        } else {
            Ok(if client < server { client } else { server })
        }
    }

    pub(super) async fn authenticate<S>(
        &mut self,
        stream: &mut S,
        methods: &[SecurityType],
        fallback: bool,
    ) -> Result<bool, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let Some(credentials) = self.credentials.take() else {
            return Ok(false);
        };
        if methods.contains(&SecurityType::AppleArd) {
            SecurityType::AppleArd.write(stream).await?;
            if let AuthResult::Failed = protocol::authenticate(stream, &credentials).await? {
                let reason = crate::limits::string(stream, crate::limits::MAX_NAME)
                    .await
                    .unwrap_or_else(|_| "connection closed".to_owned());
                return Err(VncError::ArdAuthFailed(reason));
            }
            info!("Apple ARD auth done, client connected");
            return Ok(true);
        }
        if !fallback {
            return Err(VncError::ArdNotOffered);
        }
        Ok(false)
    }
}

impl<F> super::Config<F> {
    pub(in crate::client) fn set_ard_credentials(
        &mut self,
        username: impl Into<String>,
        password: impl Into<String>,
    ) {
        self.ard.credentials = Some(ArdCredentials {
            username: username.into(),
            password: password.into(),
        });
    }
}
