#[cfg(feature = "cursor")]
mod cursor;
mod decoders;
mod raw;
#[cfg(feature = "encoding-tight")]
mod tight;
#[cfg(feature = "encoding-trle")]
mod trle;
#[cfg(any(feature = "encoding-tight", feature = "encoding-zrle"))]
mod zlib;
#[cfg(feature = "encoding-zrle")]
mod zrle;
pub(crate) use decoders::Decoders;

fn initialized_vec(len: usize) -> Vec<u8> {
    vec![0; len]
}

#[cfg(all(test, any(feature = "encoding-tight", feature = "cursor")))]
mod tests;
