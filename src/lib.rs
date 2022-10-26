pub mod client;
mod decoder;
pub mod error;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i32)]
pub enum VncEncoding {
    Raw = 0,
    CopyRect = 1,
    Rre = 2,
    Hextile = 5,
    Tight = 7,
    Trle = 15,
    Zrle = 16,
    CursorPseudo = -239,
    DesktopSizePseudo = -223,
}

impl From<u32> for VncEncoding {
    fn from(num: u32) -> Self {
        unsafe { std::mem::transmute(num) }
    }
}

impl From<VncEncoding> for u32 {
    fn from(e: VncEncoding) -> Self {
        e as u32
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Eq)]
#[repr(u8)]
pub enum VncVersion {
    RFB33,
    RFB37,
    RFB38,
}

impl From<[u8; 12]> for VncVersion {
    fn from(version: [u8; 12]) -> Self {
        match &version {
            b"RFB 003.003\n" => VncVersion::RFB33,
            b"RFB 003.007\n" => VncVersion::RFB37,
            b"RFB 003.008\n" => VncVersion::RFB38,
            // https://www.rfc-editor.org/rfc/rfc6143#section-7.1.1
            //  Other version numbers are reported by some servers and clients,
            //  but should be interpreted as 3.3 since they do not implement the
            //  different handshake in 3.7 or 3.8.
            _ => VncVersion::RFB33,
        }
    }
}

impl From<VncVersion> for &[u8; 12] {
    fn from(version: VncVersion) -> Self {
        match version {
            VncVersion::RFB33 => b"RFB 003.003\n",
            VncVersion::RFB37 => b"RFB 003.007\n",
            VncVersion::RFB38 => b"RFB 003.008\n",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[should_panic]
    #[test]
    fn test() {}
}
