#![no_main]

use libfuzzer_sys::fuzz_target;
use std::{
    io,
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use vnc::{PixelFormat, VncConnector, VncEncoding};

struct Wire {
    bytes: Vec<u8>,
    position: usize,
}

impl AsyncRead for Wire {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        output: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let count = output.remaining().min(self.bytes.len() - self.position);
        output.put_slice(&self.bytes[self.position..self.position + count]);
        self.position += count;
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Wire {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Ok(bytes.len()))
    }
    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn greeting() -> Vec<u8> {
    let mut bytes = b"RFB 003.008\n\x01\x01\x00\x00\x00\x00".to_vec();
    bytes.extend([0, 32, 0, 32]);
    bytes.extend(Vec::<u8>::from(PixelFormat::rgba()));
    bytes.extend([0; 4]);
    bytes
}

fn wire(data: &[u8]) -> Vec<u8> {
    if data[0].is_multiple_of(3) {
        return data[1..].to_vec();
    }
    let mut bytes = greeting();
    if data[0] % 3 == 1 {
        bytes.extend(&data[1..]);
    } else if data.len() >= 4 {
        let encoding = [0_u32, 7, 15, 16, (-239_i32) as u32][usize::from(data[1] % 5)];
        bytes.extend([
            0,
            0,
            0,
            1,
            0,
            0,
            0,
            0,
            0,
            1 + data[2] % 16,
            0,
            1 + data[3] % 16,
        ]);
        bytes.extend(encoding.to_be_bytes());
        if encoding == 16 {
            // Mutate decoded tile data too, rather than waiting for random valid zlib streams.
            let mut compressed = Vec::with_capacity(data.len() * 2 + 128);
            flate2::Compress::new(flate2::Compression::fast(), true)
                .compress_vec(&data[4..], &mut compressed, flate2::FlushCompress::Sync)
                .unwrap();
            bytes.extend((compressed.len() as u32).to_be_bytes());
            bytes.extend(compressed);
        } else {
            bytes.extend(&data[4..]);
        }
    }
    bytes
}

fuzz_target!(|data: &[u8]| {
    if data.is_empty() || data.len() > 65536 {
        return;
    }
    if let Ok(format) = <[u8; 16]>::try_from(data.get(1..17).unwrap_or_default()) {
        let _ = PixelFormat::try_from(format);
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let operation = async {
            let mut connector = VncConnector::new(Wire {
                bytes: wire(data),
                position: 0,
            })
            .set_auth_method(async { Ok("test".into()) });
            for encoding in [
                VncEncoding::Raw,
                VncEncoding::CopyRect,
                VncEncoding::Tight,
                VncEncoding::Trle,
                VncEncoding::Zrle,
                VncEncoding::CursorPseudo,
                VncEncoding::DesktopSizePseudo,
                VncEncoding::LastRectPseudo,
            ] {
                connector = connector.add_encoding(encoding);
            }
            if let Ok(state) = connector.build().unwrap().try_start().await {
                let client = state.finish().unwrap();
                let _ = client.input(vnc::X11Event::Refresh).await;
                for _ in 0..64 {
                    if client.recv_event().await.is_err() {
                        break;
                    }
                }
                let _ = client.close().await;
            }
        };
        let _ = tokio::time::timeout(Duration::from_millis(100), operation).await;
    });
});
