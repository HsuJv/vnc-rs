mod raw;
mod zlib;
mod zrle;
pub(crate) use raw::Decoder as RawDecoder;
pub(crate) use zrle::Decoder as ZrleDecoder;

pub(self) fn uninit_vec(len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    #[allow(clippy::uninit_vec)]
    unsafe {
        v.set_len(len)
    };
    v
}
