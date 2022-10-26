use anyhow::Result;
use tokio::{self, net::TcpStream};
use vnc_rs::client::VncConnector;

#[tokio::main]
async fn main() -> Result<()> {
    let tcp = TcpStream::connect("127.0.0.1:5900").await?;
    let _vnc = VncConnector::new(tcp)
        .set_auth_method(|| "123".to_string())
        .build()
        .try_start()
        .await?;

    Ok(())
}
