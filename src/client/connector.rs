use super::{
    auth::{AuthHelper, AuthResult, SecurityType},
    connection::VncClient,
};
use anyhow::{Ok, Result};
use std::future::Future;
use std::pin::Pin;
use tokio::io::{AsyncRead, AsyncWrite};
use tracing::{info, trace};

use crate::{PixelFormat, VncEncoding, VncError, VncVersion};

pub enum VncState<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Future<Output = Result<String>> + 'static,
{
    Handshake(VncConnector<S, F>),
    Authenticate(VncConnector<S, F>),
    Connected(VncClient<S>),
}

impl<S, F> VncState<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + 'static,
    F: Future<Output = Result<String>> + 'static,
{
    pub fn try_start(self) -> Pin<Box<dyn Future<Output = Result<Self>>>> {
        Box::pin(async move {
            match self {
                VncState::Handshake(mut connector) => {
                    // Read the rfbversion informed by the server
                    let rfbversion = VncVersion::read(&mut connector.stream).await?;
                    let rfbversion = if connector.rfb_version < rfbversion {
                        connector.rfb_version
                    } else {
                        rfbversion
                    };

                    // Record the negotiated rfbversion
                    connector.rfb_version = rfbversion;
                    trace!("Negotiated rfb version: {:?}", rfbversion);
                    rfbversion.write(&mut connector.stream).await?;
                    Ok(VncState::Authenticate(connector).try_start().await?)
                }
                VncState::Authenticate(mut connector) => {
                    let security_types =
                        SecurityType::read(&mut connector.stream, &connector.rfb_version).await?;

                    assert!(!security_types.is_empty());

                    if let SecurityType::None = security_types[0] {
                        trace!("No auth needed");
                    } else {
                        // choose a auth method

                        // get password
                        if connector.auth_methond.is_none() {
                            return Err(VncError::NoPassword.into());
                        }
                        let credential = (connector.auth_methond.take().unwrap()).await?;

                        // auth
                        let auth = AuthHelper::read(&mut connector.stream, &credential).await?;
                        auth.write(&mut connector.stream).await?;
                        let result = auth.finish(&mut connector.stream).await?;
                        if let AuthResult::Failed = result {
                            return Err(VncError::WrongPassword.into());
                        }
                    }
                    info!("auth done, client connected");

                    Ok(VncState::Connected(VncClient::new(
                        connector.stream,
                        connector.allow_shared,
                        connector.pixel_format,
                        connector.encodings,
                    )))
                }
                _ => unreachable!(),
            }
        })
    }

    pub fn finish(self) -> Result<VncClient<S>> {
        if let VncState::Connected(client) = self {
            Ok(client)
        } else {
            Err(VncError::ConnectError.into())
        }
    }
}

pub struct VncConnector<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Future<Output = Result<String>> + 'static,
{
    stream: S,
    auth_methond: Option<F>,
    rfb_version: VncVersion,
    allow_shared: bool,
    pixel_format: Option<PixelFormat>,
    encodings: Vec<VncEncoding>,
}

impl<S, F> VncConnector<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Future<Output = Result<String>> + 'static,
{
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            auth_methond: None,
            allow_shared: true,
            rfb_version: VncVersion::RFB33,
            pixel_format: None,
            encodings: Vec::new(),
        }
    }

    pub fn set_auth_method(mut self, auth_callback: F) -> Self {
        self.auth_methond = Some(auth_callback);
        self
    }

    pub fn set_version(mut self, version: VncVersion) -> Self {
        self.rfb_version = version;
        self
    }

    pub fn set_pixel_format(mut self, pf: PixelFormat) -> Self {
        self.pixel_format = Some(pf);
        self
    }

    pub fn allow_shared(mut self, allow_shared: bool) -> Self {
        self.allow_shared = allow_shared;
        self
    }

    pub fn add_encoding(mut self, encoding: VncEncoding) -> Self {
        self.encodings.push(encoding);
        self
    }

    pub fn build(self) -> Result<VncState<S, F>> {
        if self.encodings.is_empty() {
            return Err(VncError::NoEncoding.into());
        }
        Ok(VncState::Handshake(self))
    }
}
