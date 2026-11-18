//! Per-connection decoder state and static rectangle dispatch.

use crate::client::connection::{pack_screen, unpack_screen};
use crate::client::desktop::Desktop;
use crate::{PixelFormat, Rect, VncEncoding, VncError, VncEvent};
use std::{
    future::Future,
    sync::atomic::{AtomicU32, Ordering},
};
use tokio::io::{AsyncRead, AsyncReadExt};

struct ImageRect {
    rect: Rect,
    encoding: VncEncoding,
}

impl TryFrom<[u8; 12]> for ImageRect {
    type Error = VncError;

    fn try_from(buf: [u8; 12]) -> Result<Self, VncError> {
        Ok(Self {
            rect: Rect {
                x: ((buf[0] as u16) << 8) | buf[1] as u16,
                y: ((buf[2] as u16) << 8) | buf[3] as u16,
                width: ((buf[4] as u16) << 8) | buf[5] as u16,
                height: ((buf[6] as u16) << 8) | buf[7] as u16,
            },
            encoding: VncEncoding::from_wire(
                ((buf[8] as u32) << 24)
                    | ((buf[9] as u32) << 16)
                    | ((buf[10] as u32) << 8)
                    | (buf[11] as u32),
            )?,
        })
    }
}

impl ImageRect {
    async fn read<S>(reader: &mut S) -> Result<Self, VncError>
    where
        S: AsyncRead + Unpin,
    {
        let mut rect_buf = [0_u8; 12];
        reader.read_exact(&mut rect_buf).await?;
        rect_buf.try_into()
    }
}

pub(crate) struct Decoders<'a> {
    raw: super::raw::Decoder,
    #[cfg(feature = "encoding-tight")]
    tight: super::tight::Decoder,
    #[cfg(feature = "encoding-zrle")]
    zrle: super::zrle::Decoder,
    #[cfg(feature = "encoding-trle")]
    trle: super::trle::Decoder,
    #[cfg(feature = "cursor")]
    cursor: super::cursor::Decoder,
    screen: (u16, u16),
    shared_screen: &'a AtomicU32,
    #[cfg(feature = "desktop-resize")]
    desktop: &'a Desktop,
    // Keep only a zero-sized value when desktop coordination is disabled.
    #[cfg(not(feature = "desktop-resize"))]
    desktop: Desktop,
}

impl<'a> Decoders<'a> {
    pub(crate) fn new(shared_screen: &'a AtomicU32, desktop: &'a Desktop) -> Self {
        Self {
            raw: super::raw::Decoder::new(),
            #[cfg(feature = "encoding-tight")]
            tight: super::tight::Decoder::new(),
            #[cfg(feature = "encoding-zrle")]
            zrle: super::zrle::Decoder::new(),
            #[cfg(feature = "encoding-trle")]
            trle: super::trle::Decoder::new(),
            #[cfg(feature = "cursor")]
            cursor: super::cursor::Decoder::new(),
            screen: unpack_screen(shared_screen.load(Ordering::Acquire)),
            shared_screen,
            #[cfg(feature = "desktop-resize")]
            desktop,
            #[cfg(not(feature = "desktop-resize"))]
            desktop: desktop.clone(),
        }
    }

    pub(crate) async fn update<S, F, Fut>(
        &mut self,
        rect_num: u16,
        pf: &PixelFormat,
        stream: &mut S,
        output_func: &F,
        encodings: &[VncEncoding],
    ) -> Result<(), VncError>
    where
        S: AsyncRead + Unpin,
        F: Fn(VncEvent) -> Fut,
        Fut: Future<Output = Result<(), VncError>>,
    {
        let mut batch = self.desktop.batch();
        for _ in 0..rect_num {
            let rect = ImageRect::read(stream).await?;
            if rect.encoding != VncEncoding::Raw && !encodings.contains(&rect.encoding) {
                return Err(VncError::InvalidImageData);
            }
            let needs_bounds = match rect.encoding {
                VncEncoding::DesktopSizePseudo | VncEncoding::LastRectPseudo => false,
                #[cfg(feature = "desktop-resize")]
                VncEncoding::ExtendedDesktopSizePseudo => false,
                #[cfg(feature = "cursor")]
                VncEncoding::CursorPseudo => false,
                _ => true,
            };
            if needs_bounds {
                crate::limits::rectangle(&rect.rect, self.screen)?;
            }
            let changes_framebuffer = match rect.encoding {
                #[cfg(feature = "desktop-resize")]
                VncEncoding::ExtendedDesktopSizePseudo => false,
                VncEncoding::LastRectPseudo => false,
                #[cfg(feature = "cursor")]
                VncEncoding::CursorPseudo => false,
                _ => true,
            };
            if changes_framebuffer {
                batch.before_pixels()?;
            }

            match rect.encoding {
                VncEncoding::Raw => {
                    self.raw.decode(pf, &rect.rect, stream, output_func).await?;
                }
                VncEncoding::CopyRect => {
                    let source_x = stream.read_u16().await?;
                    let source_y = stream.read_u16().await?;
                    let mut src_rect = rect.rect;
                    src_rect.x = source_x;
                    src_rect.y = source_y;
                    crate::limits::rectangle(&src_rect, self.screen)?;
                    output_func(VncEvent::Copy(rect.rect, src_rect)).await?;
                }
                #[cfg(feature = "encoding-tight")]
                VncEncoding::Tight => {
                    self.tight
                        .decode(pf, &rect.rect, stream, output_func)
                        .await?;
                }
                #[cfg(feature = "encoding-trle")]
                VncEncoding::Trle => {
                    self.trle
                        .decode(pf, &rect.rect, stream, output_func)
                        .await?;
                }
                #[cfg(feature = "encoding-zrle")]
                VncEncoding::Zrle => {
                    self.zrle
                        .decode(pf, &rect.rect, stream, output_func)
                        .await?;
                }
                #[cfg(feature = "cursor")]
                VncEncoding::CursorPseudo => {
                    self.cursor
                        .decode(pf, &rect.rect, stream, output_func)
                        .await?;
                }
                #[cfg(feature = "desktop-resize")]
                VncEncoding::ExtendedDesktopSizePseudo => {
                    let update = crate::DesktopUpdate::read(stream, rect.rect).await?;
                    batch
                        .extended(update, &mut self.screen, self.shared_screen, output_func)
                        .await?;
                }

                VncEncoding::DesktopSizePseudo => {
                    crate::limits::dimensions(rect.rect.width, rect.rect.height)?;
                    if rect.rect.x != 0 || rect.rect.y != 0 {
                        return Err(VncError::InvalidImageData);
                    }
                    self.screen = (rect.rect.width, rect.rect.height);
                    self.shared_screen
                        .store(pack_screen(self.screen), Ordering::Release);
                    self.desktop.legacy_resize();
                    output_func(VncEvent::SetResolution(
                        (rect.rect.width, rect.rect.height).into(),
                    ))
                    .await?;
                }
                VncEncoding::LastRectPseudo => {
                    break;
                }
            }
        }
        batch
            .finish(&mut self.screen, self.shared_screen, output_func)
            .await?;
        Ok(())
    }
}
