//! Tunnels to this computer's loopback (ADR-201), like `ssh -L`: a Sirus window on another
//! computer opens `GET /api/tunnel?port=N` over the authenticated remote socket with its device
//! token, and the bytes of one TCP connection travel as binary WebSocket frames between it and
//! `127.0.0.1:N` (or `[::1]:N`) here. That is how the Mac's browser panel shows the dev server an
//! agent started on a headless host, with its cookies, absolute URLs and HMR sockets intact.
//!
//! Only loopback is reachable, only ports 1024 to 65535, only for a device that paired as a Sirus
//! window, and at most `MAX_TUNNELS` at once. The route lives in `remote.rs`.

use std::future::Future;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// The lowest port a tunnel may reach: privileged services stay out of reach.
pub const MIN_PORT: u16 = 1024;
/// Tunnels open at once across every device; a browser opens a handful per page.
const MAX_TUNNELS: usize = 64;
/// The most bytes one frame carries.
const CHUNK: usize = 64 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

static OPEN: AtomicUsize = AtomicUsize::new(0);

/// The `port` of a tunnel request: digits only, 1024 to 65535.
pub fn valid_port(text: &str) -> Option<u16> {
    if text.is_empty() || text.len() > 5 || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|port| *port >= MIN_PORT)
}

/// One of the `MAX_TUNNELS` places, given back when dropped.
pub struct Permit(());

impl Permit {
    pub fn take() -> Option<Permit> {
        OPEN.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |open| {
            (open < MAX_TUNNELS).then_some(open + 1)
        })
        .ok()
        .map(|_| Permit(()))
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        OPEN.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Connects to `port` on this computer's loopback, IPv4 first: a dev server bound to
/// `localhost` may listen on either.
pub async fn connect(port: u16) -> Option<tokio::net::TcpStream> {
    for address in [
        std::net::SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        std::net::SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ] {
        if let Ok(Ok(stream)) =
            tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(address)).await
        {
            let _ = stream.set_nodelay(true);
            return Some(stream);
        }
    }
    None
}

/// What arrived on the socket side of a tunnel.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    Data(Vec<u8>),
    /// Text, pings and pongs: not part of the stream.
    Other,
    End,
}

/// The socket side of a tunnel: the WebSocket, or a fake in tests.
pub(crate) trait Frames {
    async fn next(&mut self) -> Frame;
    async fn send(&mut self, bytes: Vec<u8>) -> bool;
    async fn close(&mut self);
}

impl Frames for WebSocket {
    async fn next(&mut self) -> Frame {
        match self.recv().await {
            Some(Ok(Message::Binary(bytes))) => Frame::Data(bytes.to_vec()),
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => Frame::End,
            Some(Ok(_)) => Frame::Other,
        }
    }

    async fn send(&mut self, bytes: Vec<u8>) -> bool {
        WebSocket::send(self, Message::Binary(bytes.into()))
            .await
            .is_ok()
    }

    async fn close(&mut self) {
        let _ = WebSocket::send(self, Message::Close(None)).await;
    }
}

/// Moves bytes both ways until either side ends or `stop` resolves; then both are closed.
pub(crate) async fn pipe<F, T>(mut frames: F, target: T, stop: impl Future<Output = ()>)
where
    F: Frames,
    T: AsyncRead + AsyncWrite,
{
    let (mut read, mut write) = tokio::io::split(target);
    let mut buffer = vec![0u8; CHUNK];
    tokio::pin!(stop);
    loop {
        tokio::select! {
            frame = frames.next() => match frame {
                Frame::Data(bytes) => {
                    if write.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Frame::Other => {}
                Frame::End => break,
            },
            read = read.read(&mut buffer) => match read {
                Ok(0) | Err(_) => break,
                Ok(count) => {
                    if !frames.send(buffer[..count].to_vec()).await {
                        break;
                    }
                }
            },
            _ = &mut stop => break,
        }
    }
    let _ = write.shutdown().await;
    frames.close().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[test]
    fn ports_are_unprivileged_numbers_only() {
        assert_eq!(valid_port("5173"), Some(5173));
        assert_eq!(valid_port("1024"), Some(1024));
        assert_eq!(valid_port("65535"), Some(65535));
        for bad in [
            "", "0", "80", "1023", "65536", "99999", "-1", "+5173", " 5173", "5173a", "0x50",
            "000080", "123456",
        ] {
            assert_eq!(valid_port(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn permits_are_bounded_and_given_back() {
        let held: Vec<Permit> = std::iter::from_fn(Permit::take).take(1000).collect();
        assert!(held.len() <= MAX_TUNNELS);
        assert!(Permit::take().is_none());
        drop(held);
        assert!(Permit::take().is_some());
    }

    struct Fake {
        incoming: mpsc::Receiver<Frame>,
        outgoing: mpsc::Sender<Vec<u8>>,
        closed: mpsc::Sender<()>,
    }

    impl Frames for Fake {
        async fn next(&mut self) -> Frame {
            self.incoming.recv().await.unwrap_or(Frame::End)
        }
        async fn send(&mut self, bytes: Vec<u8>) -> bool {
            self.outgoing.send(bytes).await.is_ok()
        }
        async fn close(&mut self) {
            let _ = self.closed.send(()).await;
        }
    }

    fn fake() -> (
        Fake,
        mpsc::Sender<Frame>,
        mpsc::Receiver<Vec<u8>>,
        mpsc::Receiver<()>,
    ) {
        let (to_pipe, incoming) = mpsc::channel(8);
        let (outgoing, from_pipe) = mpsc::channel(8);
        let (closed, closes) = mpsc::channel(1);
        (
            Fake {
                incoming,
                outgoing,
                closed,
            },
            to_pipe,
            from_pipe,
            closes,
        )
    }

    #[tokio::test]
    async fn bytes_cross_both_ways_and_the_end_closes_both_sides() {
        let (frames, to_pipe, mut from_pipe, mut closes) = fake();
        let (target, mut server) = tokio::io::duplex(1024);
        let piping = tokio::spawn(pipe(frames, target, std::future::pending()));

        to_pipe
            .send(Frame::Data(b"GET / HTTP/1.1\r\n".to_vec()))
            .await
            .unwrap();
        to_pipe.send(Frame::Other).await.unwrap();
        let mut request = [0u8; 16];
        server.read_exact(&mut request).await.unwrap();
        assert_eq!(&request, b"GET / HTTP/1.1\r\n");

        server.write_all(b"HTTP/1.1 200 OK").await.unwrap();
        assert_eq!(from_pipe.recv().await.unwrap(), b"HTTP/1.1 200 OK");

        drop(to_pipe);
        piping.await.unwrap();
        assert!(closes.recv().await.is_some());
        let mut rest = Vec::new();
        server.read_to_end(&mut rest).await.unwrap();
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn a_closed_target_or_a_stop_ends_the_tunnel() {
        let (frames, _to_pipe, _from_pipe, mut closes) = fake();
        let (target, server) = tokio::io::duplex(64);
        let piping = tokio::spawn(pipe(frames, target, std::future::pending()));
        drop(server);
        piping.await.unwrap();
        assert!(closes.recv().await.is_some());

        let (frames, _to_pipe, _from_pipe, mut closes) = fake();
        let (target, _server) = tokio::io::duplex(64);
        let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
        let piping = tokio::spawn(pipe(frames, target, async move {
            let _ = stopped.await;
        }));
        stop.send(()).unwrap();
        piping.await.unwrap();
        assert!(closes.recv().await.is_some());
    }

    #[tokio::test]
    async fn only_loopback_listeners_are_reached() {
        let listener = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(connect(port).await.is_some());
        drop(listener);
    }
}
