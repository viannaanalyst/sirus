//! Remote access for the headless host (ADR-131). `sirus-host serve` always serves:
//! it listens on loopback by default, for a tunnel such as `cloudflared` in front of it
//! (ADR-155), serves the built UI from a folder, and answers the
//! `sirus-host pair | devices | revoke` commands over a private control socket in
//! the data folder (`host.sock`, `0600`). Pairing codes, device tokens, the peer
//! filter and the Origin check are the Mac app's own (`remote.rs`).

use super::*;

/// The control socket's file name inside the data folder.
pub const CONTROL_SOCKET: &str = "host.sock";
const CONTROL_LIMIT: u64 = 64 * 1024;

pub struct Options {
    pub bind: IpAddr,
    pub port: u16,
    /// The URL to put in pairing links when the host is reached under another name.
    pub url: Option<String>,
    /// The Mermaid and KaTeX files served at `/assets/`.
    pub assets_dir: Option<PathBuf>,
    /// Sirus Native (ADR-189): the window's own listener on this loopback port, always on.
    /// `port` is then the listener for other devices, on only while `remote.json` says so.
    pub local_port: Option<u16>,
    /// Added to this computer's name on the phone's list.
    pub label: Option<String>,
}

/// Whether the window has a listener of its own.
pub fn has_local_listener() -> bool {
    super::has_local_listener()
}

/// Starts serving; returns the addresses a device can open.
pub async fn start_host(app: AppHandle, data_dir: &Path, options: Options) -> Result<Vec<String>> {
    let path = data_dir.join("remote.json");
    let mut config = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Config>(&bytes).ok())
        .unwrap_or_default();
    let windowed = options.local_port.is_some();
    config.port = Some(options.port);
    if windowed {
        if !config.windowed {
            config.enabled = false;
            config.keep_awake = false;
            config.windowed = true;
        }
    } else {
        // A server always serves; with a window, the owner's switch decides.
        config.enabled = true;
        config.keep_awake = false;
    }
    let remote = Arc::new(Remote {
        path,
        config: parking_lot::Mutex::new(config),
        pairing: parking_lot::Mutex::new(None),
        server: parking_lot::Mutex::new(None),
        error: parking_lot::Mutex::new(None),
        revoked: broadcast::channel(16).0,
        awake: parking_lot::Mutex::new(None),
        listen: parking_lot::Mutex::new(Listen {
            bind: Some(options.bind),
            url: options.url,
            assets_dir: options.assets_dir,
            local_port: options.local_port,
            label: options.label,
        }),
    });
    if REMOTE.set(remote.clone()).is_err() {
        return Err(Error::new("remote", "Remote access is already running."));
    }
    save(&remote)?;
    if windowed {
        start_kind(app.clone(), remote.clone(), Kind::Window).await;
        if let Some(error) = remote.error.lock().clone() {
            return Err(Error::new("remote", error));
        }
        sync_awake(&remote);
        // A busy port for other devices is shown in Settings, not a reason to stop the window.
        if remote.config.lock().enabled {
            start(app, remote.clone()).await;
        }
        let running = remote.server.lock().is_some();
        return Ok(if running {
            base_urls(&remote)
        } else {
            Vec::new()
        });
    }
    start(app, remote.clone()).await;
    if let Some(error) = remote.error.lock().clone() {
        return Err(Error::new("remote", error));
    }
    Ok(base_urls(&remote))
}

/// A file under `/assets/` in `dir`; nothing outside the folder is ever read.
pub(super) fn read_asset(dir: &Path, path: &str) -> Option<(Vec<u8>, String)> {
    let root = dir.canonicalize().ok()?;
    let relative = path.strip_prefix("/assets/")?;
    let safe = !relative.is_empty()
        && relative.len() <= 512
        && relative.split('/').all(|part| {
            !part.is_empty() && !part.starts_with('.') && !part.contains(['\\', '\0', '%'])
        });
    let file = root.join(relative).canonicalize().ok().filter(|_| safe)?;
    if !file.starts_with(&root) || !file.is_file() {
        return None;
    }
    let bytes = std::fs::read(&file).ok()?;
    Some((bytes, mime_type(&file).into()))
}

fn mime_type(file: &Path) -> &'static str {
    let extension = file
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum ControlRequest {
    Pair,
    /// A code for the window's own pairing: no address, and it works with other devices off.
    PairWindow,
    Status,
    /// How many conversations are in a turn or wait on the person: the updater installs a new
    /// host only at zero, since a restart stops them (ADR-206).
    Busy,
    #[serde(rename_all = "camelCase")]
    Revoke {
        device_id: String,
    },
}

/// Answers `sirus-host pair | devices | revoke` from the same user (the socket is `0600`
/// in the `0700` data folder). One JSON request and one JSON reply per connection.
pub async fn serve_control(app: AppHandle, data_dir: &Path) -> std::io::Result<()> {
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    let socket = data_dir.join(CONTROL_SOCKET);
    let _ = std::fs::remove_file(&socket);
    let listener = tokio::net::UnixListener::bind(&socket)?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
    }
    crate::runtime::async_runtime::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                break;
            };
            let app = app.clone();
            crate::runtime::async_runtime::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut line = String::new();
                if BufReader::new(read.take(CONTROL_LIMIT))
                    .read_line(&mut line)
                    .await
                    .is_err()
                {
                    return;
                }
                let reply = match serde_json::from_str::<ControlRequest>(&line) {
                    Ok(request) => control(app, request).await,
                    Err(error) => Err(Error::new("control", error.to_string())),
                };
                let reply = match reply {
                    Ok(value) => json!({ "ok": true, "value": value }),
                    Err(error) => json!({ "ok": false, "error": error }),
                };
                let _ = write.write_all(format!("{reply}\n").as_bytes()).await;
            });
        }
    });
    Ok(())
}

async fn control(app: AppHandle, request: ControlRequest) -> Result<Value> {
    Ok(match request {
        ControlRequest::Pair => serde_json::to_value(remote_pair().await?)?,
        ControlRequest::PairWindow => json!({ "code": window_code()? }),
        ControlRequest::Status => serde_json::to_value(remote_action(app, Action::Status).await?)?,
        ControlRequest::Busy => {
            let state = app.state::<std::sync::Arc<crate::commands::AppState>>();
            let data = state.data.lock();
            json!({ "turns": busy_turns(data.sessions.iter().map(|session| &session.status)) })
        }
        ControlRequest::Revoke { device_id } => {
            let known = remote()?
                .config
                .lock()
                .devices
                .iter()
                .any(|device| device.id == device_id);
            if !known {
                return Err(Error::not_found("No paired device has this id."));
            }
            serde_json::to_value(remote_action(app, Action::Revoke { device_id }).await?)?
        }
    })
}

/// Conversations a restart would cut: in a turn, or waiting on an approval or a question.
fn busy_turns<'a>(statuses: impl Iterator<Item = &'a crate::models::SessionStatus>) -> usize {
    statuses.filter(|status| status.is_active()).count()
}

/// A one-time code for the window's own device.
fn window_code() -> Result<String> {
    let remote = remote()?;
    let code = pairing_code();
    *remote.pairing.lock() = Some(PairCode {
        hash: hash(&code),
        expires: Instant::now() + PAIR_TTL,
        failures: 0,
    });
    Ok(code)
}

/// The CLI's side: one request to a running `sirus-host serve`. `None` when none runs.
pub fn control_request(
    data_dir: &Path,
    request: &Value,
) -> Option<std::result::Result<Value, Value>> {
    use std::io::{BufRead, Read, Write};
    let mut stream = std::os::unix::net::UnixStream::connect(data_dir.join(CONTROL_SOCKET)).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    stream.write_all(format!("{request}\n").as_bytes()).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(stream.take(CONTROL_LIMIT))
        .read_line(&mut line)
        .ok()?;
    let reply: Value = serde_json::from_str(&line).ok()?;
    Some(if reply["ok"] == true {
        Ok(reply["value"].clone())
    } else {
        Err(reply["error"].clone())
    })
}

/// Paired devices straight from `remote.json`, when no server runs.
pub fn saved_devices(data_dir: &Path) -> Vec<DeviceInfo> {
    std::fs::read(data_dir.join("remote.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Config>(&bytes).ok())
        .unwrap_or_default()
        .devices
        .into_iter()
        .map(|device| DeviceInfo {
            id: device.id,
            name: device.name,
            created_at: device.created_at,
            last_seen: device.last_seen,
        })
        .collect()
}

/// Removes a device from `remote.json` when no server runs. Whether it was there.
pub fn revoke_saved(data_dir: &Path, device_id: &str) -> Result<bool> {
    let path = data_dir.join("remote.json");
    let Some(mut config) = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Config>(&bytes).ok())
    else {
        return Ok(false);
    };
    let before = config.devices.len();
    config.devices.retain(|device| device.id != device_id);
    if config.devices.len() == before {
        return Ok(false);
    }
    let remote = Remote {
        path,
        config: parking_lot::Mutex::new(config),
        pairing: parking_lot::Mutex::new(None),
        server: parking_lot::Mutex::new(None),
        error: parking_lot::Mutex::new(None),
        revoked: broadcast::channel(1).0,
        awake: parking_lot::Mutex::new(None),
        listen: parking_lot::Mutex::new(Listen::default()),
    };
    save(&remote)?;
    Ok(true)
}

/// The pairing link as a QR code for a terminal (two rows per character).
pub fn qr_text(text: &str) -> Option<String> {
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M).ok()?;
    Some(
        code.render::<qrcode::render::unicode::Dense1x2>()
            .dark_color(qrcode::render::unicode::Dense1x2::Light)
            .light_color(qrcode::render::unicode::Dense1x2::Dark)
            .quiet_zone(true)
            .build(),
    )
}

impl DeviceInfo {
    pub fn summary(&self) -> String {
        let seen = self
            .last_seen
            .map(|at| at.format("%Y-%m-%d %H:%M UTC").to_string())
            .unwrap_or_else(|| "never".into());
        format!(
            "{}  {}  paired {}  last seen {}",
            self.id,
            self.name,
            self.created_at.format("%Y-%m-%d"),
            seen
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_turn_or_a_wait_on_the_person_keeps_the_host_busy() {
        use crate::models::SessionStatus::*;
        let statuses = [Idle, Running, Waiting, Completed, Starting, Failed, Stopped];
        assert_eq!(busy_turns(statuses.iter()), 3);
        assert_eq!(busy_turns([Idle, Completed, Stopped].iter()), 0);
    }

    #[test]
    fn assets_stay_inside_the_assets_folder() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("reply-render");
        std::fs::create_dir_all(root.join("chunks")).unwrap();
        std::fs::write(root.join("mermaid.js"), "js").unwrap();
        std::fs::write(root.join("chunks/a.js"), "js").unwrap();
        std::fs::write(root.join("katex.css"), "css").unwrap();
        std::fs::write(dir.path().join("secret.txt"), "secret").unwrap();
        let (bytes, mime) = read_asset(&root, "/assets/mermaid.js").unwrap();
        assert_eq!(
            (bytes.as_slice(), mime.as_str()),
            (&b"js"[..], "text/javascript; charset=utf-8")
        );
        assert!(read_asset(&root, "/assets/chunks/a.js").is_some());
        assert_eq!(
            read_asset(&root, "/assets/katex.css").unwrap().1,
            "text/css; charset=utf-8"
        );
        // Routes and anything outside the folder are missing, never another file.
        for path in [
            "/",
            "/index.html",
            "/sessions/42",
            "/assets/",
            "/assets/missing.js",
            "/assets/../secret.txt",
            "/assets/chunks/../../secret.txt",
            "/assets/%2e%2e/secret.txt",
            "/assets/.env",
        ] {
            assert!(read_asset(&root, path).is_none(), "{path}");
        }
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("secret.txt"), root.join("link.txt"))
                .unwrap();
            assert!(read_asset(&root, "/assets/link.txt").is_none());
        }
    }

    #[test]
    fn saved_devices_can_be_listed_and_revoked_offline() {
        let dir = tempfile::tempdir().unwrap();
        let remote = Remote {
            path: dir.path().join("remote.json"),
            config: parking_lot::Mutex::new(Config::default()),
            pairing: parking_lot::Mutex::new(None),
            server: parking_lot::Mutex::new(None),
            error: parking_lot::Mutex::new(None),
            revoked: broadcast::channel(1).0,
            awake: parking_lot::Mutex::new(None),
            listen: parking_lot::Mutex::new(Listen::default()),
        };
        *remote.pairing.lock() = Some(PairCode {
            hash: hash("123456"),
            expires: Instant::now() + PAIR_TTL,
            failures: 0,
        });
        let (id, _) = redeem(&remote, "123456", "Phone", None).unwrap();
        let devices = saved_devices(dir.path());
        assert_eq!(devices.len(), 1);
        assert!(devices[0].summary().contains("Phone"));
        assert!(!revoke_saved(dir.path(), "other").unwrap());
        assert!(revoke_saved(dir.path(), &id).unwrap());
        assert!(saved_devices(dir.path()).is_empty());
    }

    #[test]
    fn a_host_advertises_only_the_address_it_was_given() {
        let dir = tempfile::tempdir().unwrap();
        let remote = Remote {
            path: dir.path().join("remote.json"),
            config: parking_lot::Mutex::new(Config::default()),
            pairing: parking_lot::Mutex::new(None),
            server: parking_lot::Mutex::new(None),
            error: parking_lot::Mutex::new(None),
            revoked: broadcast::channel(1).0,
            awake: parking_lot::Mutex::new(None),
            listen: parking_lot::Mutex::new(Listen {
                bind: Some("127.0.0.1".parse().unwrap()),
                url: None,
                assets_dir: None,
                ..Listen::default()
            }),
        };
        assert!(base_urls(&remote).is_empty());
        remote.listen.lock().url = Some("https://vps.example.com".into());
        assert_eq!(base_urls(&remote), ["https://vps.example.com"]);
        assert!(qr_text("https://vps.example.com/?pair=123456").is_some());
    }
}
