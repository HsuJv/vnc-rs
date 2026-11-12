use std::time::Duration;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use vnc::{VncConnector, VncEncoding, VncError, VncVersion};

/// Apple's non-standard version string, reported by macOS servers.
const APPLE_VERSION: &[u8; 12] = b"RFB 003.889\n";

#[tokio::test]
async fn ard_authentication_completes_with_the_38_handshake() {
    let (client, mut server) = duplex(4096);
    let server_task = tokio::spawn(async move {
        server.write_all(APPLE_VERSION).await.unwrap();
        let mut version = [0u8; 12];
        server.read_exact(&mut version).await.unwrap();
        // ARD must negotiate 3.8 so the server offers its security-type list.
        assert_eq!(&version, b"RFB 003.008\n");
        server.write_all(&[1, 30]).await.unwrap();
        let mut selected = [0u8; 1];
        server.read_exact(&mut selected).await.unwrap();
        assert_eq!(selected[0], 30);

        // Challenge: generator 2, Mersenne prime 2^127 - 1, server key 2.
        server.write_u16(2).await.unwrap();
        server.write_u16(16).await.unwrap();
        let mut prime = [0xffu8; 16];
        prime[0] = 0x7f;
        server.write_all(&prime).await.unwrap();
        let mut server_key = [0u8; 16];
        server_key[15] = 2;
        server.write_all(&server_key).await.unwrap();

        // Response: 128-byte encrypted credentials + 16-byte client key.
        let mut response = [0u8; 144];
        server.read_exact(&mut response).await.unwrap();
        server.write_u32(0).await.unwrap();

        let mut shared = [0u8; 1];
        server.read_exact(&mut shared).await.unwrap();
        assert_eq!(shared[0], 1);
        server.write_u16(1024).await.unwrap();
        server.write_u16(768).await.unwrap();
        server
            .write_all(&[32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0])
            .await
            .unwrap();
        server.write_u32(0).await.unwrap();
        // SetEncodings and the initial update request follow; just drain them.
        let mut rest = [0u8; 64];
        let _ = server.read(&mut rest).await.unwrap();
    });

    let result = tokio::time::timeout(
        Duration::from_secs(1),
        VncConnector::new(client)
            .set_ard_credentials("user", "pass")
            .set_auth_method(async { Ok("test".into()) })
            .add_encoding(VncEncoding::Raw)
            .build()
            .unwrap()
            .try_start(),
    )
    .await
    .expect("ARD handshake timed out");
    let client = result.unwrap().finish().unwrap();
    assert_eq!(client.server_name(), "");
    drop(client);
    server_task.await.unwrap();
}

#[tokio::test]
async fn ard_with_a_lower_version_limit_is_rejected() {
    for version in [VncVersion::RFB33, VncVersion::RFB37] {
        let (client, mut server) = duplex(256);
        let server_task = tokio::spawn(async move {
            server.write_all(APPLE_VERSION).await.unwrap();
            // The client reports the conflict before answering the greeting.
            let mut sent = Vec::new();
            server.read_to_end(&mut sent).await.unwrap();
            assert!(sent.is_empty());
        });
        type UnusedAuth = std::future::Ready<Result<String, VncError>>;
        let result = tokio::time::timeout(
            Duration::from_secs(1),
            VncConnector::<_, UnusedAuth>::new(client)
                .set_ard_credentials("user", "pass")
                .set_version(version)
                .add_encoding(VncEncoding::Raw)
                .build()
                .unwrap()
                .try_start(),
        )
        .await
        .expect("version negotiation timed out");
        assert!(matches!(
            result,
            Err(VncError::ArdVersionConflict(limit)) if limit == version
        ));
        server_task.await.unwrap();
    }
}

#[tokio::test]
async fn missing_ard_support_is_reported() {
    let (client, mut server) = duplex(256);
    let server_task = tokio::spawn(async move {
        server.write_all(APPLE_VERSION).await.unwrap();
        let mut version = [0u8; 12];
        server.read_exact(&mut version).await.unwrap();
        assert_eq!(&version, b"RFB 003.008\n");
        // Only Tight is offered: neither ARD nor a legacy method.
        server.write_all(&[1, 16]).await.unwrap();
    });
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        VncConnector::new(client)
            .set_ard_credentials("user", "pass")
            .set_auth_method(async { Ok("test".into()) })
            .add_encoding(VncEncoding::Raw)
            .build()
            .unwrap()
            .try_start(),
    )
    .await
    .expect("security negotiation timed out");
    assert!(matches!(result, Err(VncError::ArdNotOffered)));
    server_task.await.unwrap();
}

#[tokio::test]
async fn ard_without_fallback_credentials_reports_not_offered() {
    let (client, mut server) = duplex(256);
    let server_task = tokio::spawn(async move {
        server.write_all(APPLE_VERSION).await.unwrap();
        let mut version = [0u8; 12];
        server.read_exact(&mut version).await.unwrap();
        // VNC authentication only, and no VNC password was supplied.
        server.write_all(&[1, 2]).await.unwrap();
    });
    type UnusedAuth = std::future::Ready<Result<String, VncError>>;
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        VncConnector::<_, UnusedAuth>::new(client)
            .set_ard_credentials("user", "pass")
            .add_encoding(VncEncoding::Raw)
            .build()
            .unwrap()
            .try_start(),
    )
    .await
    .expect("security negotiation timed out");
    assert!(matches!(result, Err(VncError::ArdNotOffered)));
    server_task.await.unwrap();
}

#[tokio::test]
async fn apple_version_still_downgrades_without_ard() {
    let (client, mut server) = duplex(256);
    let server_task = tokio::spawn(async move {
        server.write_all(APPLE_VERSION).await.unwrap();
        let mut version = [0u8; 12];
        server.read_exact(&mut version).await.unwrap();
        // Without ARD the historical RFC 6143 downgrade applies unchanged.
        assert_eq!(&version, b"RFB 003.003\n");
        server.write_u32(3).await.unwrap();
    });
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        VncConnector::new(client)
            .set_auth_method(async { Ok("test".into()) })
            .add_encoding(VncEncoding::Raw)
            .build()
            .unwrap()
            .try_start(),
    )
    .await
    .expect("security negotiation timed out");
    assert!(matches!(result, Err(VncError::ConnectError)));
    server_task.await.unwrap();
}

#[tokio::test]
async fn failures_finish_without_waiting_for_server_eof() {
    for version in [VncVersion::RFB33, VncVersion::RFB37, VncVersion::RFB38] {
        for password in [false, true] {
            if !password && version != VncVersion::RFB38 {
                continue;
            }
            for status in [1_u32, 2, u32::MAX] {
                let (client, mut server) = duplex(256);
                let greeting: &[u8; 12] = version.into();
                server.write_all(greeting).await.unwrap();
                if version == VncVersion::RFB33 {
                    server.write_u32(2).await.unwrap();
                } else {
                    server
                        .write_all(&[1, if password { 2 } else { 1 }])
                        .await
                        .unwrap();
                }
                if password {
                    server.write_all(&[0; 16]).await.unwrap();
                }
                server.write_u32(status).await.unwrap();
                if status == 1 && version == VncVersion::RFB38 {
                    server.write_u32(6).await.unwrap();
                    server.write_all(b"denied").await.unwrap();
                }
                let result = tokio::time::timeout(
                    Duration::from_secs(1),
                    VncConnector::new(client)
                        .set_auth_method(async { Ok("test".into()) })
                        .set_version(version)
                        .add_encoding(VncEncoding::Raw)
                        .build()
                        .unwrap()
                        .try_start(),
                )
                .await
                .expect("authentication waited for EOF");
                if status == 1 && version != VncVersion::RFB38 {
                    assert!(matches!(result, Err(VncError::WrongPassword)));
                } else {
                    assert!(matches!(result, Err(VncError::General(_))));
                }
                // No ClientInit byte may be sent after authentication fails.
                let mut sent = Vec::new();
                server.read_to_end(&mut sent).await.unwrap();
                assert_eq!(
                    sent.len(),
                    12 + usize::from(version != VncVersion::RFB33) + if password { 16 } else { 0 }
                );
            }
        }
    }
}
