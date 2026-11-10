//! The TLS side of other computers (ADR-133): an HTTPS pairing request and the `wss://` socket,
//! both on rustls with ring and the system's roots (an HTTPS name has a public
//! certificate). `tungstenite` is handed the finished stream, so it needs no TLS feature.
use crate::i18n::t;
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub type TlsStream = StreamOwned<ClientConnection, TcpStream>;

/// Where another computer answers: an HTTPS host and port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub host: String,
    pub port: u16,
}

impl Address {
    /// Accepts `https://name.example.com`, `name.example.com` or `name.example.com:8443`, with or without a
    /// trailing path. Plain `http://` is refused: the token never travels unencrypted.
    pub fn parse(input: &str) -> Result<Address, String> {
        let text = input.trim();
        let rest = match text.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => rest,
            Some(_) => return Err(t("O endereço precisa ser https.").into()),
            None => text,
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().map_err(|_| t("Porta inválida."))?),
            None => (authority, 443),
        };
        let valid = !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
        if !valid || port == 0 {
            return Err(t("Endereço inválido.").into());
        }
        Ok(Address {
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    /// The address in the form it is stored and shown: `https://host`, with a port only when it
    /// is not 443.
    pub fn display(&self) -> String {
        if self.port == 443 {
            format!("https://{}", self.host)
        } else {
            format!("https://{}:{}", self.host, self.port)
        }
    }
}

fn config() -> Result<Arc<ClientConfig>, String> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            let loaded = rustls_native_certs::load_native_certs();
            roots.add_parsable_certificates(loaded.certs);
            if roots.is_empty() {
                return Err(t("Não encontrei certificados do sistema.").into());
            }
            let provider = Arc::new(rustls::crypto::ring::default_provider());
            ClientConfig::builder_with_provider(provider)
                .with_safe_default_protocol_versions()
                .map_err(|e| e.to_string())
                .map(|builder| {
                    Arc::new(builder.with_root_certificates(roots).with_no_client_auth())
                })
        })
        .clone()
}

/// A TLS connection to the computer, with the name checked against the certificate.
pub fn connect(address: &Address, timeout: Duration) -> Result<TlsStream, String> {
    let target = (address.host.as_str(), address.port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or(t("O endereço não resolve."))?;
    let tcp = TcpStream::connect_timeout(&target, timeout).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let name =
        rustls::pki_types::ServerName::try_from(address.host.clone()).map_err(|e| e.to_string())?;
    let connection = ClientConnection::new(config()?, name).map_err(|e| e.to_string())?;
    Ok(StreamOwned::new(connection, tcp))
}

/// Opens `wss://host/api/socket`; the caller sends `hello` with the token.
pub fn open_socket(address: &Address) -> Result<tungstenite::WebSocket<TlsStream>, String> {
    let stream = connect(address, Duration::from_secs(10))?;
    let url = format!("wss://{}/api/socket", host_header(address));
    let (socket, _) = tungstenite::client(url, stream).map_err(|e| e.to_string())?;
    Ok(socket)
}

/// Opens `wss://host/api/tunnel?port=N` (ADR-201) with the device token as a bearer header;
/// the host connects its loopback port first, so a closed port fails the handshake.
pub fn open_tunnel(
    address: &Address,
    token: &str,
    port: u16,
) -> Result<tungstenite::WebSocket<TlsStream>, String> {
    use tungstenite::client::IntoClientRequest;
    let stream = connect(address, Duration::from_secs(10))?;
    let mut request = format!("wss://{}/api/tunnel?port={port}", host_header(address))
        .into_client_request()
        .map_err(|e| e.to_string())?;
    let bearer = tungstenite::http::HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|e| e.to_string())?;
    request
        .headers_mut()
        .insert(tungstenite::http::header::AUTHORIZATION, bearer);
    let (socket, _) = tungstenite::client(request, stream).map_err(|e| e.to_string())?;
    Ok(socket)
}

fn host_header(address: &Address) -> String {
    if address.port == 443 {
        address.host.clone()
    } else {
        format!("{}:{}", address.host, address.port)
    }
}

/// One JSON `POST`, answered with the status and the body (`Connection: close`).
pub fn post_json(address: &Address, path: &str, body: &str) -> Result<(u16, String), String> {
    let mut stream = connect(address, Duration::from_secs(10))?;
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        host_header(address),
        body.len()
    )
    .map_err(|e| e.to_string())?;
    let mut raw = Vec::new();
    // A server that closes without a TLS close_notify ends the read with an error after the
    // body; what arrived is still a whole reply when the framing says so.
    let _ = stream.read_to_end(&mut raw);
    parse_response(&raw)
}

fn parse_response(raw: &[u8]) -> Result<(u16, String), String> {
    let split = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .ok_or(t("Resposta incompleta."))?;
    let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or(t("Resposta inválida."))?;
    let body = &raw[split + 4..];
    let body = if head.contains("transfer-encoding: chunked") {
        dechunk(body)?
    } else {
        body.to_vec()
    };
    Ok((status, String::from_utf8_lossy(&body).into_owned()))
}

fn dechunk(mut data: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    loop {
        let line_end = data
            .windows(2)
            .position(|w| w == b"\r\n")
            .ok_or(t("Resposta incompleta."))?;
        let size_text = String::from_utf8_lossy(&data[..line_end]);
        let size = usize::from_str_radix(size_text.split(';').next().unwrap_or("").trim(), 16)
            .map_err(|_| t("Resposta inválida."))?;
        data = &data[line_end + 2..];
        if size == 0 {
            return Ok(out);
        }
        if data.len() < size + 2 {
            return Err(t("Resposta incompleta.").into());
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size + 2..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_normalised_and_http_is_refused() {
        let a = Address::parse(" https://VPS.example.com/ ").unwrap();
        assert_eq!((a.host.as_str(), a.port), ("vps.example.com", 443));
        assert_eq!(a.display(), "https://vps.example.com");
        let b = Address::parse("vps.example.com:8443").unwrap();
        assert_eq!(b.display(), "https://vps.example.com:8443");
        assert!(Address::parse("http://vps.example.com").is_err());
        assert!(Address::parse("").is_err());
        assert!(Address::parse("https://bad host").is_err());
        assert!(Address::parse("host:0").is_err());
        assert!(Address::parse("host:abc").is_err());
    }

    #[test]
    fn replies_are_read_plain_or_chunked() {
        let plain = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        assert_eq!(parse_response(plain).unwrap(), (200, "{}".into()));
        let chunked =
            b"HTTP/1.1 400 Bad\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2\r\nde\r\n0\r\n\r\n";
        assert_eq!(parse_response(chunked).unwrap(), (400, "abcde".into()));
        assert!(parse_response(b"HTTP/1.1 200 OK\r\n").is_err());
        assert!(
            parse_response(b"HTTP/1.1 200\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nab").is_err()
        );
    }
}
