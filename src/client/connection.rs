use futures::TryStreamExt;
use tokio_stream::wrappers::ReceiverStream;

use std::{
    future::Future,
    sync::{
        atomic::{AtomicU32, Ordering},
        Arc,
    },
    vec,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{
        mpsc::{
            channel,
            error::{TryRecvError, TrySendError},
            Receiver, Sender,
        },
        oneshot, Mutex,
    },
};
use tokio_util::compat::*;
use tracing::*;

use crate::{codec, PixelFormat, Rect, VncEncoding, VncError, VncEvent, X11Event};
const CHANNEL_SIZE: usize = 4096;

#[cfg(not(target_arch = "wasm32"))]
use tokio::spawn;
#[cfg(target_arch = "wasm32")]
use wasm_bindgen_futures::spawn_local as spawn;

use super::messages::{ClientMsg, ServerMsg};
use super::resize::DesktopState;

struct ImageRect {
    rect: Rect,
    encoding: VncEncoding,
}

impl From<[u8; 12]> for ImageRect {
    fn from(buf: [u8; 12]) -> Self {
        Self {
            rect: Rect {
                x: ((buf[0] as u16) << 8) | buf[1] as u16,
                y: ((buf[2] as u16) << 8) | buf[3] as u16,
                width: ((buf[4] as u16) << 8) | buf[5] as u16,
                height: ((buf[6] as u16) << 8) | buf[7] as u16,
            },
            encoding: (((buf[8] as u32) << 24)
                | ((buf[9] as u32) << 16)
                | ((buf[10] as u32) << 8)
                | (buf[11] as u32))
                .into(),
        }
    }
}

impl ImageRect {
    async fn read<S>(reader: &mut S) -> Result<Self, VncError>
    where
        S: AsyncRead + Unpin,
    {
        let mut rect_buf = [0_u8; 12];
        reader.read_exact(&mut rect_buf).await?;
        Ok(rect_buf.into())
    }
}

struct VncInner {
    name: String,
    screen: Arc<AtomicU32>,
    desktop: Arc<DesktopState>,
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
        let (conn_ch_tx, conn_ch_rx) = channel(CHANNEL_SIZE);
        let (input_ch_tx, input_ch_rx) = channel(CHANNEL_SIZE);
        let (output_ch_tx, output_ch_rx) = channel(CHANNEL_SIZE);
        let (decoding_stop_tx, decoding_stop_rx) = oneshot::channel();
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

        let screen = Arc::new(AtomicU32::new((u32::from(width) << 16) | u32::from(height)));
        let decoder_screen = Arc::clone(&screen);
        let desktop = Arc::new(DesktopState::default());
        let decoder_desktop = Arc::clone(&desktop);
        let network_desktop = Arc::clone(&desktop);
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
            let result = tokio::select! {
                _ = decoding_stop_rx => Ok(()),
                result = asycn_vnc_read_loop(&mut conn_ch_rx, pf, &output_func, &encodings, &decoder_screen, &decoder_desktop) => result,
            };
            if let Err(e) = result {
                if let VncError::IoError(e) = e {
                    if let std::io::ErrorKind::UnexpectedEof = e.kind() {
                        // this should be a normal case when the network connection disconnects
                        // and we just send an EOF over the inner bridge between the process thread and the decode thread
                        // do nothing here
                    } else {
                        error!("Error occurs during the decoding {:?}", e);
                        let _ = output_func(VncEvent::Error(e.to_string())).await;
                    }
                } else {
                    error!("Error occurs during the decoding {:?}", e);
                    let _ = output_func(VncEvent::Error(e.to_string())).await;
                }
            }
            decoder_desktop.close();
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

    async fn input(&mut self, event: X11Event) -> Result<(), VncError> {
        if self.closed {
            Err(VncError::ClientNotRunning)
        } else {
            let size = self.screen.load(Ordering::Acquire);
            let (width, height) = ((size >> 16) as u16, size as u16);
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
                X11Event::CopyText(text) => ClientMsg::ClientCutText(text),
            };
            self.input_ch.send(msg).await?;
            Ok(())
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
    inner: Arc<Mutex<VncInner>>,
    desktop: Arc<DesktopState>,
    input_ch: Sender<ClientMsg>,
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
            desktop: Arc::clone(&inner.desktop),
            input_ch: inner.input_ch.clone(),
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    /// Last validated ExtendedDesktopSize layout; None means support is unconfirmed.
    pub fn desktop_layout(&self) -> Option<crate::DesktopLayout> {
        self.desktop.layout()
    }

    /// Request a negotiated single-screen resize and wait for the server result.
    /// Available on native targets with a Tokio time driver; wasm supports observation only.
    /// Continue draining events concurrently. Each queue/confirmation wait is bounded
    /// to five seconds. Cancellation after dispatch makes later requests uncertain;
    /// reconnect and observe before retrying. Dispatch includes one incremental one-pixel
    /// update request so servers can deliver the confirmation.
    #[cfg(not(target_arch = "wasm32"))]
    pub async fn resize_desktop(
        &self,
        width: u16,
        height: u16,
    ) -> Result<crate::DesktopLayout, crate::ResizeError> {
        self.desktop.request(&self.input_ch, width, height).await
    }

    /// Input a `X11Event` from the frontend
    ///
    pub async fn input(&self, event: X11Event) -> Result<(), VncError> {
        self.inner.lock().await.input(event).await
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
            inner: self.inner.clone(),
            desktop: Arc::clone(&self.desktop),
            input_ch: self.input_ch.clone(),
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

    let name_len = stream.read_u32().await?;
    let mut name_buf = vec![0_u8; name_len as usize];
    stream.read_exact(&mut name_buf).await?;
    let name = String::from_utf8_lossy(&name_buf).into_owned();

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
    encodings: &[VncEncoding],
    screen: &AtomicU32,
    desktop: &DesktopState,
) -> Result<(), VncError>
where
    S: AsyncRead + Unpin,
    F: Fn(VncEvent) -> Fut,
    Fut: Future<Output = Result<(), VncError>>,
{
    let mut raw_decoder = codec::RawDecoder::new();
    let mut zrle_decoder = codec::ZrleDecoder::new();
    let mut tight_decoder = codec::TightDecoder::new();
    let mut trle_decoder = codec::TrleDecoder::new();
    let mut cursor = codec::CursorDecoder::new();

    // main decoding loop
    loop {
        let server_msg = ServerMsg::read(stream).await?;
        trace!("Server message got: {:?}", server_msg);
        match server_msg {
            ServerMsg::FramebufferUpdate(rect_num) => {
                let mut updates = Vec::new();
                let mut framebuffer_changed = false;
                for _ in 0..rect_num {
                    let rect = ImageRect::read(stream).await?;
                    if !matches!(
                        rect.encoding,
                        VncEncoding::CursorPseudo
                            | VncEncoding::ExtendedDesktopSizePseudo
                            | VncEncoding::LastRectPseudo
                    ) {
                        if !updates.is_empty() {
                            return Err(VncError::InvalidImageData);
                        }
                        framebuffer_changed = true;
                    }
                    match rect.encoding {
                        VncEncoding::Raw => {
                            raw_decoder
                                .decode(pf, &rect.rect, stream, output_func)
                                .await?;
                        }
                        VncEncoding::CopyRect => {
                            let source_x = stream.read_u16().await?;
                            let source_y = stream.read_u16().await?;
                            let mut src_rect = rect.rect;
                            src_rect.x = source_x;
                            src_rect.y = source_y;
                            output_func(VncEvent::Copy(rect.rect, src_rect)).await?;
                        }
                        VncEncoding::Tight => {
                            tight_decoder
                                .decode(pf, &rect.rect, stream, output_func)
                                .await?;
                        }
                        VncEncoding::Trle => {
                            trle_decoder
                                .decode(pf, &rect.rect, stream, output_func)
                                .await?;
                        }
                        VncEncoding::Zrle => {
                            zrle_decoder
                                .decode(pf, &rect.rect, stream, output_func)
                                .await?;
                        }
                        VncEncoding::CursorPseudo => {
                            cursor.decode(pf, &rect.rect, stream, output_func).await?;
                        }
                        VncEncoding::ExtendedDesktopSizePseudo => {
                            // Confirm only after the entire message excludes framebuffer changes.
                            // Cursor metadata may accompany desktop layout updates.
                            if framebuffer_changed
                                || updates.len() >= 16
                                || !encodings.contains(&VncEncoding::ExtendedDesktopSizePseudo)
                            {
                                return Err(VncError::InvalidImageData);
                            }
                            updates.push(crate::DesktopUpdate::read(stream, rect.rect).await?);
                        }
                        VncEncoding::DesktopSizePseudo => {
                            desktop.legacy_resize();
                            screen.store(
                                (u32::from(rect.rect.width) << 16) | u32::from(rect.rect.height),
                                Ordering::Release,
                            );
                            output_func(VncEvent::SetResolution(
                                (rect.rect.width, rect.rect.height).into(),
                            ))
                            .await?;
                        }
                        VncEncoding::LastRectPseudo => {
                            if !updates.is_empty()
                                && !encodings.contains(&VncEncoding::LastRectPseudo)
                            {
                                return Err(VncError::InvalidImageData);
                            }
                            break;
                        }
                    }
                }
                for update in updates {
                    if let Some(layout) = &update.layout {
                        screen.store(
                            (u32::from(layout.width) << 16) | u32::from(layout.height),
                            Ordering::Release,
                        );
                    }
                    desktop.observe(&update);
                    output_func(VncEvent::DesktopUpdate(update)).await?;
                }
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

    // main traffic loop
    loop {
        if pending > 0 {
            match conn_ch.try_send(Ok(buffer[0..pending].to_owned())) {
                Err(TrySendError::Full(_message)) => (),
                Err(TrySendError::Closed(_message)) => break,
                Ok(()) => pending = 0,
            }
        }

        tokio::select! {
            _ = &mut stop_ch => break,
            result = stream.read(&mut buffer), if pending == 0 => {
                match result {
                    Ok(nread) => {
                        if nread > 0 {
                            match conn_ch.try_send(Ok(buffer[0..nread].to_owned())) {
                                Err(TrySendError::Full(_message)) => pending = nread,
                                Err(TrySendError::Closed(_message)) => break,
                                Ok(()) => ()
                            }
                        } else {
                            // According to the tokio's Doc
                            // https://docs.rs/tokio/latest/tokio/io/trait.AsyncRead.html
                            // if nread == 0, then EOF is reached
                            trace!("Net Connection EOF detected");
                            break;
                        }
                    }
                    Err(e) => {
                        error!("{}", e.to_string());
                        break;
                    }
                }
            }
            Some(msg) = input_ch.recv() => {
                msg.write(&mut stream).await?;
            }
        }
    }

    // notify the decoding thread
    let _ = conn_ch
        .send(Err(std::io::Error::from(std::io::ErrorKind::UnexpectedEof)))
        .await;

    Ok(())
}

#[cfg(test)]
mod resize_tests;
