use crate::{PixelFormat, Rect, VncEvent};
use anyhow::Result;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    sync::mpsc::Sender,
};

use super::uninit_vec;

pub struct Decoder {}

impl Decoder {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn decode<S>(
        &mut self,
        format: &PixelFormat,
        rect: &Rect,
        input: &mut S,
        output: &Sender<VncEvent>,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        // +----------------------------+--------------+-------------+
        // | No. of bytes               | Type [Value] | Description |
        // +----------------------------+--------------+-------------+
        // | width*height*bytesPerPixel | PIXEL array  | pixels      |
        // +----------------------------+--------------+-------------+
        let bpp = format.bits_per_pixel / 8;
        let buffer_size = bpp as usize * rect.height as usize * rect.width as usize;
        let mut pixels = uninit_vec(buffer_size);
        input.read_exact(&mut pixels).await?;
        output.send(VncEvent::RawImage(*rect, pixels)).await?;
        Ok(())
    }
}
