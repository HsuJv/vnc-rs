#[cfg(feature = "cursor")]
mod cursor;
mod raw;
#[cfg(feature = "encoding-tight")]
mod tight;
#[cfg(feature = "encoding-trle")]
mod trle;
#[cfg(any(feature = "encoding-tight", feature = "encoding-zrle"))]
mod zlib;
#[cfg(feature = "encoding-zrle")]
mod zrle;
#[cfg(feature = "cursor")]
pub(crate) use cursor::Decoder as CursorDecoder;
pub(crate) use raw::Decoder as RawDecoder;
#[cfg(feature = "encoding-tight")]
pub(crate) use tight::Decoder as TightDecoder;
#[cfg(feature = "encoding-trle")]
pub(crate) use trle::Decoder as TrleDecoder;
#[cfg(feature = "encoding-zrle")]
pub(crate) use zrle::Decoder as ZrleDecoder;

fn initialized_vec(len: usize) -> Vec<u8> {
    vec![0; len]
}

#[cfg(all(test, any(feature = "encoding-tight", feature = "cursor")))]
mod tests;
