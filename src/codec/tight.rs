use crate::{PixelFormat, Rect, VncError, VncEvent};
use anyhow::Result;
use std::io::Read;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite},
    sync::mpsc::Sender,
};
use tracing::error;

use super::{uninit_vec, zlib::ZlibReader};

const MAX_PALETTE: usize = 256;

#[derive(Default)]
pub struct Decoder {
    zlibs: [Option<flate2::Decompress>; 4],
    ctrl: u8,
    filter: u8,
    palette: Vec<u8>,
}

impl Decoder {
    pub fn new() -> Self {
        let mut new = Self {
            palette: Vec::with_capacity(MAX_PALETTE * 4),
            ..Default::default()
        };
        for i in 0..4 {
            let decompressor = flate2::Decompress::new(true);
            new.zlibs[i] = Some(decompressor);
        }
        new
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
        let ctrl = input.read_u8().await?;
        for i in 0..4 {
            if (ctrl >> i) & 1 == 1 {
                self.zlibs[i].as_mut().unwrap().reset(true);
            }
        }

        // Figure out filter
        self.ctrl = ctrl >> 4;

        match self.ctrl {
            8 => {
                // fill Rect
                self.fill_rect(format, rect, input, output).await
            }
            9 => {
                // jpeg Rect
                self.jpeg_rect(format, rect, input, output).await
            }
            10 => {
                // png Rect
                error!("PNG received in standard Tight rect");
                Err(VncError::InvalidImageData.into())
            }
            x if x & 0x8 == 0 => {
                // basic Rect
                self.basic_rect(format, rect, input, output).await
            }
            _ => {
                error!("Illegal tight compression received ({})", self.ctrl);
                Err(VncError::InvalidImageData.into())
            }
        }
    }

    async fn read_data<S>(&mut self, input: &mut S) -> Result<Vec<u8>>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let len = {
            let mut len;
            let mut byte = input.read_u8().await? as usize;
            len = byte & 0x7f;
            if byte & 0x80 == 0x80 {
                byte = input.read_u8().await? as usize;
                len |= (byte & 0x7f) << 7;

                if byte & 0x80 == 0x80 {
                    byte = input.read_u8().await? as usize;
                    len |= (byte as usize) << 14;
                }
            }
            len
        };
        let mut data = uninit_vec(len);
        input.read_exact(&mut data).await?;
        Ok(data)
    }

    async fn fill_rect<S>(
        &mut self,
        format: &PixelFormat,
        rect: &Rect,
        input: &mut S,
        output: &Sender<VncEvent>,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let mut color = [0; 3];
        let alpha = 255;
        input.read_exact(&mut color).await?;
        let bpp = format.bits_per_pixel as usize / 8;
        let mut image = Vec::with_capacity(rect.width as usize * rect.height as usize * bpp);

        let pixel_mask = (format.red_max as u32) << format.red_shift
            | (format.green_max as u32) << format.green_shift
            | (format.blue_max as u32) << format.blue_shift;

        let alpha_shift = match pixel_mask {
            0xff_ff_ff_00 => 0,
            0xff_ff_00_ff => 8,
            0xff_00_ff_ff => 16,
            0x00_ff_ff_ff => 24,
            _ => unreachable!(),
        };
        let true_color = if format.big_endian_flag > 0 {
            // r, g, b
            (((color[0] as u32 & format.red_max as u32) << format.red_shift)
                | ((color[1] as u32 & format.green_max as u32) << format.green_shift)
                | ((color[2] as u32 & format.blue_max as u32) << format.blue_shift)
                | ((alpha as u32) << alpha_shift))
                .to_be_bytes()
        } else {
            // b, g, r
            (((color[2] as u32 & format.red_max as u32) << format.red_shift)
                | ((color[1] as u32 & format.green_max as u32) << format.green_shift)
                | ((color[0] as u32 & format.blue_max as u32) << format.blue_shift)
                | ((alpha as u32) << alpha_shift))
                .to_le_bytes()
        };

        for _ in 0..rect.width {
            for _ in 0..rect.height {
                image.extend_from_slice(&true_color);
            }
        }
        output.send(VncEvent::RawImage(*rect, image)).await?;
        Ok(())
    }

    async fn jpeg_rect<S>(
        &mut self,
        _format: &PixelFormat,
        rect: &Rect,
        input: &mut S,
        output: &Sender<VncEvent>,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let data = self.read_data(input).await?;
        output.send(VncEvent::JpegImage(*rect, data)).await?;
        Ok(())
    }

    async fn basic_rect<S>(
        &mut self,
        format: &PixelFormat,
        rect: &Rect,
        input: &mut S,
        output: &Sender<VncEvent>,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        self.filter = {
            if self.ctrl & 0x4 == 4 {
                input.read_u8().await?
            } else {
                0
            }
        };

        let stream_id = self.ctrl & 0x3;
        match self.filter {
            0 => {
                // copy filter
                self.copy_filter(stream_id, format, rect, input, output)
                    .await
            }
            1 => {
                // palette
                self.palette_filter(stream_id, format, rect, input, output)
                    .await
            }
            2 => {
                // gradient
                error!("Gradient filter not implemented");
                Err(VncError::InvalidImageData.into())
            }
            _ => {
                error!("Illegal tight filter received (filter: {})", self.filter);
                Err(VncError::InvalidImageData.into())
            }
        }
    }

    async fn copy_filter<S>(
        &mut self,
        stream: u8,
        _format: &PixelFormat,
        rect: &Rect,
        input: &mut S,
        output: &Sender<VncEvent>,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let uncompressed_size = rect.width as usize * rect.height as usize * 3;
        if uncompressed_size == 0 {
            return Ok(());
        };

        let mut data;
        if uncompressed_size < 12 {
            data = uninit_vec(uncompressed_size);
            input.read_exact(&mut data).await?;
        } else {
            let d = self.read_data(input).await?;
            let mut reader = ZlibReader::new(self.zlibs[stream as usize].take().unwrap(), &d);
            data = uninit_vec(uncompressed_size);
            reader.read_exact(&mut data)?;
            self.zlibs[stream as usize] = Some(reader.into_inner()?);
        }
        let mut image = Vec::with_capacity(uncompressed_size / 3 * 4);
        let mut j = 0;
        while j < uncompressed_size {
            image.extend_from_slice(&data[j..j + 3]);
            image.push(255);
            j += 3;
        }

        output.send(VncEvent::RawImage(*rect, data)).await?;

        Ok(())
    }

    async fn palette_filter<S>(
        &mut self,
        stream: u8,
        _format: &PixelFormat,
        rect: &Rect,
        input: &mut S,
        output: &Sender<VncEvent>,
    ) -> Result<()>
    where
        S: AsyncRead + AsyncWrite + Unpin,
    {
        let num_colors = input.read_u8().await? as usize + 1;
        let palette_size = num_colors * 3;

        self.palette = uninit_vec(palette_size);
        input.read_exact(&mut self.palette).await?;

        let bpp = if num_colors <= 2 { 1 } else { 8 };
        let row_size = (rect.width as usize * bpp + 7) / 8;
        let uncompressed_size = rect.height as usize * row_size;

        if uncompressed_size == 0 {
            return Ok(());
        }

        let mut data;
        if uncompressed_size < 12 {
            data = uninit_vec(uncompressed_size);
            input.read_exact(&mut data).await?;
        } else {
            let d = self.read_data(input).await?;
            let mut reader = ZlibReader::new(self.zlibs[stream as usize].take().unwrap(), &d);
            data = uninit_vec(uncompressed_size);
            reader.read_exact(&mut data)?;
            self.zlibs[stream as usize] = Some(reader.into_inner()?);
        }

        if num_colors == 2 {
            self.mono_rect(data, rect, output).await?
        } else {
            self.palette_rect(data, rect, output).await?
        }

        Ok(())
    }

    async fn mono_rect(
        &mut self,
        data: Vec<u8>,
        rect: &Rect,
        output: &Sender<VncEvent>,
    ) -> Result<()> {
        // Convert indexed (palette based) image data to RGB
        // TODO: reduce number of calculations inside loop
        let total = rect.width as usize * rect.height as usize * 4;
        let mut image = uninit_vec(total);

        let w = (rect.width as usize + 7) / 8;
        let w1 = rect.width as usize / 8;

        for y in 0..rect.height as usize {
            let mut dp;
            let mut sp;
            for x in 0..w1 {
                for b in (0..=7).rev() {
                    dp = (y * rect.width as usize + x * 8 + 7 - b) * 4;
                    sp = (data[y * w + x] as usize >> b & 1) * 3;
                    image[dp] = self.palette[sp];
                    image[dp + 1] = self.palette[sp + 1];
                    image[dp + 2] = self.palette[sp + 2];
                    image[dp + 3] = 255;
                }
            }
            let x = w1;
            let mut b = 7;
            while b >= 8 - rect.width as usize % 8 {
                dp = (y * rect.width as usize + x * 8 + 7 - b) * 4;
                sp = (data[y * w + x] as usize >> b & 1) * 3;
                image[dp] = self.palette[sp];
                image[dp + 1] = self.palette[sp + 1];
                image[dp + 2] = self.palette[sp + 2];
                image[dp + 3] = 255;
                b -= 1;
            }
        }
        output.send(VncEvent::RawImage(*rect, image)).await?;
        Ok(())
    }

    async fn palette_rect(
        &mut self,
        data: Vec<u8>,
        rect: &Rect,
        output: &Sender<VncEvent>,
    ) -> Result<()> {
        // Convert indexed (palette based) image data to RGB
        let total = rect.width as usize * rect.height as usize * 4;
        let mut image = Vec::with_capacity(total);
        let mut i = 0;
        let mut j = 0;
        while i < total {
            let sp = data[j] as usize * 3;
            image.extend_from_slice(&self.palette[sp..sp + 3]);
            image.push(255);
            i += 4;
            j += 1;
        }
        output.send(VncEvent::RawImage(*rect, image)).await?;
        Ok(())
    }
}
