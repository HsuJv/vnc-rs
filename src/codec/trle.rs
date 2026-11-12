use crate::{PixelFormat, Rect, VncError, VncEvent};
use std::future::Future;
use tokio::io::AsyncRead;

pub struct Decoder {}

impl Decoder {
    pub fn new() -> Self {
        Self {}
    }

    /// TRLE is a known wire encoding, but tile data is refused on purpose: the
    /// previous implementation reused the ZRLE loop with 64x64 tiles and a
    /// leading 4-byte zlib length, while RFC 6143 section 7.7.5 defines 16x16
    /// tiles and no length header at all, so it could only ever emit wrong
    /// pixels and report nothing.
    pub async fn decode<S, F, Fut>(
        &mut self,
        _format: &PixelFormat,
        _rect: &Rect,
        _input: &mut S,
        _output_func: &F,
    ) -> Result<(), VncError>
    where
        S: AsyncRead + Unpin,
        F: Fn(VncEvent) -> Fut,
        Fut: Future<Output = Result<(), VncError>>,
    {
        Err(VncError::UnsupportedEncoding(
            crate::VncEncoding::Trle as i32,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[tokio::test]
    async fn trle_tiles_are_refused_instead_of_rendering_pixels() {
        let rect = Rect {
            x: 0,
            y: 0,
            width: 16,
            height: 16,
        };
        let output = Mutex::new(Vec::new());
        let data = vec![0_u8; 512];
        let result = Decoder::new()
            .decode(
                &PixelFormat::rgba(),
                &rect,
                &mut data.as_slice(),
                &|event| {
                    output.lock().unwrap().push(event);
                    async { Ok(()) }
                },
            )
            .await;

        assert!(matches!(result, Err(VncError::UnsupportedEncoding(15))));
        assert!(output.lock().unwrap().is_empty());
    }
}
