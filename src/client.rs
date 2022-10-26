use anyhow::{Ok, Result};
use std::pin::Pin;
use std::{future::Future, vec};
use tokio::io::{AsyncBufRead, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{error::VncError, VncVersion};

const VNC_VER_UNSUPPORTED: &str = "unsupported version";
const VNC_FAILED: &str = "Connection failed with unknow reason";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SecurityType {
    Invalid = 0,
    None = 1,
    VncAuth = 2,
    // RA2 = 5,
    // RA2ne = 6,
    // Tight = 16,
    // Ultra = 17,
    // TLS = 18,
    // VeNCrypt = 19,
}

pub enum VncState<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce() -> String + 'static,
{
    Init(VncConnector<S, F>),
    Handshake(VncConnector<S, F>),
}

impl<S, F> VncState<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin + 'static,
    F: FnOnce() -> String + 'static,
{
    // pub fn handle(self) -> Self {
    //     match self {
    //         VncState::Init => VncState::Handshake,
    //         VncState::Handshake => unreachable!(),
    //     }
    // }

    pub fn try_start(self) -> Pin<Box<dyn Future<Output = Result<Self>>>> {
        Box::pin(async move {
            match self {
                VncState::Init(mut connector) => {
                    let mut rfbversion: [u8; 12] = [0; 12];
                    connector.stream.read_exact(&mut rfbversion).await?;
                    let rfbversion = rfbversion.into();
                    let rfbversion = if connector.max_version < rfbversion {
                        connector.max_version
                    } else {
                        rfbversion
                    };
                    connector
                        .stream
                        .write_all(&<VncVersion as Into<&[u8; 12]>>::into(rfbversion)[..])
                        .await?;
                    Ok(VncState::Handshake(connector).try_start().await?)
                }
                VncState::Handshake(mut connector) => {
                    // +--------------------------+-------------+--------------------------+
                    // | No. of bytes             | Type        | Description              |
                    // |                          | [Value]     |                          |
                    // +--------------------------+-------------+--------------------------+
                    // | 1                        | U8          | number-of-security-types |
                    // | number-of-security-types | U8 array    | security-types           |
                    // +--------------------------+-------------+--------------------------+
                    let num = connector.stream.read_u8().await?;

                    if num == 0 {
                        let err_msg = connector.read_string_with_u32_len().await?;
                        return Err(VncError::Custom(err_msg).into());
                    }

                    unreachable!()
                }
                _ => unimplemented!(),
            }
        })
    }
}

pub struct VncConnector<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce() -> String + 'static,
{
    stream: S,
    auth_methond: Option<F>,
    max_version: VncVersion,
}

impl<S, F> VncConnector<S, F>
where
    S: AsyncRead + AsyncWrite + Unpin,
    F: FnOnce() -> String + 'static,
{
    pub fn new(stream: S) -> Self {
        Self {
            stream,
            auth_methond: None,
            max_version: VncVersion::RFB33,
        }
    }

    pub fn set_auth_method(mut self, auth_callback: F) -> Self {
        self.auth_methond = Some(auth_callback);
        self
    }

    pub fn set_version(mut self, version: VncVersion) -> Self {
        self.max_version = version;
        self
    }

    pub fn build(self) -> VncState<S, F> {
        VncState::Init(self)
    }

    async fn read_string_with_u32_len(&mut self) -> Result<String> {
        let msg_len = self.stream.read_u32().await?;
        let mut msg_vec = vec![0; msg_len as usize];
        self.stream.read_exact(&mut msg_vec).await?;
        Ok(String::from_utf8_lossy(&msg_vec).to_string())
    }
}

pub struct VncClient<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream: S,
}

impl<S> VncClient<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(stream: S) -> Self {
        Self { stream }
    }
}
