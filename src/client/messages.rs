use crate::{PixelFormat, Rect, ScreenLayout, VncEncoding, VncError};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

#[derive(Debug)]
pub(super) enum ClientMsg {
    SetPixelFormat(PixelFormat),
    SetEncodings(Vec<VncEncoding>),
    FramebufferUpdateRequest(Rect, u8),
    KeyEvent(u32, bool),
    PointerEvent(u16, u16, u8),
    ClientCutText(String),
    SetDesktopSize(u16, u16, Vec<ScreenLayout>),
}

impl ClientMsg {
    pub(super) async fn write<S>(self, writer: &mut S) -> Result<(), VncError>
    where
        S: AsyncWrite + Unpin,
    {
        match self {
            ClientMsg::SetPixelFormat(pf) => {
                // +--------------+--------------+--------------+
                // | No. of bytes | Type [Value] | Description  |
                // +--------------+--------------+--------------+
                // | 1            | U8 [0]       | message-type |
                // | 3            |              | padding      |
                // | 16           | PIXEL_FORMAT | pixel-format |
                // +--------------+--------------+--------------+
                let mut payload = vec![0_u8, 0, 0, 0];
                payload.extend(<PixelFormat as Into<Vec<u8>>>::into(pf));
                writer.write_all(&payload).await?;
                Ok(())
            }
            ClientMsg::SetEncodings(encodings) => {
                //  +--------------+--------------+---------------------+
                // | No. of bytes | Type [Value] | Description         |
                // +--------------+--------------+---------------------+
                // | 1            | U8 [2]       | message-type        |
                // | 1            |              | padding             |
                // | 2            | U16          | number-of-encodings |
                // +--------------+--------------+---------------------+

                // This is followed by number-of-encodings repetitions of the following:
                // +--------------+--------------+---------------+
                // | No. of bytes | Type [Value] | Description   |
                // +--------------+--------------+---------------+
                // | 4            | S32          | encoding-type |
                // +--------------+--------------+---------------+
                let mut payload = vec![2, 0];
                payload.extend_from_slice(&(encodings.len() as u16).to_be_bytes());
                for e in encodings {
                    payload.write_u32(e.into()).await?;
                }
                writer.write_all(&payload).await?;
                Ok(())
            }
            ClientMsg::FramebufferUpdateRequest(rect, incremental) => {
                // +--------------+--------------+--------------+
                // | No. of bytes | Type [Value] | Description  |
                // +--------------+--------------+--------------+
                // | 1            | U8 [3]       | message-type |
                // | 1            | U8           | incremental  |
                // | 2            | U16          | x-position   |
                // | 2            | U16          | y-position   |
                // | 2            | U16          | width        |
                // | 2            | U16          | height       |
                // +--------------+--------------+--------------+
                let mut payload = vec![3, incremental];
                payload.extend_from_slice(&rect.x.to_be_bytes());
                payload.extend_from_slice(&rect.y.to_be_bytes());
                payload.extend_from_slice(&rect.width.to_be_bytes());
                payload.extend_from_slice(&rect.height.to_be_bytes());
                writer.write_all(&payload).await?;
                Ok(())
            }
            ClientMsg::KeyEvent(keycode, down) => {
                // +--------------+--------------+--------------+
                // | No. of bytes | Type [Value] | Description  |
                // +--------------+--------------+--------------+
                // | 1            | U8 [4]       | message-type |
                // | 1            | U8           | down-flag    |
                // | 2            |              | padding      |
                // | 4            | U32          | key          |
                // +--------------+--------------+--------------+
                let mut payload = vec![4, down as u8, 0, 0];
                payload.write_u32(keycode).await?;
                writer.write_all(&payload).await?;
                Ok(())
            }
            ClientMsg::PointerEvent(x, y, mask) => {
                // +--------------+--------------+--------------+
                // | No. of bytes | Type [Value] | Description  |
                // +--------------+--------------+--------------+
                // | 1            | U8 [5]       | message-type |
                // | 1            | U8           | button-mask  |
                // | 2            | U16          | x-position   |
                // | 2            | U16          | y-position   |
                // +--------------+--------------+--------------+
                let mut payload = vec![5, mask];
                payload.write_u16(x).await?;
                payload.write_u16(y).await?;
                writer.write_all(&payload).await?;
                Ok(())
            }
            ClientMsg::SetDesktopSize(width, height, screens) => {
                // +--------------+--------------+-------------------+
                // | No. of bytes | Type [Value] | Description       |
                // +--------------+--------------+-------------------+
                // | 1            | U8 [251]     | message-type      |
                // | 1            |              | padding           |
                // | 2            | U16          | width             |
                // | 2            | U16          | height            |
                // | 1            | U8           | number-of-screens |
                // | 1            |              | padding           |
                // +--------------+--------------+-------------------+
                // Followed by number-of-screens SCREEN structures:
                // +--------------+--------------+--------------+
                // | 4            | U32          | id           |
                // | 2            | U16          | x-position   |
                // | 2            | U16          | y-position   |
                // | 2            | U16          | width        |
                // | 2            | U16          | height       |
                // | 4            | U32          | flags        |
                // +--------------+--------------+--------------+
                let mut payload = vec![251_u8, 0];
                payload.extend_from_slice(&width.to_be_bytes());
                payload.extend_from_slice(&height.to_be_bytes());
                payload.push(screens.len() as u8);
                payload.push(0);
                for s in screens {
                    payload.extend_from_slice(&s.id.to_be_bytes());
                    payload.extend_from_slice(&s.x.to_be_bytes());
                    payload.extend_from_slice(&s.y.to_be_bytes());
                    payload.extend_from_slice(&s.width.to_be_bytes());
                    payload.extend_from_slice(&s.height.to_be_bytes());
                    payload.extend_from_slice(&s.flags.to_be_bytes());
                }
                writer.write_all(&payload).await?;
                Ok(())
            }
            ClientMsg::ClientCutText(s) => {
                //   +--------------+--------------+--------------+
                //   | No. of bytes | Type [Value] | Description  |
                //   +--------------+--------------+--------------+
                //   | 1            | U8 [6]       | message-type |
                //   | 3            |              | padding      |
                //   | 4            | U32          | length       |
                //   | length       | U8 array     | text         |
                //   +--------------+--------------+--------------+
                let mut payload = vec![6_u8, 0, 0, 0];
                payload.write_u32(s.len() as u32).await?;
                payload.write_all(s.as_bytes()).await?;
                writer.write_all(&payload).await?;
                Ok(())
            }
        }
    }
}

#[derive(Debug)]
pub(super) enum ServerMsg {
    FramebufferUpdate(u16),
    // SetColorMapEntries,
    Bell,
    ServerCutText(String),
}

impl ServerMsg {
    pub(super) async fn read<S>(reader: &mut S) -> Result<Self, VncError>
    where
        S: AsyncRead + Unpin,
    {
        let server_msg = reader.read_u8().await?;

        match server_msg {
            0 => {
                // FramebufferUpdate
                //   +--------------+--------------+----------------------+
                //   | No. of bytes | Type [Value] | Description          |
                //   +--------------+--------------+----------------------+
                //   | 1            | U8 [0]       | message-type         |
                //   | 1            |              | padding              |
                //   | 2            | U16          | number-of-rectangles |
                //   +--------------+--------------+----------------------+
                let _padding = reader.read_u8().await?;
                let rects = reader.read_u16().await?;
                Ok(ServerMsg::FramebufferUpdate(rects))
            }
            1 => {
                // SetColorMapEntries
                // +--------------+--------------+------------------+
                // | No. of bytes | Type [Value] | Description      |
                // +--------------+--------------+------------------+
                // | 1            | U8 [1]       | message-type     |
                // | 1            |              | padding          |
                // | 2            | U16          | first-color      |
                // | 2            | U16          | number-of-colors |
                // +--------------+--------------+------------------+
                unimplemented!()
            }
            2 => {
                // Bell
                //   +--------------+--------------+--------------+
                //   | No. of bytes | Type [Value] | Description  |
                //   +--------------+--------------+--------------+
                //   | 1            | U8 [2]       | message-type |
                //   +--------------+--------------+--------------+
                Ok(ServerMsg::Bell)
            }
            3 => {
                // ServerCutText
                // +--------------+--------------+--------------+
                // | No. of bytes | Type [Value] | Description  |
                // +--------------+--------------+--------------+
                // | 1            | U8 [3]       | message-type |
                // | 3            |              | padding      |
                // | 4            | U32          | length       |
                // | length       | U8 array     | text         |
                // +--------------+--------------+--------------+
                let mut padding = [0; 3];
                reader.read_exact(&mut padding).await?;
                let len = reader.read_u32().await?;
                let mut buffer_str = vec![0; len as usize];
                reader.read_exact(&mut buffer_str).await?;
                Ok(Self::ServerCutText(
                    String::from_utf8_lossy(&buffer_str).to_string(),
                ))
            }
            _ => Err(VncError::WrongServerMessage),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ScreenLayout;

    async fn serialize(msg: ClientMsg) -> Vec<u8> {
        let mut buf = Vec::new();
        msg.write(&mut buf).await.unwrap();
        buf
    }

    #[tokio::test]
    async fn set_desktop_size_wire_format() {
        let screens = vec![ScreenLayout {
            id: 0x11223344,
            x: 0,
            y: 0,
            width: 1280,
            height: 720,
            flags: 0xAABBCCDD,
        }];
        let buf = serialize(ClientMsg::SetDesktopSize(1280, 720, screens)).await;

        assert_eq!(buf.len(), 8 + 16, "header + one SCREEN struct");
        assert_eq!(buf[0], 251, "message-type");
        assert_eq!(buf[1], 0, "padding");
        assert_eq!(u16::from_be_bytes([buf[2], buf[3]]), 1280);
        assert_eq!(u16::from_be_bytes([buf[4], buf[5]]), 720);
        assert_eq!(buf[6], 1, "number-of-screens");
        assert_eq!(buf[7], 0, "padding");
        assert_eq!(u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]), 0x11223344);
        assert_eq!(u16::from_be_bytes([buf[12], buf[13]]), 0, "x");
        assert_eq!(u16::from_be_bytes([buf[14], buf[15]]), 0, "y");
        assert_eq!(u16::from_be_bytes([buf[16], buf[17]]), 1280);
        assert_eq!(u16::from_be_bytes([buf[18], buf[19]]), 720);
        assert_eq!(
            u32::from_be_bytes([buf[20], buf[21], buf[22], buf[23]]),
            0xAABBCCDD
        );
    }

    #[tokio::test]
    async fn framebuffer_update_request_wire_format_unchanged() {
        let rect = Rect { x: 0, y: 0, width: 1920, height: 1080 };
        let buf = serialize(ClientMsg::FramebufferUpdateRequest(rect, 1)).await;
        assert_eq!(buf[0], 3);
        assert_eq!(buf[1], 1, "incremental");
        assert_eq!(u16::from_be_bytes([buf[6], buf[7]]), 1920);
        assert_eq!(u16::from_be_bytes([buf[8], buf[9]]), 1080);
    }
}
