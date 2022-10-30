mod raw;
mod tight;
mod zlib;
mod zrle;
mod cursor;
pub(crate) use raw::Decoder as RawDecoder;
pub(crate) use tight::Decoder as TightDecoder;
pub(crate) use zrle::Decoder as ZrleDecoder;
pub(crate) use cursor::Decoder as CursorDecoder;

pub(self) fn uninit_vec(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    #[allow(clippy::uninit_vec)]
    unsafe {
        v.set_len(len)
    };
    v
}
