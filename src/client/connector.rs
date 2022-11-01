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
    F: Future<Output = Result<String>>,
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

/// Connection Builder to setup a vnc client
pub struct VncConnector<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: Future<Output = Result<String>>,
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
    F: Future<Output = Result<String>>,
{
    /// To new a vnc client configuration with stream `S`
    ///
    /// `S` should implement async I/O methods
    ///
    /// ```no_run
    /// use vnc::{PixelFormat, VncConnector};
    /// use tokio::{self, net::TcpStream};
    /// use anyhow::Result;
    ///
    /// #[tokio::main]
    /// async fn main() -> Result<()> {
    ///     let tcp = TcpStream::connect("127.0.0.1:5900").await?;
    ///     let vnc = VncConnector::new(tcp)
    ///         .set_auth_method(async move { Ok("password".to_string()) })
    ///         .add_encoding(vnc::VncEncoding::Tight)
    ///         .add_encoding(vnc::VncEncoding::Zrle)
    ///         .add_encoding(vnc::VncEncoding::CopyRect)
    ///         .add_encoding(vnc::VncEncoding::Raw)
    ///         .allow_shared(true)
    ///         .set_pixel_format(PixelFormat::bgra())
    ///         .build()?
    ///         .try_start()
    ///         .await?
    ///         .finish()?;
    ///     Ok(())
    /// }
    /// ```
    ///
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

    /// An async callback which is used to query credentials if the vnc server has set
    ///
    /// ```no_compile
    /// connector = connector.set_auth_method(async move { Ok("password".to_string()) })
    /// ```
    ///
    /// if you're building a wasm app,
    /// the async callback also allows you to combine it to a promise
    ///
    /// ```no_compile
    /// #[wasm_bindgen]
    /// extern "C" {
    ///     fn get_password() -> js_sys::Promise;
    /// }
    ///
    /// connector = connector
    ///        .set_auth_method(async move {
    ///            let auth = JsFuture::from(get_password()).await.unwrap();
    ///            Ok(auth.as_string().unwrap())
    ///     });
    /// ```
    ///
    /// While in the js code
    ///
    ///
    /// ```javascript
    /// var password = '';
    /// function get_password() {
    ///     return new Promise((reslove, reject) => {
    ///        document.getElementById("submit_password").addEventListener("click", () => {
    ///             password = window.document.getElementById("input_password").value
    ///             reslove(password)
    ///         })
    ///     });
    /// }
    /// ```
    ///
    /// The future won't be polled if the sever doesn't apply any password protections to the session
    ///
    pub fn set_auth_method(mut self, auth_callback: F) -> Self {
        self.auth_methond = Some(auth_callback);
        self
    }

    /// The max vnc version that we supported
    ///
    /// Version should be one of the [VncVersion]
    ///
    pub fn set_version(mut self, version: VncVersion) -> Self {
        self.rfb_version = version;
        self
    }

    /// Set the rgb order which you will use to resolve the image data
    ///
    /// In most of the case, use `PixelFormat::bgra()` on little endian PCs
    ///
    /// And use `PixelFormat::rgba()` on wasm apps (with canvas)
    ///
    /// Also, customized format is allowed
    ///
    /// Will use the default format informed by the vnc server if not set
    ///
    /// In this condition, the client will get a [crate::VncEvent::SetPixelFormat] event notified
    ///
    pub fn set_pixel_format(mut self, pf: PixelFormat) -> Self {
        self.pixel_format = Some(pf);
        self
    }

    /// Shared-flag is non-zero (true) if the server should try to share the
    ///
    /// desktop by leaving other clients connected, and zero (false) if it
    ///
    /// should give exclusive access to this client by disconnecting all
    ///
    /// other clients.
    ///
    pub fn allow_shared(mut self, allow_shared: bool) -> Self {
        self.allow_shared = allow_shared;
        self
    }

    /// Client encodings that we want to use
    ///
    /// One of [VncEncoding]
    ///
    /// [VncEncoding::Raw] must be sent as the RFC required
    ///
    /// The order to add encodings is the order to inform the server
    ///
    pub fn add_encoding(mut self, encoding: VncEncoding) -> Self {
        self.encodings.push(encoding);
        self
    }

    /// Complete the client configuration
    ///
    pub fn build(self) -> Result<VncState<S, F>> {
        if self.encodings.is_empty() {
            return Err(VncError::NoEncoding.into());
        }
        Ok(VncState::Handshake(self))
    }
}
