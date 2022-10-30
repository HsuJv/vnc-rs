use crate::PixelFormat;

type ImageData = Vec<u8>;

#[derive(Debug, Clone, Copy)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

#[derive(Debug, Clone)]
pub struct Screen {
    pub width: u16,
    pub height: u16,
}

impl From<(u16, u16)> for Screen {
    fn from(tuple: (u16, u16)) -> Self {
        Self {
            width: tuple.0,
            height: tuple.1,
        }
    }
}

type SrcRect = Rect;
type DstRect = Rect;

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum VncEvent {
    SetResulotin(Screen),
    SetPixelFormat(PixelFormat),
    RawImage(Rect, ImageData),
    Copy(DstRect, SrcRect),
    JpegImage(Rect, ImageData),
    // PngImage(Rect, ImageData),
    Bell,
    Text(String),
}

#[derive(Debug, Clone)]
pub struct ClientKeyEvent {
    pub keycode: u32,
    pub down: bool,
}

impl From<(u32, bool)> for ClientKeyEvent {
    fn from(tuple: (u32, bool)) -> Self {
        Self {
            keycode: tuple.0,
            down: tuple.1,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClientMouseEvent {
    pub position_x: u16,
    pub position_y: u16,
    pub bottons: u8,
}

impl From<(u16, u16, u8)> for ClientMouseEvent {
    fn from(tuple: (u16, u16, u8)) -> Self {
        Self {
            position_x: tuple.0,
            position_y: tuple.1,
            bottons: tuple.2,
        }
    }
}

#[non_exhaustive]
#[derive(Debug, Clone)]
pub enum X11Event {
    Refresh,
    KeyEvent(ClientKeyEvent),
    PointerEvent(ClientMouseEvent),
    CopyText(String),
}
