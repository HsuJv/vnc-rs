# vnc-rs

[![Build](https://github.com/HsuJv/vnc-rs/actions/workflows/build.yml/badge.svg?branch=main)](https://github.com/HsuJv/vnc-rs/actions/workflows/build.yml)
[![API Docs](https://docs.rs/vnc-rs/badge.svg)](https://docs.rs/vnc-rs/latest/vnc)
[![LICENSE](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE-MIT)
[![LICENSE](https://img.shields.io/badge/license-Apache-blue.svg)](LICENSE-APACHE)

## Description

An async implementation of VNC client side protocol

Worked as a protocol engine which consumes input event from the frontend and the vnc server and generate event to let the frontend render.

Can be used on both the OS and WASI.

A simple X11 client usage can be found at [example](https://github.com/HsuJv/vnc-rs/blob/main/example/src/main.rs)

A command line probe that talks Apple ARD to the built-in macOS server can be found at [ard_probe](https://github.com/HsuJv/vnc-rs/blob/main/example/src/bin/ard_probe.rs)

A simple web assembly client can be found at [webvnc](https://github.com/HsuJv/webgateway/tree/main/webvnc/src)

If you encounter any problems in use, Any [issues](https://github.com/HsuJv/vnc-rs/issues) or [pull requests](https://github.com/HsuJv/vnc-rs/pulls) are welcome.

## Why this

I initially intended to write a wasm version of vnc, and yes, a wasm version has been implemented as per the [VNC Core RFC](https://www.rfc-editor.org/rfc/rfc6143.html)

During the implementation I found an existing respository [whitequark's rust vnc](https://github.com/whitequark/rust-vnc). Which is too coupled to the OS (requring multi-thread and `std::io::TcpStream`) to migrate to a wasm application. But thanks to whitequark's work, I finally worked out how to do VncAuth on the client side.

Looking back [whitequark's rust vnc](https://github.com/whitequark/rust-vnc) and [my old webvnc](https://github.com/HsuJv/webgateway/tree/a031a9d0472677cb17cc269abdd1cbc7349582bc/webvnc),  I didn't think it would be appropriate to put the parsing of the vnc protocol directly into the application. So I separated the engine part and the result is this crate.

It is intended to be a more compatible vnc engine that can be built for both OS applications and Wasm applications. So I did my best to minimise dependencies. However, asynchrony is necessary for websocket processes and in the end I chose `tokio` over `async_std`, which makes it a bit incompatible.

## Authentication

The engine negotiates three RFB security types:

| Type | Name | How to enable |
| --- | --- | --- |
| 1 | None | nothing to configure |
| 2 | VNC authentication | `set_auth_method`, which supplies the server password |
| 30 | Apple Remote Desktop | `set_ard_credentials(username, password)` with a macOS account |

Against the built-in macOS Screen Sharing service, prefer ARD. The client
answers Apple's `RFB 003.889` version with the 3.8 handshake, runs an ephemeral
Diffie-Hellman exchange and sends the credentials in a single AES-128 encrypted
block, so nobody needs to be logged into the Mac first — a machine locked at the
login window still accepts the connection. VNC authentication only works when
"VNC viewers may control screen with password" is enabled there, and macOS 10.7
and later raise a login window that blocks unattended use.

Like every other ARD implementation, only the credential block is encrypted;
the desktop session after authentication is not. Apple's other security types
(31 to 36) are not implemented.

Two limits are visible from the API:

+ Both fields of the credential block are 64 bytes wide, so a username or
  password longer than 63 bytes is truncated and a warning is logged.
+ ARD needs the 3.8 handshake. A client that `set_version` limited to an older
  release fails with `VncError::ArdVersionConflict` when it meets an Apple
  server, instead of having that limit raised silently.

### Verified against a real Mac

Protocol logic is covered by the fake-server tests (`cargo test`). The
interoperability check below was run on 2026-11-11 against the built-in macOS
Screen Sharing service (2880x1800 desktop, same subnet, Screen Sharing enabled,
the account allowed to use it, the Mac kept awake, and "VNC viewers may control
screen with password" left unchecked so the server offers only Apple types).

```bash
# what the server offers: expect "RFB 003.889" and a 30 in the list
cargo run --manifest-path example/Cargo.toml --bin ard_probe -- <mac-ip>

# full ARD login, with the short account name printed by `id -un` on the Mac
cargo run --manifest-path example/Cargo.toml --bin ard_probe -- <mac-ip> --connect <username> '<password>'
```

| Check | Result |
| --- | --- |
| Security types | pass — `RFB 003.889`, only 30/31/32/33/35/36 offered, no type 2 |
| Correct credentials | pass — authentication 749 ms, first frame at 1.63 s |
| Wrong password | pass — `ARD authentication failed: Authentication or authorization failure`, exit 1, no hang |
| Locked at the login window | pass — authenticates and delivers a frame with nobody logged in and no local approval |
| Fallback (VNC password option set) | pass — type 2 and 30 both offered, the client still prefers 30 |

When a run does not match, start here:

+ Only `2 (VNC auth)` is offered — the "VNC viewers may control screen with
  password" checkbox is set; clear it to test ARD.
+ A version string other than `003.889` — the peer is not the built-in macOS
  service on port 5900.
+ `ArdAuthFailed` with a password you know is right — the account is not allowed
  in the Screen Sharing list.
+ After `close()` the decoder logs one `General("Channel closed")` error; it is
  harmless and does not change the exit status.

`-vv` traces can contain sensitive data, so redact them before reporting and
never include the password. If none of the notes above explains a failing run,
open an issue with the macOS version, the full output of the probe command and
the `-vv` output of the failing step.

## Encodings
I've only tried video streaming from tight vnc server on win10/ubuntu 20.04 and the built-in vnc server on macos

On the macOS server both authentication paths have been exercised: VNC authentication with "VNC viewers may control screen with password" enabled, and Apple ARD with it disabled, which also works while the Mac sits locked at the login window (see Verified against a real Mac above).

Tight encoding, Zrle encoding & Raw encoding all work fine.

According to the RFC, the [Hextile Encoding](https://www.rfc-editor.org/rfc/rfc6143.html#section-7.7.4) and [RRE Encoding](https://www.rfc-editor.org/rfc/rfc6143.html#section-7.7.3) are both obsolescent, so I didn't try to implement them.

### TRLE (encoding type 15)

The decoder follows [RFC 6143 section 7.7.5](https://www.rfc-editor.org/rfc/rfc6143.html#section-7.7.5):
16x16 tiles, one subencoding byte per tile, no length header and no inflate,
with the palette, solid colour and reuse mode remembered across the tiles of
one rectangle and reset at the next rectangle — that is what the reference
decoder `libvncclient/trle.c` does, and it matches the RFC's "the previous
tile" wording. Malformed input (an unused subencoding, an out-of-range palette
index, a truncated stream, a run longer than the pixels left in the tile)
returns an error rather than guessing; the offending tile emits nothing, though
tiles already delivered earlier in the same rectangle are not rolled back.

Three differences from the reference decoder are deliberate: a run that
overruns the tile is an error instead of a `possible TRLE corruption` warning,
subencoding 129 without a known palette is an error instead of reading
uninitialised memory, and everything else — tile size, the subencoding table,
the `& 0x7f` mapping when 127 reuses a 130-and-up mode, and the quirk that
subencoding 0, 128 and 129 leave the remembered mode unchanged — matches it.

**Verified**

+ 9 unit tests in `src/codec/trle.rs` cover every subencoding, tile clipping,
  palette reuse and the malformed branches, including that raw, plain RLE and
  palette RLE tiles leave the reuse state untouched.
+ `tests/trle.rs` runs over a real TCP loopback: handshake, `SetEncodings`
  advertising type 15, a type-15 rectangle, and the resulting `RawImage`.
+ Cross-checked against `libvncclient/trle.c` compiled as an independent oracle
  over 147 generated rectangles (139 valid, 8 malformed): our decoder against
  the encoder's ground truth 147/147, against the oracle 146/147 — the single
  difference is the intentional run-length bound above. Two traps when
  reproducing this: the palette-RLE subencoding is `128 + k`, because the
  reference derives the palette length as `type - 128`; and multi-tile
  rectangles must be compared at tile coordinates, since concatenating tiles
  in order gives a different pixel arrangement. The generator lives outside the
  repository on another machine, so this check is not reproducible from a fresh
  clone.
+ The oracle only understands 4-byte CPIXELs, so it cannot cover the 3-byte
  CPIXEL case RFC 6143 defines for 32bpp formats of depth 24 or less; that path
  rests on our own unit tests.

**Who actually sends type 15**

Advertising the encoding does not make a server use it: RFB lets the server
answer with any encoding it implements. Checked at source level, none of the
common servers has a type 15 encoder — libvncserver ships a client decoder only
(its README support table and the "Added a TRLE decoder" NEWS entry both say
so), UltraVNC 1.8.x has no type 15 definition and no such encoder, TigerVNC and
QEMU have none either, and the GPL RealVNC 4.0 and 4.1 `encodings.h` lists
Raw, CopyRect, RRE, CoRRE, Hextile and ZRLE but not TRLE. macOS Screen Sharing
answered with Raw in our tests. RFC 6143's statement that "current servers use
the ZRLE, TRLE, and CopyRect encodings" matches none of these trees.

The one confirmed server-side implementation is
[MiniVNC](https://github.com/marciot/mac-minivnc), a VNC server for 68k
Macintosh machines: `mTRLEEncoding = 15` is selected when the client advertises
it, the tile bytes come from `VNCEncodeTRLE.cpp`, and `VNCServer.cpp` writes
that encoding value into the framebuffer-update rectangle header.
`cargo run --manifest-path example/Cargo.toml --bin trle_probe -- <host>`
advertises type 15 only, so the answer is unambiguous: either the server sends
TRLE rectangles and each one decodes, or the library rejects the encoding it
never offered.

**Not verified**

+ No live MiniVNC run. It needs a 68k Mac ROM and a System 7 disk with MacTCP,
  neither of which is redistributable; Basilisk II itself is packaged in
  Ubuntu's multiverse and can use its built-in slirp with port forwarding, so
  no root is required once those two files exist.
+ Porting MiniVNC's encoder to Linux for a byte-level comparison does not work.
  The shipped build uses its 68k assembly tile routines; the "portable C"
  fallback in `VNCEncodeTilesC.cpp` has never been compiled — it does not build
  (`*src32++` increments a cast rvalue in nine places, `nativeToRle` uses an
  undeclared `rmask`, and two `goto`s jump over initialised locals) — and once
  those are repaired it still runs past its scratch buffer. Its output would
  not represent a real MiniVNC stream, so the live run is the only meaningful
  third-party check left.
+ Whether MiniVNC handles width-clipped tiles correctly is open: its native
  fast path sizes packed rows without per-row padding, which RFC 6143 requires
  when a tile's width is not a multiple of 8, 4 or 2 pixels. Only the live run
  can settle it.

## Decoder limits

Connections reject framebuffers exceeding 8,294,400 pixels or 8192 pixels on
either axis, compressed rectangles over 64 MiB, clipboard text over 1 MiB,
and desktop names or failure reasons over 4096 bytes. These are per-message
limits, not a total application memory budget. Unknown or unadvertised wire
encodings are rejected; Raw is always accepted as required by RFB.

## Simple example

```Rust
use anyhow::{Context, Result};
use minifb::{Window, WindowOptions};
use tokio::{self, net::TcpStream};
use tracing::Level;
use vnc::{PixelFormat, Rect, VncConnector, VncEvent, X11Event};

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
        .set_auth_method(async move { Ok("123".to_string()) })
        .add_encoding(vnc::VncEncoding::Tight)
        .add_encoding(vnc::VncEncoding::Zrle)
        .add_encoding(vnc::VncEncoding::CopyRect)
        .add_encoding(vnc::VncEncoding::Raw)
        .allow_shared(true)
        .set_pixel_format(PixelFormat::bgra())
        .build()?
        .try_start()
        .await?
        .finish()?;

    let mut canvas = CanvasUtils::new()?;

    let mut now = std::time::Instant::now();
    loop {
        match vnc.poll_event().await {
            Ok(Some(e)) => {
                let _ = canvas.hande_vnc_event(e);
            }
            Ok(None) => (),
            Err(e) => {
                tracing::error!("{}", e.to_string());
                break;
            }
        }
        if now.elapsed().as_millis() > 16 {
            let _ = canvas.flush();
            let _ = vnc.input(X11Event::Refresh).await;
            now = std::time::Instant::now();
        }
    }
    canvas.close();
    let _ = vnc.close().await;
    Ok(())
}

struct CanvasUtils {
    window: Window,
    video: Vec<u32>,
    width: u32,
    height: u32,
}

impl CanvasUtils {
    fn new() -> Result<Self> {
        Ok(Self {
            window: Window::new(
                "mstsc-rs Remote Desktop in Rust",
                800_usize,
                600_usize,
                WindowOptions::default(),
            )
            .with_context(|| "Unable to create window".to_string())?,
            video: vec![],
            width: 800,
            height: 600,
        })
    }

    fn init(&mut self, width: u32, height: u32) -> Result<()> {
        let mut window = Window::new(
            "mstsc-rs Remote Desktop in Rust",
            width as usize,
            height as usize,
            WindowOptions::default(),
        )
        .with_context(|| "Unable to create window")?;
        window.limit_update_rate(Some(std::time::Duration::from_micros(16600)));
        self.window = window;
        self.width = width;
        self.height = height;
        self.video.resize(height as usize * width as usize, 0);
        Ok(())
    }

    fn draw(&mut self, rect: Rect, data: Vec<u8>) -> Result<()> {
        // since we set the PixelFormat as bgra
        // the pixels must be sent in [blue, green, red, alpha] in the network order

        let mut s_idx = 0;
        for y in rect.y..rect.y + rect.height {
            let mut d_idx = y as usize * self.width as usize + rect.x as usize;

            for _ in rect.x..rect.x + rect.width {
                self.video[d_idx] =
                    u32::from_le_bytes(data[s_idx..s_idx + 4].try_into().unwrap()) & 0x00_ff_ff_ff;
                s_idx += 4;
                d_idx += 1;
            }
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<()> {
        self.window
            .update_with_buffer(&self.video, self.width as usize, self.height as usize)
            .with_context(|| "Unable to update screen buffer")?;
        Ok(())
    }

    fn copy(&mut self, dst: Rect, src: Rect) -> Result<()> {
        println!("Copy");
        let mut tmp = vec![0; src.width as usize * src.height as usize];
        let mut tmp_idx = 0;
        for y in 0..src.height as usize {
            let mut s_idx = (src.y as usize + y) * self.width as usize + src.x as usize;
            for _ in 0..src.width {
                tmp[tmp_idx] = self.video[s_idx];
                tmp_idx += 1;
                s_idx += 1;
            }
        }
        tmp_idx = 0;
        for y in 0..src.height as usize {
            let mut d_idx = (dst.y as usize + y) * self.width as usize + dst.x as usize;
            for _ in 0..src.width {
                self.video[d_idx] = tmp[tmp_idx];
                tmp_idx += 1;
                d_idx += 1;
            }
        }
        Ok(())
    }

    fn close(&self) {}

    fn hande_vnc_event(&mut self, event: VncEvent) -> Result<()> {
        match event {
            VncEvent::SetResolution(screen) => {
                tracing::info!("Resize {:?}", screen);
                self.init(screen.width as u32, screen.height as u32)?
            }
            VncEvent::RawImage(rect, data) => {
                self.draw(rect, data)?;
            }
            VncEvent::Bell => {
                tracing::warn!("Bell event got, but ignore it");
            }
            VncEvent::SetPixelFormat(_) => unreachable!(),
            VncEvent::Copy(dst, src) => {
                self.copy(dst, src)?;
            }
            VncEvent::JpegImage(_rect, _data) => {
                tracing::warn!("Jpeg event got, but ignore it");
            }
            VncEvent::SetCursor(rect, data) => {
                if rect.width != 0 {
                    self.draw(rect, data)?;
                }
            }
            VncEvent::Text(string) => {
                tracing::info!("Got clipboard message {}", string);
            }
            _ => unreachable!(),
        }
        Ok(())
    }
}
```

## Acknowledgements
[whitequark's rust vnc](https://github.com/whitequark/rust-vnc).

## License

Licensed under either of

 * Apache License, Version 2.0
   ([LICENSE-APACHE](LICENSE-APACHE) or http://www.apache.org/licenses/LICENSE-2.0)
 * MIT license
   ([LICENSE-MIT](LICENSE-MIT) or http://opensource.org/licenses/MIT)

at your option.

## Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted
for inclusion in the work by you, as defined in the Apache-2.0 license, shall be
dual licensed as above, without any additional terms or conditions.

## Negotiated desktop resizing

Native clients can negotiate `ExtendedDesktopSizePseudo`, inspect `desktop_layout()`, and explicitly call `resize_desktop(width, height)` while continuing to drain events. Requests require one advertised screen covering the desktop, preserve its identity and flags, and are bounded to five seconds each for queue capacity and confirmation. Cancellation, timeout or forwarded results require observation before retrying. Negotiated layouts are limited to 8192 pixels per side and 8,294,400 pixels total; a message may contain at most 16 layout updates, including LastRect-terminated messages.

`DesktopUpdate` is distinct from legacy `SetResolution`: preserve pixels when dimensions are unchanged, and never answer each extended announcement with a full refresh. Resize requests are not available in wasm builds; layout observation remains supported.
