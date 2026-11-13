//! TRLE (RFB framebuffer encoding type 15) probe.
//!
//! Usage:
//!   trle_probe <host[:port]> [--password P] [--seconds N] [-v|-vv]
//!
//! The client advertises *only* type 15, so a server that does not implement
//! TRLE has to answer with some other encoding, which this library rejects as
//! unadvertised. That makes the probe a direct test of "does this server really
//! emit type 15", which is the open question for MiniVNC.

use anyhow::{Context, Result};
use std::time::Duration;
use tokio::net::TcpStream;
use tracing::Level;
use vnc::{PixelFormat, VncConnector, VncEncoding, VncError, VncEvent, X11Event};

const DEFAULT_PORT: u16 = 5900;

fn usage() -> ! {
    eprintln!(
        "usage: trle_probe <host[:port]> [--password P] [--seconds N] [-v|-vv]\n\
         \n\
         \x20 --password  VNC password, when the server requires authentication\n\
         \x20 --seconds   how long to drain updates (default 5)\n\
         \x20 -v / -vv    debug / trace logging"
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut verbose = 0u8;
    let mut positional = Vec::new();
    let mut password = String::new();
    let mut seconds = 5u64;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-v" => verbose = verbose.max(1),
            "-vv" => verbose = 2,
            "--password" => {
                password = match args.get(i + 1).cloned() {
                    Some(value) => value,
                    None => usage(),
                };
                i += 1;
            }
            "--seconds" => {
                seconds = match args.get(i + 1).and_then(|s| s.parse().ok()) {
                    Some(value) => value,
                    None => usage(),
                };
                i += 1;
            }
            other => positional.push(other.to_string()),
        }
        i += 1;
    }

    let level = match verbose {
        0 => Level::INFO,
        1 => Level::DEBUG,
        _ => Level::TRACE,
    };
    tracing_subscriber::FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .init();

    let Some(address) = positional.first() else {
        usage();
    };
    let address = if address.contains(':') {
        address.clone()
    } else {
        format!("{address}:{DEFAULT_PORT}")
    };

    let stream = TcpStream::connect(&address)
        .await
        .with_context(|| format!("cannot connect to {address}"))?;

    // Only type 15 is advertised; the library never accepts an encoding it did
    // not offer, so anything else arrives as a decoding error.
    let client = VncConnector::new(stream)
        .set_auth_method(async move { Ok::<String, VncError>(password) })
        .set_pixel_format(PixelFormat::rgba())
        .add_encoding(VncEncoding::Trle)
        .build()?
        .try_start()
        .await
        .with_context(|| "handshake failed; the server may require a password")?
        .finish()?;

    println!(
        "connected to {address}, server name {:?}",
        client.server_name()
    );
    let _ = client.input(X11Event::FullRefresh).await;

    let mut rects = 0usize;
    let mut bytes = 0usize;
    let mut resolution = None;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(seconds);

    while tokio::time::Instant::now() < deadline {
        let event = match tokio::time::timeout(Duration::from_millis(500), client.recv_event())
            .await
        {
            Ok(Ok(event)) => event,
            Ok(Err(VncError::IoError(_))) => break,
            Ok(Err(other)) => {
                println!("no TRLE received: the server answered with an encoding we did not advertise ({other})");
                std::process::exit(1);
            }
            Err(_) => continue,
        };
        match event {
            VncEvent::RawImage(rect, data) => {
                rects += 1;
                bytes += data.len();
                println!(
                    "TRLE rectangle {}x{} at ({}, {}): {} bytes decoded",
                    rect.width,
                    rect.height,
                    rect.x,
                    rect.y,
                    data.len()
                );
            }
            VncEvent::SetResolution(screen) => {
                resolution = Some((screen.width, screen.height));
            }
            VncEvent::Error(message) => {
                println!("no TRLE received: decoder reported {message}");
                std::process::exit(1);
            }
            other => tracing::debug!("ignored event {other:?}"),
        }
    }

    if rects == 0 {
        println!(
            "connected, but the server sent no TRLE rectangle within {seconds}s \
             (resolution {:?})",
            resolution
        );
        std::process::exit(1);
    }
    println!("success: {rects} TRLE rectangle(s), {bytes} decoded byte(s)");
    client.close().await?;
    Ok(())
}
