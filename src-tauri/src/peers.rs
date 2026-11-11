//! Paired computers of this host (ADR-208). An agent on one computer can start and follow work
//! on another one through the host it is paired with: the same `hello` token and `invoke`
//! frames a phone or the window sends, over the same `wss://` address (Tailscale serve). No new
//! port, no new transport. The registry is `peers.json` in the data folder (mode 0600); it is
//! written only by `sirus-host peer add|remove`, never by a command a device can call, and the
//! tokens never leave this module (lists show name and address only).
use rustls::{ClientConfig, ClientConnection, RootCertStore, StreamOwned};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};
use tungstenite::{Message, WebSocket};

pub const FILE: &str = "peers.json";
const MAX_PEERS: usize = 8;
const CALL_TIMEOUT: Duration = Duration::from_secs(90);
/// The one command a peer runs on the other host (`peer_tools.rs`).
const COMMAND: &str = "peer_tool";

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Peer {
    pub id: String,
    pub name: String,
    /// `https://host[:port]`.
    pub address: String,
    pub token: String,
}

impl std::fmt::Debug for Peer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Peer({} at {})", self.name, self.address)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct Registry {
    #[serde(default)]
    peers: Vec<Peer>,
}

pub fn load(dir: &Path) -> Vec<Peer> {
    std::fs::read_to_string(dir.join(FILE))
        .ok()
        .and_then(|text| serde_json::from_str::<Registry>(&text).ok())
        .map(|registry| registry.peers)
        .unwrap_or_default()
}

fn save(dir: &Path, peers: &[Peer]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let text = serde_json::to_string_pretty(&Registry {
        peers: peers.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let temporary = dir.join(format!("{FILE}.tmp"));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    drop(file);
    std::fs::rename(&temporary, dir.join(FILE)).map_err(|e| e.to_string())
}

/// Name and address of each paired computer: what an agent may see.
pub fn public_list(peers: &[Peer]) -> Value {
    json!({
        "computers": peers.iter().map(|peer| json!({
            "id": peer.id, "name": peer.name, "address": peer.address,
        })).collect::<Vec<_>>(),
        "hint": "pass a name or id as `computer` to sirus_delegate, sirus_create_session, sirus_send_to_session, sirus_read_session, sirus_session_status, sirus_delegation_status, sirus_delegation_cancel, sirus_list_sessions or sirus_models; leave it out to stay on this computer",
    })
}

/// The `computer` argument of a tool call: `None` stays on this computer (absent, empty, `here`
/// or `local`), `Some` names a paired one. Anything but a short plain string is refused.
pub fn requested(args: &Value) -> Result<Option<String>, String> {
    let Some(value) = args.get("computer") else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    let text = value.as_str().ok_or("computer must be a name")?.trim();
    if text.is_empty() || text.eq_ignore_ascii_case("here") || text.eq_ignore_ascii_case("local") {
        return Ok(None);
    }
    if text.chars().count() > 64 || text.chars().any(char::is_control) {
        return Err("computer must be a short name; see sirus_list_computers".into());
    }
    Ok(Some(text.to_owned()))
}

/// The paired computer called `wanted` (its id or name, any case).
pub fn resolve<'a>(peers: &'a [Peer], wanted: &str) -> Result<&'a Peer, String> {
    peers
        .iter()
        .find(|peer| peer.id == wanted || peer.name.eq_ignore_ascii_case(wanted))
        .ok_or_else(|| {
            if peers.is_empty() {
                "no computer is paired with this one (the person pairs one with `sirus-host peer add`)"
                    .to_owned()
            } else {
                let names: Vec<&str> = peers.iter().map(|peer| peer.name.as_str()).collect();
                format!(
                    "no paired computer called “{wanted}”; paired: {}",
                    names.join(", ")
                )
            }
        })
}

/// Adds a computer, or refreshes the one at the same address (pairing again after a revoke).
pub fn add(dir: &Path, name: &str, address: &Address, token: &str) -> Result<Peer, String> {
    let mut peers = load(dir);
    let shown = address.display();
    let name = tidy_name(name).unwrap_or_else(|| address.host.clone());
    if let Some(known) = peers.iter_mut().find(|peer| peer.address == shown) {
        known.name = name;
        known.token = token.to_owned();
        let peer = known.clone();
        save(dir, &peers)?;
        return Ok(peer);
    }
    if peers.len() >= MAX_PEERS {
        return Err(format!("at most {MAX_PEERS} computers can be paired"));
    }
    let base = format!("pc-{}", address.host.replace('.', "-"));
    let mut id = base.clone();
    let mut n = 2;
    while peers.iter().any(|peer| peer.id == id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    let peer = Peer {
        id,
        name,
        address: shown,
        token: token.to_owned(),
    };
    peers.push(peer.clone());
    save(dir, &peers)?;
    Ok(peer)
}

pub fn remove(dir: &Path, wanted: &str) -> Result<Peer, String> {
    let mut peers = load(dir);
    let found = resolve(&peers, wanted)?.id.clone();
    let index = peers.iter().position(|peer| peer.id == found).unwrap_or(0);
    let peer = peers.remove(index);
    save(dir, &peers)?;
    Ok(peer)
}

fn tidy_name(name: &str) -> Option<String> {
    let name: String = name
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(48)
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Where another computer answers: an HTTPS host and port.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Address {
    pub host: String,
    pub port: u16,
}

impl Address {
    /// `https://name.ts.net`, `name.ts.net` or `name.ts.net:8443`. Plain `http://` is refused:
    /// the token never travels unencrypted.
    pub fn parse(input: &str) -> Result<Address, String> {
        let text = input.trim();
        let rest = match text.split_once("://") {
            Some((scheme, rest)) if scheme.eq_ignore_ascii_case("https") => rest,
            Some(_) => return Err("the address must be https".into()),
            None => text,
        };
        let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
        let (host, port) = match authority.rsplit_once(':') {
            Some((host, port)) => (host, port.parse::<u16>().map_err(|_| "invalid port")?),
            None => (authority, 443),
        };
        let valid = !host.is_empty()
            && host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.');
        if !valid || port == 0 {
            return Err("invalid address".into());
        }
        Ok(Address {
            host: host.to_ascii_lowercase(),
            port,
        })
    }

    pub fn display(&self) -> String {
        if self.port == 443 {
            format!("https://{}", self.host)
        } else {
            format!("https://{}:{}", self.host, self.port)
        }
    }

    fn host_header(&self) -> String {
        if self.port == 443 {
            self.host.clone()
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// Pairs with the host at `address` using the six-digit code from its `sirus-host pair` and
/// stores the token. Nothing here logs or returns the token.
pub async fn pair(dir: &Path, address: &Address, code: &str) -> Result<Peer, String> {
    let digits: String = code
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err("the code has six digits".into());
    }
    let body = json!({ "code": digits, "name": "Sirus host", "kind": "desktop" }).to_string();
    let response = reqwest::Client::new()
        .post(format!("{}/api/pair", address.display()))
        .header("content-type", "application/json")
        .body(body)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let status = response.status();
    let value: Value = serde_json::from_str(&response.text().await.map_err(|e| e.to_string())?)
        .unwrap_or(Value::Null);
    let token = match value["token"].as_str() {
        Some(token) if status.is_success() => token,
        _ => {
            return Err(value["message"]
                .as_str()
                .unwrap_or("that computer refused the code")
                .to_owned())
        }
    };
    let name = value["host"]["hostName"].as_str().unwrap_or("");
    add(dir, name, address, token)
}

type Tls = StreamOwned<ClientConnection, TcpStream>;

fn tls_config() -> Result<Arc<ClientConfig>, String> {
    static CONFIG: OnceLock<Result<Arc<ClientConfig>, String>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = RootCertStore::empty();
            roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
            if roots.is_empty() {
                return Err("no system certificates found".into());
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

fn open(address: &Address) -> Result<WebSocket<Tls>, String> {
    let timeout = Duration::from_secs(10);
    let target = (address.host.as_str(), address.port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("the address does not resolve")?;
    let tcp = TcpStream::connect_timeout(&target, timeout).map_err(|e| e.to_string())?;
    tcp.set_read_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    tcp.set_write_timeout(Some(timeout))
        .map_err(|e| e.to_string())?;
    let name =
        rustls::pki_types::ServerName::try_from(address.host.clone()).map_err(|e| e.to_string())?;
    let connection = ClientConnection::new(tls_config()?, name).map_err(|e| e.to_string())?;
    let url = format!("wss://{}/api/socket", address.host_header());
    let (socket, _) =
        tungstenite::client(url, StreamOwned::new(connection, tcp)).map_err(|e| e.to_string())?;
    Ok(socket)
}

fn is_idle(error: &tungstenite::Error) -> bool {
    matches!(error, tungstenite::Error::Io(e)
        if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut))
}

/// One command on the other host over a short connection, like the window's `Oneshot`.
pub fn invoke(peer: &Peer, command: &str, args: Value) -> Result<Value, String> {
    let address = Address::parse(&peer.address)?;
    let mut socket = open(&address)?;
    let send = |socket: &mut WebSocket<Tls>, frame: Value| {
        socket
            .send(Message::text(frame.to_string()))
            .map_err(|e| e.to_string())
    };
    send(&mut socket, json!({ "type": "hello", "token": peer.token }))?;
    let deadline = Instant::now() + CALL_TIMEOUT;
    let mut sent = false;
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                let mut message: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                match message["type"].as_str() {
                    Some("ready") if !sent => {
                        sent = true;
                        let frame =
                            json!({ "type": "invoke", "id": 1, "cmd": command, "args": args });
                        send(&mut socket, frame)?;
                    }
                    Some("unauthorized") => {
                        return Err(format!(
                            "{} no longer recognizes this computer; pair again",
                            peer.name
                        ))
                    }
                    Some("result") if message["id"] == 1 => {
                        let _ = socket.close(None);
                        return if message["ok"].as_bool() == Some(true) {
                            Ok(message["value"].take())
                        } else {
                            let error = &message["error"];
                            Err(error["message"]
                                .as_str()
                                .or_else(|| error.as_str())
                                .unwrap_or("the computer refused the request")
                                .to_owned())
                        };
                    }
                    _ => {}
                }
            }
            Ok(Message::Close(_)) => {
                return Err(format!("the connection to {} dropped", peer.name))
            }
            Ok(_) => {}
            Err(error) if is_idle(&error) => {}
            Err(error) => return Err(error.to_string()),
        }
        if Instant::now() > deadline {
            return Err(format!("{} took too long to answer", peer.name));
        }
    }
}

/// Runs `tool` on the paired computer `wanted` for the caller's session, and names the computer
/// in the answer. `request` is the `peer_tool` payload built by the caller.
pub fn forward(dir: &Path, wanted: &str, request: Value) -> Result<Value, String> {
    let peers = load(dir);
    let peer = resolve(&peers, wanted)?;
    let mut reply = invoke(peer, COMMAND, json!({ "request": request }))?;
    if let Some(object) = reply.as_object_mut() {
        object.insert("computer".into(), json!(peer.name));
    }
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peer(id: &str, name: &str) -> Peer {
        Peer {
            id: id.into(),
            name: name.into(),
            address: "https://x.ts.net".into(),
            token: "secret-token".into(),
        }
    }

    #[test]
    fn no_computer_argument_stays_here() {
        assert_eq!(requested(&json!({})).unwrap(), None);
        assert_eq!(requested(&json!({ "computer": null })).unwrap(), None);
        assert_eq!(requested(&json!({ "computer": "  " })).unwrap(), None);
        assert_eq!(requested(&json!({ "computer": "Here" })).unwrap(), None);
        assert_eq!(requested(&json!({ "computer": "local" })).unwrap(), None);
        assert_eq!(
            requested(&json!({ "computer": " sirus-vps " })).unwrap(),
            Some("sirus-vps".into())
        );
    }

    #[test]
    fn a_computer_argument_must_be_a_short_plain_name() {
        assert!(requested(&json!({ "computer": 3 })).is_err());
        assert!(requested(&json!({ "computer": "a\nb" })).is_err());
        assert!(requested(&json!({ "computer": "x".repeat(65) })).is_err());
    }

    #[test]
    fn only_paired_computers_resolve() {
        let peers = vec![peer("pc-a", "Sirus-VPS"), peer("pc-b", "mac")];
        assert_eq!(resolve(&peers, "sirus-vps").unwrap().id, "pc-a");
        assert_eq!(resolve(&peers, "pc-b").unwrap().name, "mac");
        let error = resolve(&peers, "unknown.ts.net").unwrap_err();
        assert!(error.contains("Sirus-VPS, mac"));
        assert!(resolve(&[], "x")
            .unwrap_err()
            .contains("no computer is paired"));
    }

    #[test]
    fn lists_and_debug_never_show_the_token() {
        let peers = vec![peer("pc-a", "vps")];
        assert!(!public_list(&peers).to_string().contains("secret-token"));
        assert!(!format!("{:?}", peers[0]).contains("secret-token"));
    }

    #[test]
    fn addresses_are_https_only() {
        let a = Address::parse("https://Box.ts.net/").unwrap();
        assert_eq!(a.display(), "https://box.ts.net");
        assert_eq!(Address::parse("box.ts.net:8443").unwrap().port, 8443);
        assert!(Address::parse("http://box.ts.net").is_err());
        assert!(Address::parse("https://bad host").is_err());
    }

    #[test]
    fn the_registry_is_private_and_pairing_again_replaces_the_token() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let address = Address::parse("https://vps.ts.net").unwrap();
        let first = add(dir.path(), "vps", &address, "t1").unwrap();
        let again = add(dir.path(), "VPS 2", &address, "t2").unwrap();
        assert_eq!(first.id, again.id);
        let peers = load(dir.path());
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].token, "t2");
        let mode = std::fs::metadata(dir.path().join(FILE))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
        remove(dir.path(), "vps 2").unwrap();
        assert!(load(dir.path()).is_empty());
    }
}
