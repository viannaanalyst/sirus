//! Pages on another computer's `localhost` (ADR-201). An agent on a paired host starts its dev
//! server on that computer's `127.0.0.1:PORT`; when the window shows that computer and a page
//! (the panel's address bar or an agent's `browser_navigate`) asks for `http://localhost:PORT`,
//! this Mac listens on its own `127.0.0.1`, on the same port when nothing here uses it, and every
//! connection accepted there travels to the host's `GET /api/tunnel?port=PORT` as binary
//! WebSocket frames over the authenticated HTTPS connection, like `ssh -L`. The same port keeps
//! cookies, absolute URLs and HMR sockets as the server wrote them; when the port is taken here,
//! another one is used and the address is rewritten, and shown back with the host's port.
//!
//! The listeners belong to the active computer: switching computers or back to this Mac closes
//! them and every tunnel through them. The token stays in this process's memory.
use crate::remote_tls::Address;
use std::collections::HashMap;
use std::io::{ErrorKind, Read, Write};
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener, TcpStream};
use std::os::fd::{AsRawFd, RawFd};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tungstenite::Message;

/// The lowest port tunnelled; the host refuses lower ones too.
const MIN_PORT: u16 = 1024;
/// The most bytes one frame carries.
const CHUNK: usize = 64 * 1024;
/// A quiet tunnel pings this often, so proxies on the way keep an idle HMR socket open.
const KEEPALIVE: Duration = Duration::from_secs(30);

struct Active {
    address: Address,
    token: String,
    stop: Arc<AtomicBool>,
    /// The host's port → the port listening here.
    ports: HashMap<u16, u16>,
    /// Where each listener can be reached, to wake its accept loop when closing.
    bound: Vec<SocketAddr>,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

/// The window now shows the computer at `address`: loopback pages go through it.
pub fn activate(address: Address, token: String) {
    deactivate();
    *ACTIVE.lock().unwrap() = Some(Active {
        address,
        token,
        stop: Arc::new(AtomicBool::new(false)),
        ports: HashMap::new(),
        bound: Vec::new(),
    });
}

/// Back to this Mac (or to another computer): every listener and tunnel closes.
pub fn deactivate() {
    let Some(active) = ACTIVE.lock().unwrap().take() else {
        return;
    };
    active.stop.store(true, Ordering::SeqCst);
    for address in active.bound {
        // The accept loops sleep in `accept`; a connection wakes them to see the stop.
        let _ = TcpStream::connect_timeout(&address, Duration::from_millis(200));
    }
}

/// The address a page loads: on a remote computer, a loopback address with a port goes through
/// a tunnel (opened on first use); anything else is left as it is.
pub fn route(url: &str) -> String {
    let Some((port, span)) = loopback_port(url) else {
        return url.to_owned();
    };
    let mut guard = ACTIVE.lock().unwrap();
    let Some(active) = guard.as_mut() else {
        return url.to_owned();
    };
    let local = match active.ports.get(&port) {
        Some(local) => *local,
        None => match listen(active, port) {
            Some(local) => local,
            None => return url.to_owned(),
        },
    };
    with_port(url, span, local)
}

/// The address a tab shows and an agent reads: a rewritten port is shown as the host's.
pub fn shown(url: &str) -> String {
    let guard = ACTIVE.lock().unwrap();
    match guard.as_ref() {
        Some(active) => shown_with(&active.ports, url),
        None => url.to_owned(),
    }
}

fn shown_with(ports: &HashMap<u16, u16>, url: &str) -> String {
    let Some((port, span)) = loopback_port(url) else {
        return url.to_owned();
    };
    match ports
        .iter()
        .find(|(remote, local)| **local == port && **remote != port)
    {
        Some((remote, _)) => with_port(url, span, *remote),
        None => url.to_owned(),
    }
}

/// The explicit port of an `http(s)://` address on a loopback name, and where it is written.
/// `localhost`, `127.0.0.1` and `[::1]` count; a default port (80, 443) or one under 1024 does not.
pub fn loopback_port(url: &str) -> Option<(u16, std::ops::Range<usize>)> {
    let lower = url.to_ascii_lowercase();
    let start = if lower.starts_with("http://") {
        7
    } else if lower.starts_with("https://") {
        8
    } else {
        return None;
    };
    let end = lower[start..]
        .find(['/', '?', '#'])
        .map_or(lower.len(), |at| start + at);
    let authority = &lower[start..end];
    if authority.contains('@') {
        return None;
    }
    let colon = authority.rfind(':')?;
    let (host, port) = (&authority[..colon], &authority[colon + 1..]);
    if !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return None;
    }
    if port.is_empty() || port.len() > 5 || !port.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let number = port.parse::<u16>().ok().filter(|p| *p >= MIN_PORT)?;
    let from = start + colon + 1;
    Some((number, from..end))
}

fn with_port(url: &str, span: std::ops::Range<usize>, port: u16) -> String {
    format!("{}{port}{}", &url[..span.start], &url[span.end..])
}

/// Whether something on this Mac already answers on `port`.
fn taken_here(port: u16) -> bool {
    [
        SocketAddr::from((Ipv4Addr::LOCALHOST, port)),
        SocketAddr::from((Ipv6Addr::LOCALHOST, port)),
    ]
    .iter()
    .any(|address| TcpStream::connect_timeout(address, Duration::from_millis(150)).is_ok())
}

/// Listens here for the host's `port`: the same number when this Mac does not use it, else any.
fn listen(active: &mut Active, port: u16) -> Option<u16> {
    let same = (!taken_here(port))
        .then(|| TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok())
        .flatten();
    let listener = match same {
        Some(listener) => listener,
        None => TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).ok()?,
    };
    let local = listener.local_addr().ok()?.port();
    let mut listeners = vec![listener];
    // `localhost` may resolve to `::1` first; the same port there when free.
    if local == port
        && let Ok(v6) = TcpListener::bind((Ipv6Addr::LOCALHOST, port))
    {
        listeners.push(v6);
    }
    for listener in listeners {
        if let Ok(address) = listener.local_addr() {
            active.bound.push(address);
        }
        let (address, token, stop) = (
            active.address.clone(),
            active.token.clone(),
            active.stop.clone(),
        );
        std::thread::spawn(move || accept_loop(listener, address, token, port, stop));
    }
    active.ports.insert(port, local);
    Some(local)
}

fn accept_loop(
    listener: TcpListener,
    address: Address,
    token: String,
    port: u16,
    stop: Arc<AtomicBool>,
) {
    for stream in listener.incoming() {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let Ok(stream) = stream else { continue };
        let (address, token, stop) = (address.clone(), token.clone(), stop.clone());
        std::thread::spawn(move || {
            let Ok(mut socket) = crate::remote_tls::open_tunnel(&address, &token, port) else {
                // The host's port is closed (or the host is gone): the page sees a reset.
                return;
            };
            let fd = socket.get_ref().sock.as_raw_fd();
            if socket
                .get_ref()
                .sock
                .set_read_timeout(Some(Duration::from_millis(1)))
                .is_ok()
            {
                let _ = pipe(&mut socket, fd, stream, &stop);
            }
        });
    }
}

/// Waits until `a` or `b` can be read (or `timeout`), as (a ready, b ready).
fn wait(a: RawFd, b: RawFd, timeout: Duration) -> (bool, bool) {
    let mut fds = [
        libc::pollfd {
            fd: a,
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: b,
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    let millis = timeout.as_millis().min(i32::MAX as u128) as libc::c_int;
    let count = unsafe { libc::poll(fds.as_mut_ptr(), 2, millis) };
    if count <= 0 {
        return (false, false);
    }
    let ready =
        |fd: &libc::pollfd| fd.revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0;
    (ready(&fds[0]), ready(&fds[1]))
}

/// Moves bytes between the tunnel's WebSocket (whose stream reads with a short timeout, and
/// whose descriptor is `socket_fd`) and the accepted connection, until either ends or `stop`.
pub fn pipe<S: Read + Write>(
    socket: &mut tungstenite::WebSocket<S>,
    socket_fd: RawFd,
    mut local: TcpStream,
    stop: &AtomicBool,
) -> std::io::Result<()> {
    local.set_nodelay(true)?;
    let local_fd = local.as_raw_fd();
    let mut buffer = vec![0u8; CHUNK];
    let mut sent = Instant::now();
    let result = loop {
        if stop.load(Ordering::SeqCst) {
            break Ok(());
        }
        let (from_socket, from_local) = wait(socket_fd, local_fd, Duration::from_millis(250));
        if from_local {
            match local.read(&mut buffer) {
                Ok(0) => break Ok(()),
                Ok(count) => {
                    if let Err(error) = socket.send(Message::binary(buffer[..count].to_vec())) {
                        break Err(std::io::Error::other(error));
                    }
                    sent = Instant::now();
                }
                Err(error)
                    if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted) => {}
                Err(error) => break Err(error),
            }
        }
        if from_socket {
            // Everything the TLS layer has already decrypted is read before waiting again.
            let ended = loop {
                match socket.read() {
                    Ok(Message::Binary(bytes)) => {
                        if let Err(error) = local.write_all(&bytes) {
                            break Some(Err(error));
                        }
                    }
                    Ok(Message::Close(_)) => break Some(Ok(())),
                    Ok(_) => {}
                    Err(tungstenite::Error::Io(error))
                        if matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) =>
                    {
                        break None;
                    }
                    Err(
                        tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed,
                    ) => {
                        break Some(Ok(()));
                    }
                    Err(error) => break Some(Err(std::io::Error::other(error))),
                }
            };
            if let Some(result) = ended {
                break result;
            }
        }
        if sent.elapsed() > KEEPALIVE {
            sent = Instant::now();
            if let Err(error) = socket.send(Message::Ping(Vec::new().into())) {
                break Err(std::io::Error::other(error));
            }
        }
    };
    let _ = local.shutdown(std::net::Shutdown::Both);
    let _ = socket.close(None);
    let _ = socket.flush();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_loopback_addresses_with_a_high_port_are_tunnelled() {
        fn at(url: &str) -> Option<(u16, &str)> {
            loopback_port(url).map(|(port, span)| (port, &url[span]))
        }
        assert_eq!(at("http://localhost:5173/"), Some((5173, "5173")));
        assert_eq!(at("http://127.0.0.1:3000"), Some((3000, "3000")));
        assert_eq!(at("https://LOCALHOST:8443/a?b#c"), Some((8443, "8443")));
        assert_eq!(at("http://[::1]:4000/x"), Some((4000, "4000")));
        assert_eq!(at("http://localhost:3000?x=1"), Some((3000, "3000")));
        for url in [
            "http://localhost/",
            "http://localhost:80/",
            "http://localhost:1023/",
            "http://localhost:65536/",
            "http://localhost:/",
            "http://localhost:30a0/",
            "http://example.com:3000/",
            "http://localhost.example.com:3000/",
            "http://user@localhost:3000/",
            "http://192.168.1.2:3000/",
            "ftp://localhost:3000/",
            "about:blank",
            "localhost:3000",
        ] {
            assert_eq!(loopback_port(url), None, "{url}");
        }
    }

    #[test]
    fn a_port_taken_here_is_rewritten_and_shown_back_as_the_hosts() {
        let url = "http://localhost:5173/app?x=1#top";
        let (port, span) = loopback_port(url).unwrap();
        assert_eq!(port, 5173);
        assert_eq!(
            with_port(url, span, 61000),
            "http://localhost:61000/app?x=1#top"
        );
        let ports = HashMap::from([(5173, 61000), (3000, 3000)]);
        assert_eq!(
            shown_with(&ports, "http://localhost:61000/app"),
            "http://localhost:5173/app"
        );
        // The same port, or one that is not a tunnel's, reads as it is.
        assert_eq!(
            shown_with(&ports, "http://localhost:3000/"),
            "http://localhost:3000/"
        );
        assert_eq!(
            shown_with(&ports, "http://localhost:4000/"),
            "http://localhost:4000/"
        );
        assert_eq!(
            shown_with(&ports, "https://example.com/"),
            "https://example.com/"
        );
    }

    #[test]
    fn without_a_remote_computer_addresses_are_left_alone() {
        deactivate();
        assert_eq!(route("http://localhost:5173/"), "http://localhost:5173/");
        assert_eq!(shown("http://localhost:61000/"), "http://localhost:61000/");
    }

    /// A host stand-in: accepts one WebSocket and answers each binary frame in capitals.
    fn shouting_host() -> (SocketAddr, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let host = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();
            loop {
                match socket.read() {
                    Ok(Message::Binary(bytes)) => {
                        let loud = bytes.to_ascii_uppercase();
                        socket.send(Message::binary(loud)).unwrap();
                    }
                    Ok(Message::Close(_)) | Err(_) => break,
                    Ok(_) => {}
                }
            }
        });
        (address, host)
    }

    #[test]
    fn bytes_cross_the_tunnel_both_ways_until_the_page_closes() {
        let (host, host_thread) = shouting_host();
        let stream = TcpStream::connect(host).unwrap();
        let (mut socket, _) = tungstenite::client(format!("ws://{host}/"), stream).unwrap();
        socket
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(1)))
            .unwrap();
        let fd = socket.get_ref().as_raw_fd();

        let here = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut page = TcpStream::connect(here.local_addr().unwrap()).unwrap();
        let (accepted, _) = here.accept().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let piping = {
            let stop = stop.clone();
            std::thread::spawn(move || pipe(&mut socket, fd, accepted, &stop))
        };

        page.write_all(b"get / http/1.1").unwrap();
        page.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut answer = [0u8; 14];
        page.read_exact(&mut answer).unwrap();
        assert_eq!(&answer, b"GET / HTTP/1.1");

        drop(page);
        assert!(piping.join().unwrap().is_ok());
        host_thread.join().unwrap();
    }

    #[test]
    fn stopping_ends_a_quiet_tunnel() {
        let (host, host_thread) = shouting_host();
        let stream = TcpStream::connect(host).unwrap();
        let (mut socket, _) = tungstenite::client(format!("ws://{host}/"), stream).unwrap();
        socket
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(1)))
            .unwrap();
        let fd = socket.get_ref().as_raw_fd();
        let here = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let _page = TcpStream::connect(here.local_addr().unwrap()).unwrap();
        let (accepted, _) = here.accept().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let piping = {
            let stop = stop.clone();
            std::thread::spawn(move || pipe(&mut socket, fd, accepted, &stop))
        };
        stop.store(true, Ordering::SeqCst);
        assert!(piping.join().unwrap().is_ok());
        host_thread.join().unwrap();
    }
}
