//! Apple Remote Desktop (ARD / RFB security type 30) probe.
//!
//! Usage:
//!   ard_probe <host[:port]> [--connect <username> <password>] [-v|-vv]
//!
//! Without `--connect` this only performs the version exchange and prints the
//! security types the server offers, which tells whether type 30 (ARD) is
//! available at all. With `--connect` it runs the full ARD flow through the
//! library and reports whether authentication and the first frame succeed.

use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::Level;
use vnc::{VncConnector, VncEncoding, VncError, VncEvent};

const APPLE_VERSION: &[u8; 12] = b"RFB 003.889\n";
const DEFAULT_PORT: u16 = 5900;

fn usage() -> ! {
    eprintln!(
        "usage: ard_probe <host[:port]> [--connect <username> <password>] [-v|-vv]\n\
         \n\
           (no --connect)  print the server version and offered security types\n\
           --connect       run the full ARD connection and wait for a first frame\n\
           -v / -vv        debug / trace logging"
    );
    std::process::exit(2)
}

#[tokio::main]
async fn main() -> Result<()> {
    let mut verbose = 0u8;
    let mut positional = Vec::new();
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "-v" => verbose = verbose.max(1),
            "-vv" => verbose = 2,
            _ => positional.push(arg),
        }
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
    let connect = match positional.get(1).map(String::as_str) {
        None => None,
        Some("--connect") => match (positional.get(2), positional.get(3)) {
            (Some(username), Some(password)) => Some((username.clone(), password.clone())),
            _ => usage(),
        },
        Some(_) => usage(),
    };
    let address = if address.contains(':') {
        address.clone()
    } else {
        format!("{address}:{DEFAULT_PORT}")
    };

    match connect {
        Some((username, password)) => connect_with_ard(&address, &username, &password).await,
        None => probe_security_types(&address).await,
    }
}

fn security_type_name(security_type: u8) -> &'static str {
    match security_type {
        0 => "invalid",
        1 => "None",
        2 => "VNC auth",
        16 => "Tight",
        18 => "TLS",
        19 => "VeNCrypt",
        30 => "Apple ARD",
        33 | 35 => "Apple (legacy SRP/DH)",
        36 => "Apple SRP",
        _ => "unknown",
    }
}

async fn read_failure_reason(stream: &mut TcpStream) -> Result<String> {
    let length = stream.read_u32().await?.min(4096) as usize;
    let mut reason = vec![0u8; length];
    stream.read_exact(&mut reason).await?;
    Ok(String::from_utf8_lossy(&reason).into_owned())
}

async fn probe_security_types(address: &str) -> Result<()> {
    let mut stream = TcpStream::connect(address)
        .await
        .with_context(|| format!("cannot connect to {address}"))?;

    let mut version = [0u8; 12];
    stream.read_exact(&mut version).await?;
    println!("server version: {:?}", String::from_utf8_lossy(&version));
    println!("apple server:   {}", &version == APPLE_VERSION);

    let reply: &[u8; 12] = match &version {
        b"RFB 003.003\n" => b"RFB 003.003\n",
        b"RFB 003.007\n" => b"RFB 003.007\n",
        _ => b"RFB 003.008\n",
    };
    stream.write_all(reply).await?;

    if reply == b"RFB 003.003\n" {
        let security_type = stream.read_u32().await?;
        println!(
            "offered (3.3 single): {security_type} ({})",
            security_type_name(security_type as u8)
        );
        return Ok(());
    }

    let count = stream.read_u8().await?;
    if count == 0 {
        let reason = read_failure_reason(&mut stream).await.unwrap_or_default();
        println!("server refused the connection: {reason}");
        return Ok(());
    }
    let mut types = vec![0u8; count as usize];
    stream.read_exact(&mut types).await?;
    let described = types
        .iter()
        .map(|t| format!("{t} ({})", security_type_name(*t)))
        .collect::<Vec<_>>()
        .join(", ");
    println!("offered security types: {described}");
    if types.contains(&30) {
        println!("=> type 30 (Apple ARD) is available; retry with --connect <username> <password>");
    } else {
        println!("=> type 30 (Apple ARD) is NOT offered by this server");
    }
    Ok(())
}

async fn connect_with_ard(address: &str, username: &str, password: &str) -> Result<()> {
    let tcp = TcpStream::connect(address)
        .await
        .with_context(|| format!("cannot connect to {address}"))?;
    let started = Instant::now();

    let state = VncConnector::new(tcp)
        .set_ard_credentials(username, password)
        .set_auth_method(async { Err(VncError::NoPassword) })
        .add_encoding(VncEncoding::Tight)
        .add_encoding(VncEncoding::Zrle)
        .add_encoding(VncEncoding::CopyRect)
        .add_encoding(VncEncoding::Raw)
        .allow_shared(true)
        .build()?
        .try_start()
        .await;
    let vnc = match state {
        Ok(state) => state.finish()?,
        Err(VncError::ArdNotOffered) => {
            bail!("the server does not offer Apple ARD (type 30); run without --connect to see the offered security types")
        }
        Err(VncError::NoPassword) => {
            bail!("the server does not offer Apple ARD (type 30); it wants VNC authentication instead")
        }
        Err(VncError::ArdAuthFailed(reason)) => {
            bail!("ARD authentication failed: {reason}")
        }
        Err(e) => return Err(e.into()),
    };
    println!(
        "ARD authentication succeeded in {:?}, desktop name: {:?}",
        started.elapsed(),
        vnc.server_name()
    );

    let first_frame = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            match vnc.poll_event().await? {
                Some(VncEvent::SetResolution(screen)) => {
                    println!("resolution: {screen:?} ({:?})", started.elapsed());
                }
                Some(VncEvent::RawImage(rect, data)) => {
                    println!("first frame: rect {rect:?}, {} bytes", data.len());
                    return Ok::<(), VncError>(());
                }
                Some(_) => {}
                None => tokio::time::sleep(Duration::from_millis(5)).await,
            }
        }
    })
    .await;

    match first_frame {
        Ok(Ok(())) => {
            println!(
                "success: ARD login bypassed the login window and got a frame in {:?}",
                started.elapsed()
            );
            vnc.close().await?;
            Ok(())
        }
        Ok(Err(e)) => {
            let _ = vnc.close().await;
            Err(e.into())
        }
        Err(_) => {
            let _ = vnc.close().await;
            bail!("timed out waiting for the first frame")
        }
    }
}
