use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use vnc::{PixelFormat, VncConnector, VncEncoding, VncError, VncEvent};

const SERVER_VERSION: &[u8; 12] = b"RFB 003.008\n";

#[tokio::test]
async fn tcp_server_sends_trle_and_client_decodes_it() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server_task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(SERVER_VERSION).await.unwrap();
        let mut version = [0_u8; 12];
        stream.read_exact(&mut version).await.unwrap();
        assert_eq!(&version, SERVER_VERSION);

        stream.write_all(&[1, 1]).await.unwrap();
        assert_eq!(stream.read_u8().await.unwrap(), 1);
        stream.write_u32(0).await.unwrap();
        assert_eq!(stream.read_u8().await.unwrap(), 1);

        stream.write_u16(2).await.unwrap();
        stream.write_u16(1).await.unwrap();
        stream
            .write_all(&Vec::<u8>::from(PixelFormat::rgba()))
            .await
            .unwrap();
        stream.write_u32(0).await.unwrap();

        assert_eq!(stream.read_u8().await.unwrap(), 2);
        assert_eq!(stream.read_u8().await.unwrap(), 0);
        let count = stream.read_u16().await.unwrap();
        let mut encodings = Vec::with_capacity(usize::from(count));
        for _ in 0..count {
            encodings.push(stream.read_u32().await.unwrap());
        }
        assert!(encodings.contains(&VncEncoding::Trle.into()));

        assert_eq!(stream.read_u8().await.unwrap(), 3);
        let mut request = [0_u8; 9];
        stream.read_exact(&mut request).await.unwrap();

        stream.write_all(&[0, 0, 0, 1]).await.unwrap();
        stream.write_all(&[0, 0, 0, 0, 0, 2, 0, 1]).await.unwrap();
        let trle_encoding: u32 = VncEncoding::Trle.into();
        stream.write_u32(trle_encoding).await.unwrap();
        stream.write_all(&[0, 1, 2, 3, 4, 5, 6]).await.unwrap();
    });

    type UnusedAuth = std::future::Ready<Result<String, VncError>>;
    let client = VncConnector::<_, UnusedAuth>::new(TcpStream::connect(address).await.unwrap())
        .add_encoding(VncEncoding::Trle)
        .build()
        .unwrap()
        .try_start()
        .await
        .unwrap()
        .finish()
        .unwrap();

    let first = tokio::time::timeout(Duration::from_secs(1), client.recv_event())
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(first, VncEvent::SetResolution(screen) if (screen.width, screen.height) == (2, 1))
    );

    let second = tokio::time::timeout(Duration::from_secs(1), client.recv_event())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(second, VncEvent::SetPixelFormat(_)));

    let third = tokio::time::timeout(Duration::from_secs(1), client.recv_event())
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        third,
        VncEvent::RawImage(rect, data)
            if (rect.width, rect.height) == (2, 1)
                && data == [1, 2, 3, 255, 4, 5, 6, 255]
    ));

    client.close().await.unwrap();
    server_task.await.unwrap();
}
