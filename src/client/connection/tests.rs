use super::*;
use std::time::Duration;
use tokio::io::duplex;

fn rect_header(rect: Rect, encoding: u32) -> Vec<u8> {
    let mut bytes = vec![0, 0, 0, 1];
    for value in [rect.x, rect.y, rect.width, rect.height] {
        bytes.extend(value.to_be_bytes());
    }
    bytes.extend(encoding.to_be_bytes());
    bytes
}

#[tokio::test]
async fn oversized_initialization_is_rejected_before_payload_reads() {
    for (width, height, length) in [
        (u16::MAX, 1, 0),
        (8192, 8192, 0),
        (0, 1, 0),
        (10, 10, u32::MAX),
    ] {
        let (mut client, mut server) = duplex(256);
        server.write_u16(width).await.unwrap();
        server.write_u16(height).await.unwrap();
        server
            .write_all(&Vec::<u8>::from(PixelFormat::rgba()))
            .await
            .unwrap();
        server.write_u32(length).await.unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            read_server_init(&mut client, &mut Some(PixelFormat::rgba()), &|_| async {
                Ok(())
            }),
        )
        .await
        .unwrap();
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn invalid_rectangles_and_lengths_are_rejected_before_payload_reads() {
    let rect = Rect {
        x: 0,
        y: 0,
        width: 1,
        height: 1,
    };
    let mut copy = rect_header(rect, VncEncoding::CopyRect.into());
    copy.extend([0, 64, 0, 0]);
    let mut zrle = rect_header(rect, VncEncoding::Zrle.into());
    zrle.extend(u32::MAX.to_be_bytes());
    let mut clipboard = vec![3, 0, 0, 0];
    clipboard.extend(u32::MAX.to_be_bytes());
    for payload in [
        rect_header(rect, 12345),
        rect_header(rect, VncEncoding::Tight.into()), // not negotiated
        rect_header(Rect { x: 64, ..rect }, 0),
        rect_header(
            Rect {
                x: u16::MAX,
                ..rect
            },
            0,
        ),
        rect_header(
            Rect {
                width: u16::MAX,
                ..rect
            },
            VncEncoding::DesktopSizePseudo.into(),
        ),
        copy,
        zrle,
        clipboard,
        vec![1],
    ] {
        let (mut client, mut server) = duplex(256);
        server.write_all(&payload).await.unwrap();
        let (_stop, stopped) = oneshot::channel();
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            asycn_vnc_read_loop(
                &mut client,
                &PixelFormat::rgba(),
                &|_| async { Ok(()) },
                stopped,
                &[
                    VncEncoding::Raw,
                    VncEncoding::CopyRect,
                    VncEncoding::Zrle,
                    VncEncoding::DesktopSizePseudo,
                ],
                (64, 64),
            ),
        )
        .await
        .unwrap();
        assert!(result.is_err());
    }
}

#[tokio::test]
async fn raw_is_implicit_and_resize_changes_decoder_bounds() {
    let mut bytes = rect_header(
        Rect {
            x: 0,
            y: 0,
            width: 128,
            height: 96,
        },
        VncEncoding::DesktopSizePseudo.into(),
    );
    bytes.extend(rect_header(
        Rect {
            x: 127,
            y: 95,
            width: 1,
            height: 1,
        },
        0,
    ));
    bytes.extend([1, 2, 3, 255]);
    let output = std::sync::Mutex::new(Vec::new());
    let (_stop, stopped) = oneshot::channel();
    let result = asycn_vnc_read_loop(
        &mut bytes.as_slice(),
        &PixelFormat::rgba(),
        &|event| {
            output.lock().unwrap().push(event);
            async { Ok(()) }
        },
        stopped,
        &[VncEncoding::DesktopSizePseudo],
        (64, 64),
    )
    .await;
    assert!(matches!(result, Err(VncError::IoError(_))));
    let output = output.into_inner().unwrap();
    assert!(
        matches!(&output[0], VncEvent::SetResolution(size) if (size.width, size.height) == (128, 96))
    );
    assert!(
        matches!(&output[1], VncEvent::RawImage(rect, bytes) if (rect.x, rect.y) == (127, 95) && bytes == &[1, 2, 3, 255])
    );
}
