use anyhow::{Ok, Result};

use std::vec;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::mpsc::{Receiver, Sender},
};
use tracing::{info, trace};

use crate::{codec, PixelFormat, Rect, VncEncoding, VncEvent, X11Event};

use super::messages::{ClientMsg, ServerMsg};

struct ImageRect {
    rect: Rect,
    encoding: VncEncoding,
}

impl From<[u8; 12]> for ImageRect {
    fn from(buf: [u8; 12]) -> Self {
        Self {
            rect: Rect {
                x: (buf[0] as u16) << 8 | buf[1] as u16,
                y: (buf[2] as u16) << 8 | buf[3] as u16,
                width: (buf[4] as u16) << 8 | buf[5] as u16,
                height: (buf[6] as u16) << 8 | buf[7] as u16,
            },
            encoding: ((buf[8] as u32) << 24
                | (buf[9] as u32) << 16
                | (buf[10] as u32) << 8
                | (buf[11] as u32))
                .into(),
        }
    }
}

impl ImageRect {
    async fn read<S>(reader: &mut S) -> Result<Self>
    where
        S: AsyncRead + Unpin,
    {
        let mut rect_buf = [0_u8; 12];
        reader.read_exact(&mut rect_buf).await?;
        Ok(rect_buf.into())
    }
}

pub struct VncClient<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream: S,
    shared: bool,
    pixel_format: Option<PixelFormat>,
    name: String,
    encodings: Vec<VncEncoding>,
    screen: (u16, u16),
}

impl<S> VncClient<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub fn new(
        stream: S,
        shared: bool,
        pixel_format: Option<PixelFormat>,
        encodings: Vec<VncEncoding>,
    ) -> Self {
        Self {
            stream,
            shared,
            pixel_format,
            name: String::new(),
            encodings,
            screen: (0, 0),
        }
    }

    pub async fn run(
        mut self,
        sender: Sender<VncEvent>,
        mut recv: Receiver<X11Event>,
    ) -> Result<()> {
        trace!("client init msg");
        self.send_client_init().await?;
        trace!("server init msg");
        self.read_server_init(&sender).await?;
        trace!("client encodings: {:?}", self.encodings);
        self.send_client_encoding().await?;
        trace!("Require the first frame");
        ClientMsg::FramebufferUpdateRequest(
            Rect {
                x: 0,
                y: 0,
                width: self.screen.0,
                height: self.screen.1,
            },
            0,
        )
        .write(&mut self.stream)
        .await?;

        trace!("Start main loop");
        let mut raw_decoder = codec::RawDecoder::new();
        let pf = self.pixel_format.as_ref().unwrap();
        loop {
            tokio::select! {
                server_msg = ServerMsg::read(&mut self.stream) => {
                    let server_msg = server_msg?;
                    trace!("Server message got: {:?}", server_msg);
                    match server_msg {
                        ServerMsg::FramebufferUpdate(rect_num) => {
                            for _ in 0..rect_num {
                                let rect = ImageRect::read(&mut self.stream).await?;

                                match rect.encoding {
                                    VncEncoding::Raw => {
                                        raw_decoder.decode(pf, &rect.rect, &mut self.stream, &sender).await?;
                                    }
                                    VncEncoding::CopyRect => {
                                        let source_x = self.stream.read_u16().await?;
                                        let source_y = self.stream.read_u16().await?;
                                        let mut src_rect = rect.rect;
                                        src_rect.x = source_x;
                                        src_rect.y = source_y;
                                        sender.send(VncEvent::Copy(rect.rect, src_rect)).await?;
                                    }
                                    VncEncoding::Tight => {
                                        unimplemented!()
                                    }
                                    VncEncoding::Zrle => {
                                        unimplemented!()
                                    }
                                    _ => unimplemented!()
                                }
                            }
                        }
                        // SetColorMapEntries,
                        ServerMsg::Bell => {
                            sender.send(VncEvent::Bell).await?;
                        }
                        ServerMsg::ServerCutText(text) => {
                            sender.send(VncEvent::Text(text)).await?;
                        }
                    }
                }
                x11_event = recv.recv() => {
                    if let Some(x11_event) = x11_event {
                        match x11_event {
                            X11Event::Refresh => {
                                ClientMsg::FramebufferUpdateRequest(
                                    Rect {
                                        x: 0,
                                        y: 0,
                                        width: self.screen.0,
                                        height: self.screen.1,
                                    },
                                    1,
                                )
                                .write(&mut self.stream)
                                .await?;
                            },
                            X11Event::KeyEvent(key) => {
                                ClientMsg::KeyEvent(key.keycode, key.down).write(&mut self.stream).await?;
                            },
                            X11Event::PointerEvent(mouse) => {
                                ClientMsg::PointerEvent(mouse.position_x, mouse.position_y, mouse.bottons).write(&mut self.stream).await?;
                            },
                            X11Event::CopyText(text) => {
                                ClientMsg::ClientCutText(text).write(&mut self.stream).await?;
                            },
                        }
                    }
                }
            }
        }
    }

    async fn send_client_init(&mut self) -> Result<()> {
        info!("Send shared flag: {}", self.shared);
        self.stream.write_u8(self.shared as u8).await?;
        Ok(())
    }

    async fn read_server_init(&mut self, sender: &Sender<VncEvent>) -> Result<()> {
        // +--------------+--------------+------------------------------+
        // | No. of bytes | Type [Value] | Description                  |
        // +--------------+--------------+------------------------------+
        // | 2            | U16          | framebuffer-width in pixels  |
        // | 2            | U16          | framebuffer-height in pixels |
        // | 16           | PIXEL_FORMAT | server-pixel-format          |
        // | 4            | U32          | name-length                  |
        // | name-length  | U8 array     | name-string                  |
        // +--------------+--------------+------------------------------+

        let screen_width = self.stream.read_u16().await?;
        let screen_height = self.stream.read_u16().await?;
        let mut send_our_pf = false;

        sender
            .send(VncEvent::SetResulotin((screen_width, screen_height).into()))
            .await?;
        self.screen = (screen_width, screen_height);

        let pixel_format = PixelFormat::read(&mut self.stream).await?;
        if self.pixel_format.is_none() {
            sender.send(VncEvent::SetPixelFormat(pixel_format)).await?;
            self.pixel_format = Some(pixel_format);
        } else {
            send_our_pf = true;
        }

        let name_len = self.stream.read_u32().await?;
        let mut name_buf = vec![0_u8; name_len as usize];
        self.stream.read_exact(&mut name_buf).await?;
        self.name = String::from_utf8(name_buf)?;

        if send_our_pf {
            info!(
                "Send customized pixel format {:#?}",
                self.pixel_format.as_ref().unwrap()
            );
            ClientMsg::SetPixelFormat(*self.pixel_format.as_ref().unwrap())
                .write(&mut self.stream)
                .await?;
        }
        Ok(())
    }

    async fn send_client_encoding(&mut self) -> Result<()> {
        ClientMsg::SetEncodings(self.encodings.clone())
            .write(&mut self.stream)
            .await?;
        Ok(())
    }
}

impl<S> Drop for VncClient<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    fn drop(&mut self) {
        trace!("Client closed");
    }
}
