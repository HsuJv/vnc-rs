use futures::TryStreamExt;
use std::{
    future::Future,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{
        mpsc::{channel, error::TryRecvError, Receiver, Sender},
        oneshot, Mutex,
    },
};
use tokio_stream::wrappers::ReceiverStream;
use tokio_util::compat::*;
use tracing::*;

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local as spawn;

use crate::{codec, PixelFormat, Rect, VncEncoding, VncError, VncEvent, X11Event};

use super::desktop::{Client as DesktopClient, Desktop};
use super::messages::{ClientMsg, ServerMsg};

#[cfg(feature = "desktop-resize")]
mod desktop;
mod output;

const NETWORK_CHANNEL_SIZE: usize = 4096;
const INPUT_CHANNEL_SIZE: usize = 4096;
const OUTPUT_CHANNEL_SIZE: usize = 2;

/// The framebuffer size is shared between the decoder, which learns of resizes,
/// and the input path, which sizes refresh requests; pack it into one atomic.
pub(crate) fn pack_screen((width, height): (u16, u16)) -> u32 {
    (u32::from(width) << 16) | u32::from(height)
}

pub(crate) fn unpack_screen(packed: u32) -> (u16, u16) {
    ((packed >> 16) as u16, packed as u16)
}

struct VncInner {
    name: String,
    screen: Arc<AtomicU32>,
    desktop: Desktop,
    input_ch: Sender<ClientMsg>,
    output_ch: Receiver<VncEvent>,
    decoding_stop: Option<oneshot::Sender<()>>,
    net_conn_stop: Option<oneshot::Sender<()>>,
    closed: bool,
}

/// The instance of a connected vnc client
///
impl VncInner {
    async fn new<S>(
        mut stream: S,
        shared: bool,
        mut pixel_format: Option<PixelFormat>,
        encodings: Vec<VncEncoding>,
    ) -> Result<Self, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let (conn_ch_tx, conn_ch_rx) = channel(NETWORK_CHANNEL_SIZE);
        let (input_ch_tx, input_ch_rx) = channel(INPUT_CHANNEL_SIZE);
        let (output_ch_tx, output_ch_rx) = channel(OUTPUT_CHANNEL_SIZE);
        let (decoding_stop_tx, mut decoding_stop_rx) = oneshot::channel();
        let (net_conn_stop_tx, net_conn_stop_rx) = oneshot::channel();

        trace!("client init msg");
        send_client_init(&mut stream, shared).await?;

        trace!("server init msg");
        let (name, (width, height)) =
            read_server_init(&mut stream, &mut pixel_format, &|e| async {
                output_ch_tx.send(e).await?;
                Ok(())
            })
            .await?;

        let screen = Arc::new(AtomicU32::new(pack_screen((width, height))));
        let decoder_screen = Arc::clone(&screen);
        let desktop = Desktop::default();
        let decoder_desktop = desktop.clone();
        let network_desktop = desktop.clone();
        trace!("client encodings: {:?}", encodings);
        send_client_encoding(&mut stream, encodings.clone()).await?;

        trace!("Require the first frame");
        input_ch_tx
            .send(ClientMsg::FramebufferUpdateRequest(
                Rect {
                    x: 0,
                    y: 0,
                    width,
                    height,
                },
                0,
            ))
            .await?;

        // start the decoding thread
        spawn(async move {
            trace!("Decoding thread starts");
            let mut conn_ch_rx = {
                let conn_ch_rx = ReceiverStream::new(conn_ch_rx).into_async_read();
                FuturesAsyncReadCompatExt::compat(conn_ch_rx)
            };

            let output_func = |e| async {
                output_ch_tx.send(e).await?;
                Ok(())
            };

            let pf = pixel_format.as_ref().unwrap();
            let result = asycn_vnc_read_loop(
                &mut conn_ch_rx,
                pf,
                &output_func,
                &mut decoding_stop_rx,
                &encodings,
                &decoder_screen,
                &decoder_desktop,
            )
            .await;
            // Release the network worker and fail any pending resize before waiting
            // for output capacity.
            drop(conn_ch_rx);
            decoder_desktop.close();
            if let Err(error) = result {
                output::report_error(error, &output_ch_tx, &mut decoding_stop_rx).await;
            }
            trace!("Decoding thread stops");
        });

        // start the traffic process thread
        spawn(async move {
            trace!("Net Connection thread starts");
            let _ =
                async_connection_process_loop(stream, input_ch_rx, conn_ch_tx, net_conn_stop_rx)
                    .await;
            network_desktop.close();
            trace!("Net Connection thread stops");
        });

        info!("VNC Client {name} starts");
        Ok(Self {
            name,
            screen,
            desktop,
            input_ch: input_ch_tx,
            output_ch: output_ch_rx,
            decoding_stop: Some(decoding_stop_tx),
            net_conn_stop: Some(net_conn_stop_tx),
            closed: false,
        })
    }

    fn input_message(&self, event: X11Event) -> Result<ClientMsg, VncError> {
        if self.closed {
            Err(VncError::ClientNotRunning)
        } else {
            let (width, height) = unpack_screen(self.screen.load(Ordering::Acquire));
            let msg = match event {
                X11Event::Refresh => ClientMsg::FramebufferUpdateRequest(
                    Rect {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    },
                    1,
                ),
                X11Event::FullRefresh => ClientMsg::FramebufferUpdateRequest(
                    Rect {
                        x: 0,
                        y: 0,
                        width,
                        height,
                    },
                    0, // non-incremental: server sends entire framebuffer
                ),
                X11Event::KeyEvent(key) => ClientMsg::KeyEvent(key.keycode, key.down),
                X11Event::PointerEvent(mouse) => {
                    ClientMsg::PointerEvent(mouse.position_x, mouse.position_y, mouse.bottons)
                }
                X11Event::CopyText(text) => {
                    if text.len() > crate::limits::MAX_TEXT {
                        return Err(VncError::InvalidImageData);
                    }
                    ClientMsg::ClientCutText(text)
                }
            };
            Ok(msg)
        }
    }

    async fn recv_event(&mut self) -> Result<VncEvent, VncError> {
        if self.closed {
            Err(VncError::ClientNotRunning)
        } else {
            match self.output_ch.recv().await {
                Some(e) => Ok(e),
                None => {
                    self.closed = true;
                    Err(VncError::ClientNotRunning)
                }
            }
        }
    }

    async fn poll_event(&mut self) -> Result<Option<VncEvent>, VncError> {
        if self.closed {
            Err(VncError::ClientNotRunning)
        } else {
            match self.output_ch.try_recv() {
                Err(TryRecvError::Disconnected) => {
                    self.closed = true;
                    Err(VncError::ClientNotRunning)
                }
                Err(TryRecvError::Empty) => Ok(None),
                Ok(e) => Ok(Some(e)),
            }
            // Ok(self.output_ch.recv().await)
        }
    }

    /// Stop the VNC engine and release resources
    ///
    fn close(&mut self) -> Result<(), VncError> {
        self.desktop.close();
        if self.net_conn_stop.is_some() {
            let net_conn_stop: oneshot::Sender<()> = self.net_conn_stop.take().unwrap();
            let _ = net_conn_stop.send(());
        }
        if self.decoding_stop.is_some() {
            let decoding_stop = self.decoding_stop.take().unwrap();
            let _ = decoding_stop.send(());
        }
        self.closed = true;
        Ok(())
    }
}

impl Drop for VncInner {
    fn drop(&mut self) {
        info!("VNC Client {} stops", self.name);
        let _ = self.close();
    }
}

pub struct VncClient {
    server_name: Arc<str>,
    inner: Arc<Mutex<VncInner>>,
    desktop: DesktopClient,
}

impl VncClient {
    pub(super) async fn new<S>(
        stream: S,
        shared: bool,
        pixel_format: Option<PixelFormat>,
        encodings: Vec<VncEncoding>,
    ) -> Result<Self, VncError>
    where
        S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    {
        let inner = VncInner::new(stream, shared, pixel_format, encodings).await?;
        Ok(Self {
            server_name: Arc::from(inner.name.as_str()),
            desktop: inner.desktop.client(&inner.input_ch),
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Desktop name advertised during the initial server handshake.
    /// This is a server-supplied label, not an authenticated machine identity.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Input a `X11Event` from the frontend
    ///
    pub async fn input(&self, event: X11Event) -> Result<(), VncError> {
        let sender = {
            let inner = self.inner.lock().await;
            if inner.closed {
                return Err(VncError::ClientNotRunning);
            }
            inner.input_ch.clone()
        };
        // Do not hold the client mutex while backpressure waits: close needs it.
        let permit = sender.reserve().await?;
        let inner = self.inner.lock().await;
        permit.send(inner.input_message(event)?);
        Ok(())
    }

    /// Receive a `VncEvent` from the engine
    /// This function will block until a `VncEvent` is received
    ///
    pub async fn recv_event(&self) -> Result<VncEvent, VncError> {
        self.inner.lock().await.recv_event().await
    }

    /// polling `VncEvent` from the engine and give it to the client
    ///
    pub async fn poll_event(&self) -> Result<Option<VncEvent>, VncError> {
        self.inner.lock().await.poll_event().await
    }

    /// Stop the VNC engine and release resources
    ///
    pub async fn close(&self) -> Result<(), VncError> {
        self.inner.lock().await.close()
    }
}

impl Clone for VncClient {
    fn clone(&self) -> Self {
        Self {
            server_name: Arc::clone(&self.server_name),
            inner: self.inner.clone(),
            desktop: self.desktop.clone(),
        }
    }
}

async fn send_client_init<S>(stream: &mut S, shared: bool) -> Result<(), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    trace!("Send shared flag: {}", shared);
    stream.write_u8(shared as u8).await?;
    Ok(())
}

async fn read_server_init<S, F, Fut>(
    stream: &mut S,
    pf: &mut Option<PixelFormat>,
    output_func: &F,
) -> Result<(String, (u16, u16)), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    F: Fn(VncEvent) -> Fut,
    Fut: Future<Output = Result<(), VncError>>,
{
    // +--------------+--------------+------------------------------+
    // | No. of bytes | Type [Value] | Description                  |
    // +--------------+--------------+------------------------------+
    // | 2            | U16          | framebuffer-width in pixels  |
    // | 2            | U16          | framebuffer-height in pixels |
    // | 16           | PIXEL_FORMAT | server-pixel-format          |
    // | 4            | U32          | name-length                  |
    // | name-length  | U8 array     | name-string                  |
    // +--------------+--------------+------------------------------+

    let screen_width = stream.read_u16().await?;
    let screen_height = stream.read_u16().await?;
    crate::limits::dimensions(screen_width, screen_height)?;
    let mut send_our_pf = false;

    output_func(VncEvent::SetResolution(
        (screen_width, screen_height).into(),
    ))
    .await?;

    let pixel_format = PixelFormat::read(stream).await?;
    if pf.is_none() {
        output_func(VncEvent::SetPixelFormat(pixel_format)).await?;
        let _ = pf.insert(pixel_format);
    } else {
        send_our_pf = true;
    }

    let name = crate::limits::string(stream, crate::limits::MAX_NAME).await?;

    if send_our_pf {
        trace!("Send customized pixel format {:#?}", pf);
        ClientMsg::SetPixelFormat(*pf.as_ref().unwrap())
            .write(stream)
            .await?;
    }
    Ok((name, (screen_width, screen_height)))
}

async fn send_client_encoding<S>(
    stream: &mut S,
    encodings: Vec<VncEncoding>,
) -> Result<(), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    ClientMsg::SetEncodings(encodings).write(stream).await?;
    Ok(())
}

async fn asycn_vnc_read_loop<S, F, Fut>(
    stream: &mut S,
    pf: &PixelFormat,
    output_func: &F,
    stop_ch: &mut oneshot::Receiver<()>,
    encodings: &[VncEncoding],
    screen: &AtomicU32,
    desktop: &Desktop,
) -> Result<(), VncError>
where
    S: AsyncRead + Unpin,
    F: Fn(VncEvent) -> Fut,
    Fut: Future<Output = Result<(), VncError>>,
{
    tokio::select! {
        biased;
        _ = stop_ch => Ok(()),
        result = read_vnc_messages(stream, pf, output_func, encodings, screen, desktop) => result,
    }
}

async fn read_vnc_messages<S, F, Fut>(
    stream: &mut S,
    pf: &PixelFormat,
    output_func: &F,
    encodings: &[VncEncoding],
    shared_screen: &AtomicU32,
    desktop: &Desktop,
) -> Result<(), VncError>
where
    S: AsyncRead + Unpin,
    F: Fn(VncEvent) -> Fut,
    Fut: Future<Output = Result<(), VncError>>,
{
    let mut decoders = codec::Decoders::new(shared_screen, desktop);

    // main decoding loop
    loop {
        let server_msg = ServerMsg::read(stream).await?;
        trace!("Server message got: {:?}", server_msg);
        match server_msg {
            ServerMsg::FramebufferUpdate(rect_num) => {
                decoders
                    .update(rect_num, pf, stream, output_func, encodings)
                    .await?;
            }
            // SetColorMapEntries,
            ServerMsg::Bell => {
                output_func(VncEvent::Bell).await?;
            }
            ServerMsg::ServerCutText(text) => {
                output_func(VncEvent::Text(text)).await?;
            }
        }
    }
}

async fn async_connection_process_loop<S>(
    mut stream: S,
    mut input_ch: Receiver<ClientMsg>,
    conn_ch: Sender<std::io::Result<Vec<u8>>>,
    mut stop_ch: oneshot::Receiver<()>,
) -> Result<(), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut buffer = [0; 65535];
    let mut pending = 0;

    loop {
        tokio::select! {
            _ = &mut stop_ch => break,
            _ = conn_ch.closed() => break,
            permit = conn_ch.reserve(), if pending > 0 => {
                match permit {
                    Ok(permit) => {
                        permit.send(Ok(buffer[..pending].to_vec()));
                        pending = 0;
                    }
                    Err(_) => break,
                }
            }
            result = stream.read(&mut buffer), if pending == 0 => {
                match result {
                    Ok(0) | Err(_) => break,
                    Ok(length) => pending = length,
                }
            }
            message = input_ch.recv() => {
                let Some(message) = message else { break; };
                tokio::select! {
                    biased;
                    _ = &mut stop_ch => break,
                    result = message.write(&mut stream) => result?,
                }
            }
        }
    }
    // Dropping the bridge signals EOF without blocking shutdown on a full queue.
    Ok(())
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod queue_tests;

#[cfg(all(
    test,
    feature = "desktop-resize",
    feature = "cursor",
    feature = "encoding-zrle",
    feature = "auth-vnc"
))]
mod resize_tests;

#[cfg(all(
    test,
    feature = "desktop-resize",
    feature = "encoding-zrle",
    feature = "auth-vnc",
    feature = "cursor"
))]
mod layout_order_tests;
