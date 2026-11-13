use crate::{PixelFormat, Rect, VncError, VncEvent};
use std::future::Future;
use tokio::io::{AsyncRead, AsyncReadExt};

const TILE: u16 = 16;
const MAX_PALETTE: usize = 128;

/// RFC 6143 section 7.7.5 remembers the palette, the solid colour and the
/// previous subencoding per rectangle, so this state is rebuilt on every
/// `decode` call instead of living in `Decoder`.
#[derive(Default)]
struct TileState {
    last_type: u8,
    palette: Vec<u8>,
    fill_color: Vec<u8>,
}

fn bits_per_index(palette_len: u8) -> Result<usize, VncError> {
    match palette_len {
        2 => Ok(1),
        3..=4 => Ok(2),
        5..=16 => Ok(4),
        _ => Err(VncError::InvalidImageData),
    }
}

async fn read_cpixel<S>(
    input: &mut S,
    compressed_bpp: usize,
    bpp: usize,
    alpha_at_first: bool,
) -> Result<Vec<u8>, VncError>
where
    S: AsyncRead + Unpin,
{
    let mut pixel = [255_u8; 4];
    let start = usize::from(alpha_at_first);
    input
        .read_exact(&mut pixel[start..start + compressed_bpp])
        .await?;
    Ok(pixel[..bpp].to_vec())
}

async fn read_palette<S>(
    input: &mut S,
    entries: usize,
    compressed_bpp: usize,
    bpp: usize,
    alpha_at_first: bool,
) -> Result<Vec<u8>, VncError>
where
    S: AsyncRead + Unpin,
{
    let mut palette = Vec::with_capacity(entries * bpp);
    for _ in 0..entries {
        palette.extend(read_cpixel(input, compressed_bpp, bpp, alpha_at_first).await?);
    }
    Ok(palette)
}

fn copy_indexed(
    palette: &[u8],
    pixels: &mut Vec<u8>,
    bpp: usize,
    index: usize,
    repeats: usize,
) -> Result<(), VncError> {
    let pixel = palette
        .get(index * bpp..index * bpp + bpp)
        .ok_or(VncError::InvalidImageData)?;
    for _ in 0..repeats {
        pixels.extend_from_slice(pixel);
    }
    Ok(())
}

async fn read_run_length<S>(input: &mut S, remaining: usize) -> Result<usize, VncError>
where
    S: AsyncRead + Unpin,
{
    let mut length = 1_usize;
    loop {
        let part = usize::from(input.read_u8().await?);
        length += part;
        if length > remaining {
            return Err(VncError::InvalidImageData);
        }
        if part != 255 {
            break;
        }
    }
    Ok(length)
}

async fn read_raw_tile<S>(
    input: &mut S,
    width: u16,
    height: u16,
    compressed_bpp: usize,
    bpp: usize,
    alpha_at_first: bool,
) -> Result<Vec<u8>, VncError>
where
    S: AsyncRead + Unpin,
{
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * bpp);
    for _ in 0..u32::from(width) * u32::from(height) {
        pixels.extend(read_cpixel(input, compressed_bpp, bpp, alpha_at_first).await?);
    }
    Ok(pixels)
}

async fn read_indexed_tile<S>(
    input: &mut S,
    width: u16,
    height: u16,
    palette: &[u8],
    bpp: usize,
    bits: usize,
) -> Result<Vec<u8>, VncError>
where
    S: AsyncRead + Unpin,
{
    let mask = (1_u8 << bits) - 1;
    let row_bytes = (usize::from(width) * bits).div_ceil(8);
    let mut row = vec![0_u8; row_bytes];
    let mut pixels = Vec::with_capacity(usize::from(width) * usize::from(height) * bpp);
    for _ in 0..height {
        input.read_exact(&mut row).await?;
        for col in 0..width {
            let bit = usize::from(col) * bits;
            let index = usize::from((row[bit / 8] >> (8 - bits - bit % 8)) & mask);
            copy_indexed(palette, &mut pixels, bpp, index, 1)?;
        }
    }
    Ok(pixels)
}

async fn read_plain_rle_tile<S>(
    input: &mut S,
    width: u16,
    height: u16,
    compressed_bpp: usize,
    bpp: usize,
    alpha_at_first: bool,
) -> Result<Vec<u8>, VncError>
where
    S: AsyncRead + Unpin,
{
    let total = usize::from(width) * usize::from(height);
    let mut pixels = Vec::with_capacity(total * bpp);
    while pixels.len() / bpp < total {
        let color = read_cpixel(input, compressed_bpp, bpp, alpha_at_first).await?;
        let run = read_run_length(input, total - pixels.len() / bpp).await?;
        for _ in 0..run {
            pixels.extend_from_slice(&color);
        }
    }
    Ok(pixels)
}

async fn read_palette_rle_tile<S>(
    input: &mut S,
    width: u16,
    height: u16,
    palette: &[u8],
    bpp: usize,
) -> Result<Vec<u8>, VncError>
where
    S: AsyncRead + Unpin,
{
    let total = usize::from(width) * usize::from(height);
    let mut filled = 0_usize;
    let mut pixels = Vec::with_capacity(total * bpp);
    while filled < total {
        let control = input.read_u8().await?;
        let index = usize::from(control & 0x7f);
        let run = if control & 0x80 != 0 {
            read_run_length(input, total - filled).await?
        } else {
            1
        };
        copy_indexed(palette, &mut pixels, bpp, index, run)?;
        filled += run;
    }
    Ok(pixels)
}

fn repeat(color: &[u8], count: usize) -> Vec<u8> {
    let mut pixels = Vec::with_capacity(count * color.len());
    for _ in 0..count {
        pixels.extend_from_slice(color);
    }
    pixels
}

pub struct Decoder {}

impl Decoder {
    pub fn new() -> Self {
        Self {}
    }

    pub async fn decode<S, F, Fut>(
        &mut self,
        format: &PixelFormat,
        rect: &Rect,
        input: &mut S,
        output_func: &F,
    ) -> Result<(), VncError>
    where
        S: AsyncRead + Unpin,
        F: Fn(VncEvent) -> Fut,
        Fut: Future<Output = Result<(), VncError>>,
    {
        crate::limits::dimensions(rect.width, rect.height)?;
        format.validate()?;

        let bpp = format.bits_per_pixel as usize / 8;
        let pixel_mask = if format.true_color_flag != 0 {
            ((format.red_max as u32) << format.red_shift)
                | ((format.green_max as u32) << format.green_shift)
                | ((format.blue_max as u32) << format.blue_shift)
        } else {
            0
        };

        let (compressed_bpp, alpha_at_first) =
            if format.bits_per_pixel == 32 && format.true_color_flag > 0 && format.depth <= 24 {
                if pixel_mask & 0x0000_00ff == 0 {
                    (3, format.big_endian_flag == 0)
                } else if pixel_mask & 0xff00_0000 == 0 {
                    (3, format.big_endian_flag > 0)
                } else {
                    (4, false)
                }
            } else {
                (bpp, false)
            };

        let mut state = TileState::default();

        let mut y = 0;
        while y < rect.height {
            let height = TILE.min(rect.height - y);
            let mut x = 0;
            while x < rect.width {
                let width = TILE.min(rect.width - x);
                let subencoding = input.read_u8().await?;

                let pixels = match subencoding {
                    // A raw tile leaves the remembered mode alone, which the
                    // reuse subencoding below depends upon.
                    0 => {
                        read_raw_tile(input, width, height, compressed_bpp, bpp, alpha_at_first)
                            .await?
                    }
                    1 => {
                        state.fill_color =
                            read_cpixel(input, compressed_bpp, bpp, alpha_at_first).await?;
                        state.last_type = 1;
                        repeat(&state.fill_color, usize::from(width) * usize::from(height))
                    }
                    2..=16 => {
                        state.palette = read_palette(
                            input,
                            usize::from(subencoding),
                            compressed_bpp,
                            bpp,
                            alpha_at_first,
                        )
                        .await?;
                        state.last_type = subencoding;
                        let bits = bits_per_index(subencoding)?;
                        read_indexed_tile(input, width, height, &state.palette, bpp, bits).await?
                    }
                    127 => {
                        // Reuse the previous tile. A remembered run mode of 130
                        // and up is the packed palette mode whose colour count
                        // sits in the low seven bits.
                        let remembered = if state.last_type >= 130 {
                            state.last_type & 0x7f
                        } else {
                            state.last_type
                        };
                        match remembered {
                            1 => {
                                repeat(&state.fill_color, usize::from(width) * usize::from(height))
                            }
                            2..=16 => {
                                let bits = bits_per_index(remembered)?;
                                read_indexed_tile(input, width, height, &state.palette, bpp, bits)
                                    .await?
                            }
                            _ => return Err(VncError::InvalidImageData),
                        }
                    }
                    128 => {
                        read_plain_rle_tile(
                            input,
                            width,
                            height,
                            compressed_bpp,
                            bpp,
                            alpha_at_first,
                        )
                        .await?
                    }
                    129 => {
                        if state.palette.is_empty() {
                            return Err(VncError::InvalidImageData);
                        }
                        read_palette_rle_tile(input, width, height, &state.palette, bpp).await?
                    }
                    130..=255 => {
                        let entries = usize::from(subencoding - 128);
                        if entries > MAX_PALETTE {
                            return Err(VncError::InvalidImageData);
                        }
                        state.palette =
                            read_palette(input, entries, compressed_bpp, bpp, alpha_at_first)
                                .await?;
                        state.last_type = subencoding;
                        read_palette_rle_tile(input, width, height, &state.palette, bpp).await?
                    }
                    _ => return Err(VncError::InvalidImageData),
                };

                output_func(VncEvent::RawImage(
                    Rect {
                        x: rect.x + x,
                        y: rect.y + y,
                        width,
                        height,
                    },
                    pixels,
                ))
                .await?;
                x += width;
            }
            y += height;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const RED: [u8; 3] = [1, 2, 3];
    const GREEN: [u8; 3] = [4, 5, 6];

    fn rgb(pixel: [u8; 3]) -> [u8; 4] {
        [pixel[0], pixel[1], pixel[2], 255]
    }

    async fn decode(bytes: &[u8], width: u16, height: u16) -> (Result<(), VncError>, Vec<u8>) {
        let pixels = Mutex::new(Vec::new());
        let mut input = bytes;
        let result = Decoder::new()
            .decode(
                &PixelFormat::rgba(),
                &Rect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                &mut input,
                &|event| {
                    if let VncEvent::RawImage(_, data) = event {
                        pixels.lock().unwrap().extend_from_slice(&data);
                    }
                    async { Ok(()) }
                },
            )
            .await;
        (result, pixels.into_inner().unwrap())
    }

    #[tokio::test]
    async fn raw_tile_expands_cpixels() {
        let (result, pixels) = decode(&[0, 1, 2, 3, 4, 5, 6], 2, 1).await;
        assert!(result.is_ok());
        assert_eq!(pixels, [rgb(RED), rgb(GREEN)].concat());
    }

    #[tokio::test]
    async fn fill_tile_is_reused_by_the_next_tile() {
        // 32 pixels wide means two tiles, so the second one carries only 127.
        let (result, pixels) = decode(&[1, 1, 2, 3, 127], 32, 1).await;
        assert!(result.is_ok());
        assert_eq!(pixels, repeat(&rgb(RED), 32));
    }

    #[tokio::test]
    async fn palette_tiles_pack_indices_from_the_high_bits() {
        // Two colours use one bit per pixel.
        let (result, pixels) = decode(&[2, 1, 2, 3, 4, 5, 6, 0b1000_0000], 2, 1).await;
        assert!(result.is_ok());
        assert_eq!(pixels, [rgb(GREEN), rgb(RED)].concat());

        // Four colours use two bits, and a three pixel row still occupies one
        // byte: the unused low bits are dropped, not carried to the next row.
        let four = [4, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 0b0111_0000];
        let (result, pixels) = decode(&four, 3, 1).await;
        assert!(result.is_ok());
        assert_eq!(pixels, [rgb(GREEN), rgb([10, 11, 12]), rgb(RED)].concat());

        // Sixteen colours use four bits, one byte per two pixels. Palette entry
        // i holds [i, i + 1, i + 2], so 0x21 then 0x30 means indices 2, 1, 3.
        let mut sixteen = vec![16];
        for entry in 0..16_u8 {
            sixteen.extend([entry, entry + 1, entry + 2]);
        }
        sixteen.extend([0x21, 0x30]);
        let (result, pixels) = decode(&sixteen, 3, 1).await;
        assert!(result.is_ok());
        assert_eq!(pixels, [rgb([2, 3, 4]), rgb(RED), rgb([3, 4, 5])].concat());
    }

    #[tokio::test]
    async fn reuse_keeps_the_palette_of_an_earlier_tile() {
        // First tile: sixteen pixels of palette entry 1, which is two index
        // bytes. Second tile: 127 with one bit per pixel, index 1 then fifteen
        // times index 0.
        let stream = [2, 1, 2, 3, 4, 5, 6, 0xff, 0xff, 127, 0x80, 0x00];
        let (result, pixels) = decode(&stream, 32, 1).await;
        assert!(result.is_ok());
        let mut expected = repeat(&rgb(GREEN), 16);
        expected.extend(repeat(&rgb(GREEN), 1));
        expected.extend(repeat(&rgb(RED), 15));
        assert_eq!(pixels, expected);
    }

    #[tokio::test]
    async fn plain_runs_fill_the_tile() {
        // The length byte counts beyond the first pixel, so 2 means three.
        let (result, pixels) = decode(&[128, 1, 2, 3, 2], 3, 1).await;
        assert!(result.is_ok());
        assert_eq!(pixels, repeat(&rgb(RED), 3));

        // A 0xff length byte continues the run: 1 + 255 + 0 covers a whole tile.
        let (result, pixels) = decode(&[128, 1, 2, 3, 0xff, 0x00], 16, 16).await;
        assert!(result.is_ok());
        assert_eq!(pixels.len(), usize::from(TILE) * usize::from(TILE) * 4);
    }

    #[tokio::test]
    async fn palette_runs_then_reuse_as_packed_indices() {
        // 130 carries a two colour palette, then one run of sixteen pixels.
        let (result, pixels) = decode(&[130, 1, 2, 3, 4, 5, 6, 0x81, 15], 16, 1).await;
        assert!(result.is_ok());
        assert_eq!(pixels, repeat(&rgb(GREEN), 16));

        // 127 after that run mode reads one bit per pixel from the same
        // palette, two index bytes for the sixteen pixels.
        let stream = [130, 1, 2, 3, 4, 5, 6, 0x81, 15, 127, 0x80, 0x00];
        let (result, pixels) = decode(&stream, 32, 1).await;
        assert!(result.is_ok());
        let mut expected = repeat(&rgb(GREEN), 16);
        expected.extend(repeat(&rgb(GREEN), 1));
        expected.extend(repeat(&rgb(RED), 15));
        assert_eq!(pixels, expected);
    }

    #[tokio::test]
    async fn non_stateful_tiles_preserve_reuse_state() {
        let mut raw_after_fill = vec![1];
        raw_after_fill.extend(RED);
        raw_after_fill.push(0);
        for _ in 0..16 {
            raw_after_fill.extend([7, 8, 9]);
        }
        raw_after_fill.push(127);
        let (result, pixels) = decode(&raw_after_fill, 48, 1).await;
        assert!(result.is_ok());
        let mut expected = repeat(&rgb(RED), 16);
        expected.extend(repeat(&rgb([7, 8, 9]), 16));
        expected.extend(repeat(&rgb(RED), 16));
        assert_eq!(pixels, expected);

        let mut plain_rle_after_palette = vec![2];
        plain_rle_after_palette.extend(RED);
        plain_rle_after_palette.extend(GREEN);
        plain_rle_after_palette.extend([0xff, 0xff]);
        plain_rle_after_palette.extend([128, 7, 8, 9, 15, 127, 0x80, 0x00]);
        let (result, pixels) = decode(&plain_rle_after_palette, 48, 1).await;
        assert!(result.is_ok());
        let mut expected = repeat(&rgb(GREEN), 16);
        expected.extend(repeat(&rgb([7, 8, 9]), 16));
        expected.extend(repeat(&rgb(GREEN), 1));
        expected.extend(repeat(&rgb(RED), 15));
        assert_eq!(pixels, expected);

        let mut palette_rle_after_palette = vec![2];
        palette_rle_after_palette.extend(RED);
        palette_rle_after_palette.extend(GREEN);
        palette_rle_after_palette.extend([0xff, 0xff]);
        palette_rle_after_palette.extend([129, 0x81, 15, 127, 0x80, 0x00]);
        let (result, pixels) = decode(&palette_rle_after_palette, 48, 1).await;
        assert!(result.is_ok());
        let mut expected = repeat(&rgb(GREEN), 32);
        expected.extend(repeat(&rgb(GREEN), 1));
        expected.extend(repeat(&rgb(RED), 15));
        assert_eq!(pixels, expected);
    }

    #[tokio::test]
    async fn malformed_subencodings_and_runs_return_errors() {
        for bytes in [
            vec![17_u8],                       // unused value
            vec![127],                         // nothing remembered yet
            vec![129, 0],                      // run mode without a palette
            vec![128, 1, 2, 3, 200],           // run longer than the tile
            vec![130, 1, 2, 3, 4, 5, 6, 0x7f], // palette index out of range
            vec![1],                           // truncated colour
            vec![2, 1, 2, 3],                  // palette cut short
        ] {
            let (result, pixels) = decode(&bytes, 2, 1).await;
            assert!(result.is_err(), "accepted {:?}", bytes);
            assert!(pixels.is_empty(), "rendered pixels for {:?}", bytes);
        }
    }

    #[tokio::test]
    async fn remembered_state_does_not_cross_rectangles() {
        let mut decoder = Decoder::new();
        let mut first = [1_u8, 1, 2, 3].as_slice();
        decoder
            .decode(
                &PixelFormat::rgba(),
                &Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                &mut first,
                &|_| async { Ok(()) },
            )
            .await
            .unwrap();

        // A new rectangle starts with nothing remembered, so 127 is an error.
        let mut second = [127_u8].as_slice();
        let result = decoder
            .decode(
                &PixelFormat::rgba(),
                &Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                &mut second,
                &|_| async { Ok(()) },
            )
            .await;
        assert!(matches!(result, Err(VncError::InvalidImageData)));
    }
}
