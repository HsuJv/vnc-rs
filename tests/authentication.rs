use std::time::Duration;
use tokio::io::{duplex, AsyncReadExt, AsyncWriteExt};
use vnc::{VncConnector, VncEncoding, VncError, VncVersion};

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
