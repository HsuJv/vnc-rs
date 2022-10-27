use anyhow::Result;
use tokio::{self, net::TcpStream};
use tracing::Level;
use vnc::{client::connector::VncConnector, PixelFormat, X11Event};

#[tokio::main]
async fn main() -> Result<()> {
    // Create tracing subscriber
    #[cfg(debug_assertions)]
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(Level::TRACE)
        .finish();
    #[cfg(not(debug_assertions))]
    let subscriber = tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(Level::INFO)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("setting default subscriber failed");

    let tcp = TcpStream::connect("127.0.0.1:5900").await?;
    let vnc = VncConnector::new(tcp)
        .set_auth_method(|| "123".to_string())
        .add_encoding(vnc::VncEncoding::Raw)
        .allow_shared(true)
        .set_pixel_format(PixelFormat::rgba())
        .build()?
        .try_start()
        .await?
        .finish()?;
    let (vnc_out_send, mut vnc_out_recv) = tokio::sync::mpsc::channel(100);
    let (vnc_in_send, vnc_in_recv) = tokio::sync::mpsc::channel(100);
    tokio::spawn(async move { vnc.run(vnc_out_send, vnc_in_recv).await.unwrap() });

    while let Some(_event) = vnc_out_recv.recv().await {
        vnc_in_send.send(X11Event::Refresh).await?;
    }
    Ok(())
}
