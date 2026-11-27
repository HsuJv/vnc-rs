use super::{AuthResult, SecurityType};
use crate::{VncError, VncVersion};
use std::future::Future;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

mod des;

pub(super) struct Config<F> {
    callback: Option<F>,
}

impl<F> Config<F> {
    pub(super) fn new() -> Self {
        Self { callback: None }
    }

    pub(super) fn available(&self, methods: &[SecurityType]) -> bool {
        methods.contains(&SecurityType::VncAuth) && self.callback.is_some()
    }
}

impl<F> Config<F>
where
    F: Future<Output = Result<String, VncError>>,
{
    pub(super) async fn authenticate<S>(
        &mut self,
        stream: &mut S,
        version: VncVersion,
        methods: &[SecurityType],
    ) -> Result<(), VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        if methods.contains(&SecurityType::VncAuth) {
            if version != VncVersion::RFB33 {
                SecurityType::VncAuth.write(stream).await?;
            }
        } else {
            return Err(VncError::General(
                "Security type apart from Vnc Auth has not been implemented".to_owned(),
            ));
        }
        let callback = self.callback.take().ok_or(VncError::NoPassword)?;
        let credential = callback.await?;
        let auth = AuthHelper::read(stream, &credential).await?;
        auth.write(stream).await?;
        if let AuthResult::Failed = auth.finish(stream).await? {
            if version != VncVersion::RFB38 {
                return Err(VncError::WrongPassword);
            }
            return Err(VncError::General(
                crate::limits::string(stream, crate::limits::MAX_NAME).await?,
            ));
        }
        Ok(())
    }
}

impl<F> super::Config<F> {
    pub(in crate::client) fn with_password<A>(self, callback: A) -> super::Config<A> {
        super::Config {
            vnc: Config {
                callback: Some(callback),
            },
            ard: self.ard,
        }
    }
}

pub(super) struct AuthHelper {
    challenge: [u8; 16],
    key: [u8; 8],
}

impl AuthHelper {
    pub(super) async fn read<S>(reader: &mut S, credential: &str) -> Result<Self, VncError>
    where
        S: AsyncRead + Unpin,
    {
        let mut challenge = [0; 16];
        reader.read_exact(&mut challenge).await?;

        let credential_len = credential.len();
        let mut key = [0u8; 8];
        for (i, key_i) in key.iter_mut().enumerate() {
            let c = if i < credential_len {
                credential.as_bytes()[i]
            } else {
                0
            };
            let mut cs = 0u8;
            for j in 0..8 {
                cs |= ((c >> j) & 1) << (7 - j)
            }
            *key_i = cs;
        }

        Ok(Self { challenge, key })
    }

    pub(super) async fn write<S>(&self, writer: &mut S) -> Result<(), VncError>
    where
        S: AsyncWrite + Unpin,
    {
        let encrypted = des::encrypt(&self.challenge, &self.key);
        writer.write_all(&encrypted).await?;
        Ok(())
    }

    pub(super) async fn finish<S>(self, reader: &mut S) -> Result<AuthResult, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let result = reader.read_u32().await?;
        result.try_into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn authentication_result_is_checked() {
        for status in [0_u32, 1, 2, 256, u32::MAX] {
            let (mut server, mut client) = tokio::io::duplex(4);
            server.write_all(&status.to_be_bytes()).await.unwrap();
            let auth = AuthHelper {
                challenge: [0; 16],
                key: [0; 8],
            };

            let result = auth.finish(&mut client).await;
            match status {
                0 => assert!(matches!(result, Ok(AuthResult::Ok))),
                1 => assert!(matches!(result, Ok(AuthResult::Failed))),
                _ => assert!(matches!(result, Err(VncError::General(_)))),
            }
        }
    }
}
