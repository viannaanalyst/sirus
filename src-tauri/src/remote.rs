//! Remote access (ADR-080): the app can serve its own interface to the person's
//! other devices. Off by default. When on, a small HTTP server listens on one
//! port, answers only loopback (other devices arrive through a tunnel such as `cloudflared`,
//! ADR-155), and serves the bundled UI plus one WebSocket.
//!
//! A device joins with a one-time pairing code (five minutes, single use) and
//! receives its own token; only the token's SHA-256 is kept, in `remote.json`.
//! Over the socket a paired device invokes the same IPC commands the window
//! uses: each call is handed to the main webview's IPC entry point, so the
//! command list, argument parsing and capability checks stay the ones the
//! desktop has. A few commands that only make sense at the Mac (pickers,
//! dictation, computer use) or that manage remote access itself are refused.
//! Events are forwarded by name as the device subscribes to them.
//!
//! The Sirus desktop app pairs cross-origin from its own page (`tauri://localhost`, ADR-133):
//! that origin is recorded with the device it paired, and only recorded origins reach the API
//! and the socket (with CORS); the device token stays mandatory everywhere.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use crate::runtime::{AppHandle, Emitter, Listener, Manager};
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::body::Bytes;
use axum::extract::{ConnectInfo, Request, State as AxumState};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use base64::Engine;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tokio::sync::{broadcast, mpsc, watch};

use crate::error::{Error, Result};

mod app_update;
/// The headless host's side of remote access (ADR-131).
pub mod host;

pub const DEFAULT_PORT: u16 = 7710;
/// Emitted when a device pairs or connects, so Settings → Connections refreshes.
pub const CHANGED: &str = "remote-changed";
const PAIR_TTL: Duration = Duration::from_secs(5 * 60);
const PAIR_FAILURES: u8 = 5;
const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_DEVICES: usize = 10;
const MAX_MESSAGE: usize = 32 * 1024 * 1024;
/// Messages queued for one device before it is considered too slow and dropped.
const QUEUE: usize = 4096;
const MAX_SUBSCRIPTIONS: usize = 64;

/// Commands a device never runs: remote access manages itself only at the Mac,
/// and these act on the Mac's own screen, microphone or file pickers.
const DENIED: &[&str] = &[
    "remote_action",
    "remote_pair",
    "pick_folder",
    "pick_executable",
    "capture_prompt_window",
    "drop_prompt_attachments",
    "dictation_status",
    "dictation_level",
    "start_dictation",
    "stop_dictation",
    "computer_action",
    "window_snap_action",
    "take_prompt_link",
    "window_ready",
    "chat_background_action",
    "vscode_themes_import_local",
    // Updating replaces and restarts this Mac's app (ADR-140).
    "app_update_action",
    // Saved browser logins stay on this Mac (ADR-186).
    "login_vault_list",
    "login_vault_save",
    "login_vault_delete",
    "login_vault_set_agent_use",
    "login_vault_matches",
    "login_vault_prompts",
    "login_vault_answer_offer",
    "login_vault_answer_approval",
    "login_vault_autofill",
    // Connector sign-ins stay on this Mac unless its own window carries them (ADR-137).
    "connector_export_logins",
];

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Config {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    port: Option<u16>,
    #[serde(default)]
    devices: Vec<Device>,
    /// Keep the Mac awake on power while remote access is on (ADR-083).
    #[serde(default)]
    keep_awake: bool,
    /// This computer's id for the phone's list of computers (ADR-132), made once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    host_id: Option<String>,
    /// Written by a host with a window of its own (Sirus Native, ADR-189). Before that the host
    /// forced `enabled` on to serve its window, so a file without this has no real choice in it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    windowed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Device {
    id: String,
    name: String,
    token_hash: String,
    created_at: DateTime<Utc>,
    #[serde(default)]
    last_seen: Option<DateTime<Utc>>,
    /// The page origin of a device that paired from another Sirus's page (ADR-132); only
    /// these origins may call the API cross-origin, and only with this device's token.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    origins: Vec<String>,
    /// `desktop` for a Sirus window that paired from Settings → Conexões (ADR-201); phones and
    /// pages leave it out. Only a desktop may run a headless host's browser or open tunnels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    kind: Option<String>,
}

impl Device {
    /// A Sirus window on another computer. Windows paired before ADR-201 sent no kind, but they
    /// always paired as "Sirus" from no page; the phone pairs as "iPhone · Sirus".
    fn is_desktop(&self) -> bool {
        desktop_kind(self.kind.as_deref(), &self.name, !self.origins.is_empty())
    }
}

/// The pure rule behind `Device::is_desktop`.
fn desktop_kind(kind: Option<&str>, name: &str, from_page: bool) -> bool {
    match kind {
        Some(kind) => kind == "desktop",
        None => !from_page && name == "Sirus",
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    id: String,
    name: String,
    created_at: DateTime<Utc>,
    last_seen: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    enabled: bool,
    running: bool,
    port: u16,
    /// The addresses this computer advertises (`--url`).
    urls: Vec<String>,
    /// The public route's address, when one is set (`remote-public.json`).
    public_url: Option<String>,
    devices: Vec<DeviceInfo>,
    error: Option<String>,
    keep_awake: bool,
    /// The Mac is being kept awake right now (it is, while on power).
    awake: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    code: String,
    /// The addresses above with `?pair=<code>`, ready for a QR code.
    urls: Vec<String>,
    /// `sirus://pair?…` for the iPhone app when a public route is set (`remote-public.json`):
    /// the public address, the code and the route's Cloudflare Access service token.
    #[serde(skip_serializing_if = "Option::is_none")]
    app_url: Option<String>,
    /// The app link, else the first URL, as a QR code (SVG markup).
    qr_svg: String,
    expires_at: DateTime<Utc>,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Action {
    Status,
    SetEnabled {
        enabled: bool,
    },
    #[serde(rename_all = "camelCase")]
    Revoke {
        device_id: String,
    },
    /// Gives a paired device the owner's own name, to tell devices apart in the list.
    #[serde(rename_all = "camelCase")]
    Rename {
        device_id: String,
        name: String,
    },
    SetKeepAwake {
        enabled: bool,
    },
}

struct PairCode {
    hash: String,
    expires: Instant,
    failures: u8,
}

struct Server {
    port: u16,
    shutdown: watch::Sender<bool>,
}

struct Remote {
    path: PathBuf,
    config: parking_lot::Mutex<Config>,
    pairing: parking_lot::Mutex<Option<PairCode>>,
    server: parking_lot::Mutex<Option<Server>>,
    error: parking_lot::Mutex<Option<String>>,
    /// Ids of devices whose access was just removed; their sockets close.
    revoked: broadcast::Sender<String>,
    /// `caffeinate -s` while the Mac should stay awake for paired devices.
    awake: parking_lot::Mutex<Option<std::process::Child>>,
    /// Where the server listens and what it serves; the desktop keeps the defaults.
    listen: parking_lot::Mutex<Listen>,
}

/// The headless host chooses its address, its advertised URL and the UI folder (ADR-131).
#[derive(Debug, Clone, Default)]
struct Listen {
    /// `None` listens on every address and lets `guard` pick the peers (the Mac app).
    bind: Option<IpAddr>,
    /// Shown in pairing links instead of the addresses found on this machine.
    url: Option<String>,
    /// The folder served at `/assets/` (Mermaid and KaTeX for the Sirus window).
    assets_dir: Option<PathBuf>,
    /// Sirus Native (ADR-189): the window's own connection listens here on loopback, always on,
    /// and `enabled` in `remote.json` only controls the listener for other devices.
    local_port: Option<u16>,
    /// Appended to this computer's name for the phone's list ("<Mac> (Sirus Native)").
    label: Option<String>,
}

/// The window's own listener (loopback), beside the one for other devices.
static LOCAL: parking_lot::Mutex<Option<Server>> = parking_lot::const_mutex(None);

/// Whether the window has a listener of its own, separate from the one for other devices.
pub fn has_local_listener() -> bool {
    REMOTE
        .get()
        .is_some_and(|remote| remote.listen.lock().local_port.is_some())
}

/// A headless host (no window of its own) with a Sirus window paired: that window may attach
/// as the browser client, so agents are offered the browser tools (ADR-201).
pub fn desktop_client_possible() -> bool {
    REMOTE.get().is_some_and(|remote| {
        remote.listen.lock().local_port.is_none()
            && remote.config.lock().devices.iter().any(Device::is_desktop)
    })
}

/// Whether the paired device `id` is a Sirus window on another computer (ADR-201).
fn device_is_desktop(remote: &Remote, id: &str) -> bool {
    remote
        .config
        .lock()
        .devices
        .iter()
        .any(|device| device.id == id && device.is_desktop())
}

static REMOTE: OnceLock<Arc<Remote>> = OnceLock::new();

fn remote() -> Result<Arc<Remote>> {
    REMOTE
        .get()
        .cloned()
        .ok_or_else(|| Error::new("remote", "Remote access is not ready yet."))
}

pub fn init(app: AppHandle, data_dir: &Path) {
    let path = data_dir.join("remote.json");
    let config = std::fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Config>(&bytes).ok())
        .unwrap_or_default();
    let enabled = config.enabled;
    let remote = Arc::new(Remote {
        path,
        config: parking_lot::Mutex::new(config),
        pairing: parking_lot::Mutex::new(None),
        server: parking_lot::Mutex::new(None),
        error: parking_lot::Mutex::new(None),
        revoked: broadcast::channel(16).0,
        awake: parking_lot::Mutex::new(None),
        listen: parking_lot::Mutex::new(Listen::default()),
    });
    let _ = REMOTE.set(remote.clone());
    sync_awake(&remote);
    if enabled {
        crate::runtime::async_runtime::spawn(async move {
            start(app, remote).await;
        });
    }
}

pub fn shutdown() {
    if let Some(remote) = REMOTE.get() {
        stop(remote);
        release_awake(remote);
    }
}

/// Paired devices can only reach an awake Mac: while remote access and the switch are on,
/// `caffeinate -s` holds off system sleep on power (the display still sleeps and locks).
/// `-w` releases it if Sirus quits without cleaning up.
fn sync_awake(remote: &Remote) {
    // `caffeinate` is the Mac's; a server stays awake on its own (ADR-131).
    let mac = cfg!(target_os = "macos") && remote.listen.lock().local_port.is_some();
    let wanted = mac && awake_wanted(&remote.config.lock());
    let mut awake = remote.awake.lock();
    if let Some(child) = awake.as_mut() {
        // A caffeinate that ended on its own is started again below.
        if !matches!(child.try_wait(), Ok(None)) {
            *awake = None;
        }
    }
    match (wanted, awake.is_some()) {
        (true, false) => {
            match std::process::Command::new("/usr/bin/caffeinate")
                .args(caffeinate_args(std::process::id()))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(child) => *awake = Some(child),
                Err(error) => tracing::warn!(%error, "cannot keep the Mac awake"),
            }
        }
        (false, true) => {
            if let Some(mut child) = awake.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
        _ => {}
    }
}

fn release_awake(remote: &Remote) {
    if let Some(mut child) = remote.awake.lock().take() {
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn awake_wanted(config: &Config) -> bool {
    config.enabled && config.keep_awake
}

fn caffeinate_args(pid: u32) -> Vec<String> {
    vec!["-s".into(), "-w".into(), pid.to_string()]
}

pub async fn remote_action(app: AppHandle, action: Action) -> Result<Status> {
    let remote = remote()?;
    match action {
        Action::Status => {
            sync_awake(&remote);
        }
        Action::SetEnabled { enabled } => {
            remote.config.lock().enabled = enabled;
            save(&remote)?;
            if enabled {
                start(app, remote.clone()).await;
            } else {
                stop(&remote);
                *remote.pairing.lock() = None;
            }
            sync_awake(&remote);
        }
        Action::SetKeepAwake { enabled } => {
            remote.config.lock().keep_awake = enabled;
            save(&remote)?;
            sync_awake(&remote);
        }
        Action::Rename { device_id, name } => {
            let name = owner_device_name(&name)?;
            refuse_window_device(&remote, &device_id)?;
            let mut config = remote.config.lock();
            let device = config
                .devices
                .iter_mut()
                .find(|device| device.id == device_id)
                .ok_or_else(|| Error::new("remote", "That device is no longer paired."))?;
            device.name = name;
            drop(config);
            save(&remote)?;
        }
        Action::Revoke { device_id } => {
            refuse_window_device(&remote, &device_id)?;
            remote
                .config
                .lock()
                .devices
                .retain(|device| device.id != device_id);
            save(&remote)?;
            let _ = remote.revoked.send(device_id);
        }
    }
    Ok(status(&remote))
}

/// The hash of the token Sirus Native's own window keeps in `native-token` (ADR-189). That
/// window is not a device the owner added: it is never listed, renamed or removed.
fn window_token_hash(remote: &Remote) -> Option<String> {
    remote.listen.lock().local_port?;
    let token = std::fs::read_to_string(remote.path.with_file_name("native-token")).ok()?;
    let token = token.trim();
    (!token.is_empty()).then(|| hash(token))
}

fn refuse_window_device(remote: &Remote, device_id: &str) -> Result<()> {
    let window = window_token_hash(remote);
    let is_window = window.is_some_and(|window| {
        remote
            .config
            .lock()
            .devices
            .iter()
            .any(|device| device.id == device_id && device.token_hash == window)
    });
    if is_window {
        return Err(Error::new(
            "remote",
            "This is the Sirus Native window itself, not a device.",
        ));
    }
    Ok(())
}

/// How many devices count toward the limit: the window's own does not.
fn counted_devices(remote: &Remote, config: &Config) -> usize {
    let window = window_token_hash(remote);
    config
        .devices
        .iter()
        .filter(|device| window.as_deref() != Some(device.token_hash.as_str()))
        .count()
}

/// A device name as the owner typed it: trimmed, one line, at most 60 characters.
fn owner_device_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(Error::new("remote", "Give the device a name."));
    }
    if name.chars().any(char::is_control) {
        return Err(Error::new("remote", "The name must be one line of text."));
    }
    Ok(name.chars().take(60).collect())
}

/// A fresh one-time code; it replaces any earlier one.
pub async fn remote_pair() -> Result<Pairing> {
    let remote = remote()?;
    if remote.server.lock().is_none() {
        return Err(Error::new("remote", "Turn on remote access first."));
    }
    if counted_devices(&remote, &remote.config.lock()) >= MAX_DEVICES {
        return Err(Error::new(
            "remote",
            format!("Up to {MAX_DEVICES} devices can be connected. Remove one first."),
        ));
    }
    let bases = base_urls(&remote);
    let route = public_route(&remote.path);
    if bases.is_empty() && route.is_none() {
        return Err(Error::new(
            "remote",
            "This computer has no address yet. Set its public route in remote-public.json.",
        ));
    }
    let code = pairing_code();
    *remote.pairing.lock() = Some(PairCode {
        hash: hash(&code),
        expires: Instant::now() + PAIR_TTL,
        failures: 0,
    });
    let urls: Vec<String> = bases
        .into_iter()
        .map(|url| format!("{url}/?pair={code}"))
        .collect();
    let app_url = route.map(|route| app_pair_link(&route, &code));
    let qr_svg = qr_svg(
        app_url
            .as_deref()
            .or(urls.first().map(String::as_str))
            .unwrap_or_default(),
    )?;
    Ok(Pairing {
        code,
        urls,
        app_url,
        qr_svg,
        expires_at: Utc::now() + chrono::Duration::from_std(PAIR_TTL).unwrap_or_default(),
    })
}

/// A public route in front of this computer, such as a Cloudflare Tunnel behind Cloudflare
/// Access: `remote-public.json` next to `remote.json`, written by the owner (`url`, and the
/// Access service token's `accessClientId` and `accessClientSecret` the iPhone app sends).
/// Read at pairing time; the device token stays mandatory behind it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PublicRoute {
    url: String,
    #[serde(default)]
    access_client_id: Option<String>,
    #[serde(default)]
    access_client_secret: Option<String>,
}

fn public_route(config_path: &Path) -> Option<PublicRoute> {
    let bytes = std::fs::read(config_path.with_file_name("remote-public.json")).ok()?;
    let mut route: PublicRoute = serde_json::from_slice(&bytes).ok()?;
    let url = route.url.trim().trim_end_matches('/').to_ascii_lowercase();
    let host = url.strip_prefix("https://")?;
    let token_ok = |value: &Option<String>| {
        value.as_deref().is_none_or(|value| {
            !value.is_empty()
                && value.len() <= 200
                && value
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        })
    };
    let valid = !host.is_empty()
        && host.len() <= 253
        && !host.contains(['/', '@', '?', '#', ':'])
        && token_ok(&route.access_client_id)
        && token_ok(&route.access_client_secret)
        && route.access_client_id.is_some() == route.access_client_secret.is_some();
    route.url = url;
    valid.then_some(route)
}

/// `sirus://pair?address=…&code=…[&cfid=…&cfsecret=…]`, which the iPhone app opens from a scan.
fn app_pair_link(route: &PublicRoute, code: &str) -> String {
    fn encode(value: &str) -> String {
        value
            .bytes()
            .map(|byte| match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                    (byte as char).to_string()
                }
                _ => format!("%{byte:02X}"),
            })
            .collect()
    }
    let mut link = format!("sirus://pair?address={}&code={code}", encode(&route.url));
    if let (Some(id), Some(secret)) = (&route.access_client_id, &route.access_client_secret) {
        link.push_str(&format!("&cfid={}&cfsecret={}", encode(id), encode(secret)));
    }
    link
}

fn qr_svg(text: &str) -> Result<String> {
    let code = qrcode::QrCode::with_error_correction_level(text, qrcode::EcLevel::M)
        .map_err(|error| Error::new("remote", error.to_string()))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .min_dimensions(240, 240)
        .quiet_zone(true)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

fn status(remote: &Remote) -> Status {
    let config = remote.config.lock().clone();
    let window = window_token_hash(remote);
    Status {
        enabled: config.enabled,
        running: remote.server.lock().is_some(),
        port: port(&config),
        urls: base_urls(remote),
        public_url: public_route(&remote.path).map(|route| route.url),
        devices: config
            .devices
            .into_iter()
            .filter(|device| window.as_deref() != Some(device.token_hash.as_str()))
            .map(|device| DeviceInfo {
                id: device.id,
                name: device.name,
                created_at: device.created_at,
                last_seen: device.last_seen,
            })
            .collect(),
        error: remote.error.lock().clone(),
        keep_awake: config.keep_awake,
        awake: remote.awake.lock().is_some(),
    }
}

fn port(config: &Config) -> u16 {
    config.port.unwrap_or(DEFAULT_PORT)
}

fn base_urls(remote: &Remote) -> Vec<String> {
    let port = port(&remote.config.lock());
    let listen = remote.listen.lock().clone();
    if let Some(url) = listen.url {
        return vec![url];
    }
    // Development builds can pair over loopback to test without a public route.
    #[cfg(debug_assertions)]
    if std::env::var("SIRUS_REMOTE_LOOPBACK").as_deref() == Ok("1") {
        return vec![format!("http://{}:{port}", Ipv4Addr::LOCALHOST)];
    }
    Vec::new()
}

fn save(remote: &Remote) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(&*remote.config.lock())?;
    let tmp = remote.path.with_extension("json.tmp");
    std::fs::write(&tmp, bytes)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, &remote.path)?;
    Ok(())
}

/// Which listener: the one for the owner's other devices, or Sirus Native's own window (ADR-189).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Devices,
    Window,
}

fn slot(remote: &Remote, kind: Kind) -> &parking_lot::Mutex<Option<Server>> {
    match kind {
        Kind::Devices => &remote.server,
        Kind::Window => &LOCAL,
    }
}

/// Boxed by hand: the connections it serves can call `remote_action`, which calls this.
fn start(
    app: AppHandle,
    remote: Arc<Remote>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    start_kind(app, remote, Kind::Devices)
}

fn start_kind(
    app: AppHandle,
    remote: Arc<Remote>,
    kind: Kind,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    Box::pin(async move {
        let (port, bind) = match kind {
            Kind::Devices => (
                port(&remote.config.lock()),
                remote
                    .listen
                    .lock()
                    .bind
                    .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED)),
            ),
            Kind::Window => (
                remote.listen.lock().local_port.unwrap_or(DEFAULT_PORT),
                IpAddr::V4(Ipv4Addr::LOCALHOST),
            ),
        };
        if slot(&remote, kind)
            .lock()
            .as_ref()
            .is_some_and(|server| server.port == port)
        {
            return;
        }
        if let Some(server) = slot(&remote, kind).lock().take() {
            let _ = server.shutdown.send(true);
        }
        let listener = match tokio::net::TcpListener::bind((bind, port)).await {
            Ok(listener) => listener,
            Err(error) => {
                tracing::warn!(%error, port, "remote access cannot listen");
                *remote.error.lock() = Some(format!("Port {port} is not available: {error}"));
                return;
            }
        };
        let (shutdown, mut stopped) = watch::channel(false);
        *slot(&remote, kind).lock() = Some(Server {
            port,
            shutdown: shutdown.clone(),
        });
        *remote.error.lock() = None;
        let shared = Shared {
            app,
            remote: remote.clone(),
            shutdown: shutdown.subscribe(),
        };
        let router = Router::new()
            .route("/api/health", get(health))
            .route("/api/pair", post(pair))
            .route("/api/app-update/manifest", get(app_update_manifest))
            .route(
                "/api/app-update/file/{platform}/{runtime}/{*path}",
                get(app_update_file),
            )
            // MCP for agents outside Sirus (ADR-209): a bearer token of its own, not a device's.
            .route(
                "/mcp",
                post(mcp)
                    .get(mcp_unsupported)
                    .delete(mcp_unsupported)
                    .layer(axum::extract::DefaultBodyLimit::max(
                        crate::mcp_external::MAX_BODY + 1024,
                    )),
            )
            .route("/api/socket", get(socket))
            .route("/api/tunnel", get(tunnel))
            .route("/api/thumbnail/{id}", get(thumbnail))
            .fallback(get(asset))
            // The bundle is several megabytes of text; compressed it is about a quarter (gzip/brotli).
            .layer(tower_http::compression::CompressionLayer::new());
        // Device streams (ADR-128) are added after compression so they are never buffered.
        // The Simulator pane is the Mac app's; the headless host has none (ADR-131).
        let router = router.layer(middleware::from_fn(guard)).with_state(shared);
        crate::runtime::async_runtime::spawn(async move {
            let served = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async move {
                let _ = stopped.wait_for(|stopped| *stopped).await;
            })
            .await;
            if let Err(error) = served {
                tracing::warn!(%error, "remote access server stopped");
                *remote.error.lock() = Some(error.to_string());
            }
            let mut server = slot(&remote, kind).lock();
            if server
                .as_ref()
                .is_some_and(|server| server.shutdown.same_channel(&shutdown))
            {
                *server = None;
            }
        });
    })
}

fn stop(remote: &Remote) {
    if let Some(server) = remote.server.lock().take() {
        let _ = server.shutdown.send(true);
    }
}

#[derive(Clone)]
struct Shared {
    app: AppHandle,
    remote: Arc<Remote>,
    shutdown: watch::Receiver<bool>,
}

/// Only loopback, and no cross-site requests except from the desktop app's page (ADR-133),
/// which gets CORS answers.
async fn guard(
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    if !allowed_peer(peer.ip()) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let remote = REMOTE.get();
    let https_host = remote.and_then(|remote| own_https_host(remote));
    if same_origin(request.headers(), https_host.as_deref()) {
        return next.run(request).await;
    }
    let Some(origin) = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|origin| origin.to_str().ok())
        .map(str::to_owned)
    else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let allowed = remote.is_some_and(|remote| {
        cross_origin_allowed(&origin, request.uri().path(), &remote.config.lock().devices)
    });
    let Some(origin) = HeaderValue::from_str(&origin).ok().filter(|_| allowed) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let mut response = if request.method() == axum::http::Method::OPTIONS {
        let mut preflight = StatusCode::NO_CONTENT.into_response();
        let headers = preflight.headers_mut();
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("GET, POST, PUT, DELETE"),
        );
        headers.insert(
            header::ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("authorization, content-type"),
        );
        headers.insert(
            header::ACCESS_CONTROL_MAX_AGE,
            HeaderValue::from_static("600"),
        );
        preflight
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
    headers.append(header::VARY, HeaderValue::from_static("origin"));
    response
}

/// This computer's HTTPS name, from the host's advertised URL.
fn own_https_host(remote: &Remote) -> Option<String> {
    let url = remote.listen.lock().url.clone()?;
    let host = url.strip_prefix("https://")?.trim_end_matches('/');
    (!host.contains(['/', ':', '@'])).then(|| host.to_ascii_lowercase())
}

/// The desktop app's own page (ADR-133), which Tauri serves as `tauri://localhost` on macOS and
/// Linux and as `http(s)://tauri.localhost` on Windows. (The native iPhone app sends no origin.)
const DESKTOP_APP_ORIGINS: &[&str] = &[
    "tauri://localhost",
    "http://tauri.localhost",
    "https://tauri.localhost",
];

/// The desktop app's origin, lowercased, when `origin` is exactly one of them.
fn desktop_app_origin(origin: &str) -> Option<String> {
    let origin = origin.to_ascii_lowercase();
    DESKTOP_APP_ORIGINS
        .contains(&origin.as_str())
        .then_some(origin)
}

/// Whether a page on another origin may make this request (ADR-133): only the Sirus desktop
/// app's own page. Pairing and the health check are open to it, since they need the code or
/// say nothing private; everything else only to origins recorded with a paired device.
fn cross_origin_allowed(origin: &str, path: &str, devices: &[Device]) -> bool {
    let Some(origin) = desktop_app_origin(origin) else {
        return false;
    };
    if matches!(path, "/api/pair" | "/api/health") {
        return true;
    }
    devices
        .iter()
        .any(|device| device.origins.contains(&origin))
}

/// The origin a request came from when it is not this computer's own page: the desktop app.
fn foreign_origin(remote: &Remote, headers: &HeaderMap) -> Option<String> {
    let own = own_https_host(remote);
    if same_origin(headers, own.as_deref()) {
        return None;
    }
    desktop_app_origin(headers.get(header::ORIGIN)?.to_str().ok()?)
}

/// This computer for the phone's list of computers (ADR-132): a stable id, its name, and
/// whether it is the Mac app or a server.
fn host_identity(remote: &Remote) -> Value {
    let id = {
        let mut config = remote.config.lock();
        match config.host_id.clone() {
            Some(id) => id,
            None => {
                let id = uuid::Uuid::new_v4().to_string();
                config.host_id = Some(id.clone());
                id
            }
        }
    };
    let _ = save(remote);
    let mut host_name = crate::commands::machine_host_name();
    if let Some(label) = remote.listen.lock().label.clone() {
        host_name = format!("{host_name} ({label})").trim().to_string();
    }
    json!({
        "id": id,
        "hostName": host_name,
        "kind": if remote.listen.lock().local_port.is_some() { "mac" } else { "server" },
    })
}

async fn health(AxumState(shared): AxumState<Shared>) -> Response {
    let mut body = host_identity(&shared.remote);
    body["app"] = json!("sirus");
    // Sirus Native's host has a listener of its own for the window (ADR-189).
    if shared.remote.listen.lock().local_port.is_some() {
        body["window"] = json!(true);
    }
    Json(body).into_response()
}

fn allowed_peer(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => ip.is_loopback(),
        IpAddr::V6(ip) => ip
            .to_ipv4_mapped()
            .map_or(ip.is_loopback(), |mapped| mapped.is_loopback()),
    }
}

/// A browser sends `Origin` on cross-site requests and WebSocket handshakes; it must name
/// this host, or the name it advertises when a tunnel forwards the request.
fn same_origin(headers: &HeaderMap, https_host: Option<&str>) -> bool {
    let Some(origin) = headers.get(header::ORIGIN) else {
        return true;
    };
    if let (Some(host), Ok(origin)) = (https_host, origin.to_str()) {
        if origin.eq_ignore_ascii_case(&format!("https://{host}")) {
            return true;
        }
    }
    let Some(host) = headers
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
    else {
        return false;
    };
    origin
        .to_str()
        .ok()
        .and_then(|origin| origin.split_once("://"))
        .is_some_and(|(_, rest)| rest.eq_ignore_ascii_case(host))
}

/// The app's data folder (where `state.db` lives), for published app updates.
fn app_data_dir(shared: &Shared) -> Option<PathBuf> {
    let state = shared.app.try_state::<Arc<crate::commands::AppState>>()?;
    state.data_path.parent().map(Path::to_path_buf)
}

/// `POST /mcp`: one JSON-RPC message of an outside MCP client (`mcp_external.rs`).
async fn mcp(AxumState(shared): AxumState<Shared>, headers: HeaderMap, body: Bytes) -> Response {
    let Some(dir) = app_data_dir(&shared) else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let bearer = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    match crate::mcp_external::handle(&shared.app, &dir, bearer, &body).await {
        crate::mcp_external::Reply::Json(value) => Json(value).into_response(),
        crate::mcp_external::Reply::Status(code) => {
            let status = StatusCode::from_u16(code).unwrap_or(StatusCode::BAD_REQUEST);
            let mut response = status.into_response();
            if status == StatusCode::UNAUTHORIZED {
                response.headers_mut().insert(
                    header::WWW_AUTHENTICATE,
                    HeaderValue::from_static("Bearer realm=\"sirus\""),
                );
            }
            response
        }
    }
}

/// The endpoint answers with JSON only: no server-sent stream and no session to end.
async fn mcp_unsupported() -> Response {
    let mut response = StatusCode::METHOD_NOT_ALLOWED.into_response();
    response
        .headers_mut()
        .insert(header::ALLOW, HeaderValue::from_static("POST"));
    response
}

/// The native app's update for its runtime version, in the Expo Updates protocol (ADR-135).
async fn app_update_manifest(AxumState(shared): AxumState<Shared>, headers: HeaderMap) -> Response {
    if bearer_device(&shared.remote, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let value = |name: &str| {
        headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };
    let (Some(platform), Some(runtime)) = (value("expo-platform"), value("expo-runtime-version"))
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(host) = own_https_host(&shared.remote)
        .or_else(|| value("host").filter(|host| !host.contains(['/', '@'])))
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let current = value("expo-current-update-id");
    let found = app_data_dir(&shared)
        .and_then(|data| app_update::update_dir(&data, &platform, &runtime))
        .and_then(|dir| {
            app_update::manifest(
                &dir,
                &format!("https://{host}"),
                &platform,
                &runtime,
                current.as_deref(),
            )
        });
    let mut response = match found {
        Some(manifest) => Json(manifest).into_response(),
        None => StatusCode::NO_CONTENT.into_response(),
    };
    let headers = response.headers_mut();
    headers.insert("expo-protocol-version", HeaderValue::from_static("1"));
    headers.insert("expo-sfv-version", HeaderValue::from_static("0"));
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=0"),
    );
    response
}

/// One file of a published app update.
async fn app_update_file(
    AxumState(shared): AxumState<Shared>,
    headers: HeaderMap,
    axum::extract::Path((platform, runtime, path)): axum::extract::Path<(String, String, String)>,
) -> Response {
    if bearer_device(&shared.remote, &headers).is_none() {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(dir) =
        app_data_dir(&shared).and_then(|data| app_update::update_dir(&data, &platform, &runtime))
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some((bytes, kind)) =
        crate::runtime::async_runtime::spawn_blocking(move || app_update::file(&dir, &path))
            .await
            .ok()
            .flatten()
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = bytes.into_response();
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(kind));
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=31536000, immutable"),
    );
    response
}

/// The host has no web app. It serves only the committed Mermaid and KaTeX files the Mac window's
/// hidden page loads (`--assets-dir`, at `/assets/`); anything else gets a short plain-text reply.
async fn asset(AxumState(shared): AxumState<Shared>, uri: Uri) -> Response {
    let path = uri.path().to_string();
    let dir = shared.remote.listen.lock().assets_dir.clone();
    let found = match dir {
        Some(dir) if path.starts_with("/assets/") => {
            crate::runtime::async_runtime::spawn_blocking(move || host::read_asset(&dir, &path))
                .await
                .ok()
                .flatten()
        }
        _ => None,
    };
    let Some((bytes, mime_type)) = found else {
        let (status, text) = if uri.path() == "/" {
            (
                StatusCode::OK,
                "Sirus host. Pair this computer from the Sirus app.\n",
            )
        } else {
            (StatusCode::NOT_FOUND, "Not found.\n")
        };
        return (
            status,
            [
                (header::CONTENT_TYPE, "text/plain; charset=utf-8"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            text,
        )
            .into_response();
    };
    let mut response = bytes.into_response();
    let headers = response.headers_mut();
    if let Ok(value) = HeaderValue::from_str(&mime_type) {
        headers.insert(header::CONTENT_TYPE, value);
    }
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    // The files are committed and change only with a release.
    headers.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=3600"),
    );
    response
}

#[derive(Deserialize)]
struct PairRequest {
    code: String,
    #[serde(default)]
    name: String,
    /// `desktop` from a Sirus window (ADR-201); the phone sends nothing.
    #[serde(default)]
    kind: Option<String>,
}

async fn pair(
    AxumState(shared): AxumState<Shared>,
    headers: HeaderMap,
    Json(request): Json<PairRequest>,
) -> Response {
    let origin = foreign_origin(&shared.remote, &headers);
    let desktop = request.kind.as_deref() == Some("desktop");
    match redeem_as(
        &shared.remote,
        &request.code,
        &request.name,
        origin,
        desktop,
    ) {
        Ok((device_id, token)) => {
            let _ = shared.app.emit(CHANGED, ());
            let host = host_identity(&shared.remote);
            Json(json!({ "deviceId": device_id, "token": token, "host": host })).into_response()
        }
        Err(message) => (
            StatusCode::UNAUTHORIZED,
            Json(json!({ "message": message })),
        )
            .into_response(),
    }
}

/// Exchanges a pairing code for a new device token; `origin` is the other computer's page
/// the device paired from, if any (ADR-132).
fn redeem(
    remote: &Remote,
    code: &str,
    name: &str,
    origin: Option<String>,
) -> std::result::Result<(String, String), &'static str> {
    redeem_as(remote, code, name, origin, false)
}

/// `redeem`, noting whether the device is a Sirus window (ADR-201).
fn redeem_as(
    remote: &Remote,
    code: &str,
    name: &str,
    origin: Option<String>,
    desktop: bool,
) -> std::result::Result<(String, String), &'static str> {
    {
        let mut pairing = remote.pairing.lock();
        let Some(current) = pairing.as_mut() else {
            return Err("This code is no longer valid. Show a new QR code on the Mac.");
        };
        if current.expires <= Instant::now() {
            *pairing = None;
            return Err("This code has expired. Show a new QR code on the Mac.");
        }
        let code: String = code
            .chars()
            .filter(|character| !matches!(character, ' ' | '-'))
            .collect();
        if current.hash != hash(&code) {
            current.failures += 1;
            if current.failures >= PAIR_FAILURES {
                *pairing = None;
            }
            return Err("This code is not valid.");
        }
        *pairing = None;
    }
    let token = random_token(32);
    let device = Device {
        id: uuid::Uuid::new_v4().to_string(),
        name: device_name(name),
        token_hash: hash(&token),
        created_at: Utc::now(),
        last_seen: None,
        origins: origin.into_iter().collect(),
        kind: desktop.then(|| "desktop".to_owned()),
    };
    let id = device.id.clone();
    {
        let mut config = remote.config.lock();
        if counted_devices(remote, &config) >= MAX_DEVICES {
            return Err("Too many devices are connected.");
        }
        config.devices.push(device);
    }
    if save(remote).is_err() {
        return Err("The Mac could not save this device.");
    }
    Ok((id, token))
}

fn device_name(name: &str) -> String {
    let name: String = name
        .chars()
        .filter(|character| !character.is_control())
        .take(60)
        .collect();
    let name = name.trim();
    if name.is_empty() {
        "Device".into()
    } else {
        name.into()
    }
}

/// The device behind `Authorization: Bearer <token>`.
fn bearer_device(remote: &Remote, headers: &HeaderMap) -> Option<String> {
    let token = headers
        .get(header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")?;
    authenticate(remote, token.trim())
}

#[derive(Deserialize)]
struct ThumbnailQuery {
    #[serde(default)]
    token: String,
    #[serde(default)]
    size: String,
}

/// A sent image's thumbnail for a paired device. `<img>` cannot send a header, so the
/// device token comes in the query; it is checked without noting the device as seen
/// (a transcript loads many thumbnails, and the socket already does that).
async fn thumbnail(
    AxumState(shared): AxumState<Shared>,
    axum::extract::Path(id): axum::extract::Path<String>,
    axum::extract::Query(query): axum::extract::Query<ThumbnailQuery>,
) -> Response {
    if !paired(&shared.remote, &query.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Some(data_path) = shared
        .app
        .try_state::<Arc<crate::commands::AppState>>()
        .map(|state| state.data_path.clone())
    else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let small = query.size == "small";
    let bytes = tokio::task::spawn_blocking(move || {
        if small {
            crate::thumbnails::read_small(&data_path, &id)
        } else {
            crate::thumbnails::read(&data_path, &id)
        }
    })
    .await
    .ok()
    .flatten();
    let (parts, body) = crate::thumbnails::response(bytes).into_parts();
    (parts.status, parts.headers, body).into_response()
}

#[derive(Deserialize)]
struct TunnelQuery {
    #[serde(default)]
    port: String,
}

/// A TCP tunnel to this computer's loopback for a Sirus window on another computer (ADR-201,
/// `tunnel.rs`). The device token comes as `Authorization: Bearer`, which a page cannot set on
/// a WebSocket; the target is connected before the upgrade, so a closed port answers 502.
async fn tunnel(
    AxumState(shared): AxumState<Shared>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<TunnelQuery>,
    upgrade: WebSocketUpgrade,
) -> Response {
    // Checked without noting the device as seen: a page opens many connections.
    let Some(device) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .and_then(|token| paired_device(&shared.remote, token.trim()))
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !device_is_desktop(&shared.remote, &device) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(port) = crate::tunnel::valid_port(&query.port) else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    let Some(permit) = crate::tunnel::Permit::take() else {
        return StatusCode::TOO_MANY_REQUESTS.into_response();
    };
    let Some(target) = crate::tunnel::connect(port).await else {
        return StatusCode::BAD_GATEWAY.into_response();
    };
    let mut revoked = shared.remote.revoked.subscribe();
    let mut shutdown = shared.shutdown.clone();
    upgrade
        .max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| async move {
            let stop = async move {
                loop {
                    tokio::select! {
                        gone = revoked.recv() => match gone {
                            Ok(id) if id == device => break,
                            Err(broadcast::error::RecvError::Closed) => break,
                            _ => {}
                        },
                        _ = shutdown.changed() => break,
                    }
                }
            };
            crate::tunnel::pipe(socket, target, stop).await;
            drop(permit);
        })
}

/// The device a token belongs to, without noting it as seen.
fn paired_device(remote: &Remote, token: &str) -> Option<String> {
    if token.is_empty() || token.len() > 256 {
        return None;
    }
    let hashed = hash(token);
    remote
        .config
        .lock()
        .devices
        .iter()
        .find(|device| device.token_hash == hashed)
        .map(|device| device.id.clone())
}

/// Whether a token belongs to a paired device (no side effects).
fn paired(remote: &Remote, token: &str) -> bool {
    paired_device(remote, token).is_some()
}

/// The device a token belongs to, noting when it was last seen.
fn authenticate(remote: &Remote, token: &str) -> Option<String> {
    let hashed = hash(token);
    let id = {
        let mut config = remote.config.lock();
        let device = config
            .devices
            .iter_mut()
            .find(|device| device.token_hash == hashed)?;
        device.last_seen = Some(Utc::now());
        device.id.clone()
    };
    let _ = save(remote);
    Some(id)
}

async fn socket(
    AxumState(shared): AxumState<Shared>,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    let origin = foreign_origin(&shared.remote, &headers);
    upgrade
        .max_message_size(MAX_MESSAGE)
        .on_upgrade(move |socket| connection(socket, shared, origin))
}

/// A socket opened from another computer's page must carry the token of the device that
/// paired from that page (ADR-132).
fn device_allows(remote: &Remote, device_id: &str, origin: Option<&str>) -> bool {
    let Some(origin) = origin else {
        return true;
    };
    remote
        .config
        .lock()
        .devices
        .iter()
        .any(|device| device.id == device_id && device.origins.iter().any(|item| item == origin))
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
enum Incoming {
    Hello {
        token: String,
        /// The client inflates gzipped binary frames (ADR-164); older clients omit it.
        #[serde(default)]
        gzip: bool,
    },
    Invoke {
        id: u64,
        cmd: String,
        #[serde(default)]
        args: Value,
    },
    Listen {
        event: String,
    },
    Unlisten {
        event: String,
    },
}

async fn connection(mut socket: WebSocket, shared: Shared, origin: Option<String>) {
    let (device, gzip, local) = match tokio::time::timeout(HELLO_TIMEOUT, socket.recv()).await {
        Ok(Some(Ok(Message::Text(text)))) => match serde_json::from_str::<Incoming>(&text) {
            Ok(Incoming::Hello { token, gzip }) => {
                let device = authenticate(&shared.remote, &token)
                    .filter(|id| device_allows(&shared.remote, id, origin.as_deref()));
                if device.is_some() {
                    let _ = shared.app.emit(CHANGED, ());
                }
                // The local window (Sirus Native, ADR-189) is the one holding the token it
                // keeps in the data folder; no address or device name makes a device local.
                let local =
                    device.is_some() && crate::browser_bridge::is_local_window(&shared.app, &token);
                (device, gzip, local)
            }
            _ => (None, false, false),
        },
        _ => (None, false, false),
    };
    let Some(device) = device else {
        let _ = socket
            .send(Message::Text(r#"{"type":"unauthorized"}"#.into()))
            .await;
        return;
    };
    if socket
        .send(Message::Text(r#"{"type":"ready"}"#.into()))
        .await
        .is_err()
    {
        return;
    }
    // A Sirus window on another computer may run a headless host's browser (ADR-201).
    let client = crate::browser_bridge::client_kind(
        local,
        device_is_desktop(&shared.remote, &device),
        shared.remote.listen.lock().local_port.is_none(),
    );
    let (outgoing, mut queue) = mpsc::channel::<String>(QUEUE);
    let mut revoked = shared.remote.revoked.subscribe();
    let mut shutdown = shared.shutdown.clone();
    let mut listeners: HashMap<String, crate::runtime::EventId> = HashMap::new();
    let connection_id = crate::browser_bridge::next_connection();
    loop {
        tokio::select! {
            message = socket.recv() => {
                let text = match message {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                    Some(Ok(_)) => continue,
                };
                match serde_json::from_str::<Incoming>(&text) {
                    Ok(Incoming::Invoke { id, cmd, args }) if crate::browser_bridge::is_local_command(&cmd) => {
                        let app = shared.app.clone();
                        let outgoing = outgoing.clone();
                        crate::runtime::async_runtime::spawn(async move {
                            let result = if crate::browser_bridge::command_allowed(&cmd, client) {
                                crate::browser_bridge::run_local(&app, connection_id, client, &outgoing, &cmd, &args)
                                    .await
                                    .map(Reply::Json)
                            } else {
                                Err(json!({ "code": "remote", "message": "This action is only available on the Mac." }))
                            };
                            let _ = outgoing.send(result_message(id, result)).await;
                        });
                    }
                    Ok(Incoming::Invoke { id, cmd, args }) => {
                        let app = shared.app.clone();
                        let outgoing = outgoing.clone();
                        crate::runtime::async_runtime::spawn(async move {
                            let reply = result_message(id, invoke(&app, cmd, args).await);
                            let _ = outgoing.send(reply).await;
                        });
                    }
                    Ok(Incoming::Listen { event }) => {
                        if valid_event(&event)
                            && (local || !crate::browser_bridge::LOCAL_EVENTS.contains(&event.as_str()))
                            && !listeners.contains_key(&event) && listeners.len() < MAX_SUBSCRIPTIONS {
                            let outgoing = outgoing.clone();
                            let name = serde_json::to_string(&event).unwrap_or_default();
                            let id = shared.app.listen_any(event.clone(), move |message| {
                                let payload = if message.payload().is_empty() { "null" } else { message.payload() };
                                let _ = outgoing.try_send(format!(r#"{{"type":"event","event":{name},"payload":{payload}}}"#));
                            });
                            listeners.insert(event, id);
                        }
                    }
                    Ok(Incoming::Unlisten { event }) => {
                        if let Some(id) = listeners.remove(&event) {
                            shared.app.unlisten(id);
                        }
                    }
                    Ok(Incoming::Hello { .. }) | Err(_) => {}
                }
            }
            Some(reply) = queue.recv() => {
                if socket.send(frame(reply, gzip)).await.is_err() {
                    break;
                }
            }
            gone = revoked.recv() => {
                if matches!(gone, Ok(ref id) if *id == device) {
                    let _ = socket.send(Message::Text(r#"{"type":"unauthorized"}"#.into())).await;
                    break;
                }
            }
            _ = shutdown.changed() => break,
        }
        // A device that cannot keep up reconnects and reloads instead of growing the queue.
        if outgoing.capacity() == 0 {
            break;
        }
    }
    for (_, id) in listeners {
        shared.app.unlisten(id);
    }
    crate::browser_bridge::unregister(connection_id);
    let _ = socket.send(Message::Close(None)).await;
}

/// Frames at least this long go gzipped to a client that asked (ADR-164): transcripts, state
/// and session snapshots, never the small streaming events.
const GZIP_FROM: usize = 32 * 1024;

/// One outgoing message: text, or a gzipped binary frame when the client inflates them and
/// the message is large. The fast level keeps a 4 MB transcript to a few milliseconds.
fn frame(text: String, gzip: bool) -> Message {
    use std::io::Write;
    if !gzip || text.len() < GZIP_FROM {
        return Message::Text(text.into());
    }
    let mut encoder = flate2::write::GzEncoder::new(
        Vec::with_capacity(text.len() / 4),
        flate2::Compression::fast(),
    );
    match encoder
        .write_all(text.as_bytes())
        .and_then(|()| encoder.finish())
    {
        Ok(bytes) => Message::Binary(bytes.into()),
        Err(_) => Message::Text(text.into()),
    }
}

fn valid_event(event: &str) -> bool {
    !event.is_empty()
        && event.len() <= 64
        && event.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | ':' | '/')
        })
}

fn has_files(args: &Value) -> bool {
    args.get("files")
        .and_then(Value::as_array)
        .is_some_and(|files| !files.is_empty())
}

fn allowed_command(command: &str) -> bool {
    !command.is_empty()
        && command.len() <= 64
        && command
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
        && !DENIED.contains(&command)
        // The embedded browser lives in this Mac's window: a paired desktop runs its own
        // (`lib/computers.ts` `runsOnThisMac`) and the phone has none.
        && !command.starts_with("browser_")
}

enum Reply {
    Json(String),
    #[allow(dead_code)]
    Raw(Vec<u8>),
}

/// Runs one IPC command as the main window would, on the main thread.
async fn invoke(
    app: &AppHandle,
    command: String,
    args: Value,
) -> std::result::Result<Reply, Value> {
    let refuse = |message: &str| json!({ "code": "remote", "message": message });
    if !allowed_command(&command) {
        return Err(refuse("This action is only available on the Mac."));
    }
    // Pasting with no files reads the Mac's pasteboard; a device sends its own files (ADR-086).
    if command == "paste_prompt_attachments" && !has_files(&args) {
        return Err(refuse("This action is only available on the Mac."));
    }
    dispatch(app, command, args).await
}

/// The host has no window: its command table calls the same functions (ADR-131).
async fn dispatch(
    app: &AppHandle,
    command: String,
    args: Value,
) -> std::result::Result<Reply, Value> {
    let args = if args.is_null() { json!({}) } else { args };
    crate::headless::invoke(app, &command, &args)
        .await
        .map(Reply::Json)
}

fn result_message(id: u64, result: std::result::Result<Reply, Value>) -> String {
    match result {
        Ok(Reply::Json(body)) => {
            let body = if body.is_empty() {
                "null"
            } else {
                body.as_str()
            };
            format!(r#"{{"type":"result","id":{id},"ok":true,"value":{body}}}"#)
        }
        Ok(Reply::Raw(bytes)) => json!({
            "type": "result",
            "id": id,
            "ok": true,
            "raw": base64::engine::general_purpose::STANDARD.encode(bytes),
        })
        .to_string(),
        Err(error) => {
            json!({ "type": "result", "id": id, "ok": false, "error": error }).to_string()
        }
    }
}

/// Six digits, so a home-screen app (which does not share Safari's storage) can type it.
/// Five minutes, single use and five wrong guesses make guessing impractical.
fn pairing_code() -> String {
    let bytes = uuid::Uuid::new_v4().into_bytes();
    let value = u64::from_le_bytes(bytes[..8].try_into().unwrap_or_default());
    format!("{:06}", value % 1_000_000)
}

pub(crate) fn random_token(bytes: usize) -> String {
    let mut buffer = Vec::with_capacity(bytes + 16);
    while buffer.len() < bytes {
        buffer.extend_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    buffer.truncate(bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buffer)
}

fn hash(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn public_routes_are_https_origins_and_make_an_app_pairing_link() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("remote.json");
        assert_eq!(public_route(&config), None);
        let write = |value: serde_json::Value| {
            std::fs::write(dir.path().join("remote-public.json"), value.to_string()).unwrap()
        };
        write(serde_json::json!({ "url": "http://mac.example.com" }));
        assert_eq!(public_route(&config), None);
        write(serde_json::json!({ "url": "https://mac.example.com/path" }));
        assert_eq!(public_route(&config), None);
        write(
            serde_json::json!({ "url": "https://mac.example.com", "accessClientId": "abc.access" }),
        );
        assert_eq!(
            public_route(&config),
            None,
            "the id and secret come together"
        );
        write(
            serde_json::json!({ "url": "https://Mac.Example.com/", "accessClientId": "abc.access", "accessClientSecret": "cfast_x-1" }),
        );
        let route = public_route(&config).unwrap();
        assert_eq!(route.url, "https://mac.example.com");
        assert_eq!(
            app_pair_link(&route, "123456"),
            "sirus://pair?address=https%3A%2F%2Fmac.example.com&code=123456&cfid=abc.access&cfsecret=cfast_x-1"
        );
        write(serde_json::json!({ "url": "https://vps.example.com" }));
        assert_eq!(
            app_pair_link(&public_route(&config).unwrap(), "000111"),
            "sirus://pair?address=https%3A%2F%2Fvps.example.com&code=000111"
        );
    }

    use super::*;

    #[test]
    fn large_frames_go_gzipped_only_to_clients_that_inflate_them() {
        use std::io::Read;
        let small = r#"{"type":"event"}"#.to_string();
        let large = format!(
            r#"{{"type":"result","value":"{}"}}"#,
            "line of output\n".repeat(4000)
        );
        assert!(
            matches!(frame(small.clone(), true), Message::Text(text) if text.as_str() == small)
        );
        assert!(matches!(frame(large.clone(), false), Message::Text(_)));
        let Message::Binary(bytes) = frame(large.clone(), true) else {
            panic!("a large frame is binary");
        };
        assert!(bytes.len() < large.len() / 10);
        let mut inflated = String::new();
        flate2::read::GzDecoder::new(&bytes[..])
            .read_to_string(&mut inflated)
            .unwrap();
        assert_eq!(inflated, large);
    }

    #[test]
    fn device_names_are_one_trimmed_line_of_at_most_60_characters() {
        assert_eq!(
            owner_device_name("  iPhone do trabalho ").unwrap(),
            "iPhone do trabalho"
        );
        assert!(owner_device_name("   ").is_err());
        assert!(owner_device_name("iPhone\nde alguém").is_err());
        assert_eq!(
            owner_device_name(&"á".repeat(80)).unwrap().chars().count(),
            60
        );
    }

    fn remote(dir: &Path) -> Remote {
        Remote {
            path: dir.join("remote.json"),
            config: parking_lot::Mutex::new(Config::default()),
            pairing: parking_lot::Mutex::new(None),
            server: parking_lot::Mutex::new(None),
            error: parking_lot::Mutex::new(None),
            revoked: broadcast::channel(4).0,
            awake: parking_lot::Mutex::new(None),
            listen: parking_lot::Mutex::new(Listen::default()),
        }
    }

    fn offer(remote: &Remote, code: &str, ttl: Duration) {
        *remote.pairing.lock() = Some(PairCode {
            hash: hash(code),
            expires: Instant::now() + ttl,
            failures: 0,
        });
    }

    #[test]
    fn only_loopback_peers_are_answered() {
        for allowed in ["127.0.0.1", "::1", "::ffff:127.0.0.1"] {
            assert!(allowed_peer(allowed.parse().unwrap()), "{allowed}");
        }
        for refused in [
            "192.168.1.10",
            "10.0.0.2",
            "100.101.102.103",
            "8.8.8.8",
            "fd7a:115c:a1e0::1",
            "::ffff:192.168.1.1",
        ] {
            assert!(!allowed_peer(refused.parse().unwrap()), "{refused}");
        }
    }

    #[test]
    fn origin_must_name_this_host() {
        let mut headers = HeaderMap::new();
        assert!(same_origin(&headers, None));
        headers.insert(header::HOST, HeaderValue::from_static("100.80.1.2:7710"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("http://100.80.1.2:7710"),
        );
        assert!(same_origin(&headers, None));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(!same_origin(&headers, None));
        // A tunnel forwards with the computer's advertised name as the origin.
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://mac.example.com"),
        );
        assert!(same_origin(&headers, Some("mac.example.com")));
        assert!(!same_origin(&headers, Some("other.example.com")));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        headers.remove(header::HOST);
        assert!(!same_origin(&headers, None));
    }

    #[test]
    fn the_desktop_app_may_pair_then_only_with_its_own_token() {
        let app = "tauri://localhost";
        // Pairing and the health check are open to the desktop app's page.
        for origin in [
            app,
            "http://tauri.localhost",
            "https://tauri.localhost",
            "TAURI://localhost",
        ] {
            assert!(cross_origin_allowed(origin, "/api/pair", &[]), "{origin}");
            assert!(cross_origin_allowed(origin, "/api/health", &[]), "{origin}");
            assert!(
                !cross_origin_allowed(origin, "/api/socket", &[]),
                "{origin}"
            );
        }
        // Look-alikes never pass.
        for refused in [
            "tauri://localhost:1420",
            "tauri://evil",
            "tauri://localhost/",
            "http://localhost:1420",
            "http://tauri.localhost.evil.example",
            "https://tauri.localhost:8443",
        ] {
            assert!(
                !cross_origin_allowed(refused, "/api/pair", &[]),
                "{refused}"
            );
        }
        let dir = tempfile::tempdir().unwrap();
        let remote = remote(dir.path());
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("vps.example.com"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("tauri://localhost"),
        );
        assert_eq!(
            foreign_origin(&remote, &headers).as_deref(),
            Some("tauri://localhost")
        );
        offer(&remote, "482913", PAIR_TTL);
        let (id, _) = redeem(&remote, "482913", "Mac", foreign_origin(&remote, &headers)).unwrap();
        let devices = remote.config.lock().devices.clone();
        assert!(cross_origin_allowed(app, "/api/socket", &devices));
        // The socket binds the desktop's token to the desktop's page, and the phone's to its own.
        assert!(device_allows(&remote, &id, Some(app)));
        assert!(!device_allows(
            &remote,
            &id,
            Some("https://other.example.com")
        ));
        offer(&remote, "123456", PAIR_TTL);
        let (phone, _) = redeem(&remote, "123456", "iPhone", None).unwrap();
        assert!(!device_allows(&remote, &phone, Some(app)));
        // Revoking the desktop closes its origin again.
        remote
            .config
            .lock()
            .devices
            .retain(|device| device.id != id);
        let devices = remote.config.lock().devices.clone();
        assert!(!cross_origin_allowed(app, "/api/socket", &devices));
    }

    #[test]
    fn foreign_origins_are_told_from_this_computers_own_page() {
        let dir = tempfile::tempdir().unwrap();
        let host = super::tests::remote(dir.path());
        host.listen.lock().url = Some("https://vps.example.com/".into());
        assert_eq!(own_https_host(&host).as_deref(), Some("vps.example.com"));
        let mut headers = HeaderMap::new();
        assert_eq!(foreign_origin(&host, &headers), None);
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://vps.example.com"),
        );
        assert_eq!(foreign_origin(&host, &headers), None);
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://other.example.com"),
        );
        assert_eq!(foreign_origin(&host, &headers), None);
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("tauri://localhost"),
        );
        assert_eq!(
            foreign_origin(&host, &headers).as_deref(),
            Some("tauri://localhost")
        );
        // The identity is stable once made.
        let first = host_identity(&host);
        assert_eq!(first["id"], host_identity(&host)["id"]);
        assert!(matches!(first["kind"].as_str(), Some("mac" | "server")));
    }

    #[test]
    fn pairing_codes_are_single_use_and_tokens_authenticate() {
        let dir = tempfile::tempdir().unwrap();
        let remote = remote(dir.path());
        offer(&remote, "482913", PAIR_TTL);
        let (id, token) = redeem(&remote, "482913", " iPhone\n", None).unwrap();
        assert!(redeem(&remote, "482913", "again", None).is_err());
        // A typed code may carry the space or dash shown on the Mac.
        offer(&remote, "123456", PAIR_TTL);
        assert!(redeem(&remote, "123 456", "Typed", None).is_ok());
        assert_eq!(authenticate(&remote, &token), Some(id.clone()));
        assert_eq!(authenticate(&remote, "wrong"), None);
        assert!(paired(&remote, &token));
        assert!(!paired(&remote, "wrong") && !paired(&remote, ""));
        let saved: Config =
            serde_json::from_slice(&std::fs::read(dir.path().join("remote.json")).unwrap())
                .unwrap();
        assert_eq!(saved.devices[0].name, "iPhone");
        assert_ne!(saved.devices[0].token_hash, token);
        assert!(saved.devices[0].last_seen.is_some());
    }

    #[test]
    fn expired_or_guessed_codes_fail() {
        let dir = tempfile::tempdir().unwrap();
        let remote = remote(dir.path());
        offer(&remote, "late", Duration::ZERO);
        assert!(redeem(&remote, "late", "", None).is_err());
        offer(&remote, "right", PAIR_TTL);
        for _ in 0..PAIR_FAILURES {
            assert!(redeem(&remote, "guess", "", None).is_err());
        }
        // Too many wrong guesses withdraw the code itself.
        assert!(redeem(&remote, "right", "", None).is_err());
        assert!(remote.config.lock().devices.is_empty());
    }

    #[test]
    fn device_limit_holds() {
        let dir = tempfile::tempdir().unwrap();
        let remote = remote(dir.path());
        for index in 0..MAX_DEVICES {
            offer(&remote, "code", PAIR_TTL);
            redeem(&remote, "code", &format!("Device {index}"), None).unwrap();
        }
        offer(&remote, "code", PAIR_TTL);
        assert!(redeem(&remote, "code", "One more", None).is_err());
    }

    #[test]
    fn commands_and_events_are_checked() {
        assert!(allowed_command("send_prompt"));
        assert!(allowed_command("load_state"));
        for refused in [
            "remote_action",
            "remote_pair",
            "pick_folder",
            "computer_action",
            "connector_export_logins",
            "browser_open",
            "browser_navigate",
            "plugin:opener|open_url",
            "",
            "a b",
        ] {
            assert!(!allowed_command(refused), "{refused}");
        }
        assert!(valid_event("agent-output"));
        assert!(has_files(
            &json!({ "owner": "s", "files": [{ "name": "a.jpg", "data": "AA" }] })
        ));
        assert!(!has_files(&json!({ "owner": "s", "files": [] })));
        assert!(!has_files(&json!({ "owner": "s" })));
        assert!(!valid_event("bad event"));
        assert!(!valid_event(""));
    }

    #[test]
    fn results_embed_the_command_reply() {
        assert_eq!(
            result_message(7, Ok(Reply::Json(r#"{"a":1}"#.into()))),
            r#"{"type":"result","id":7,"ok":true,"value":{"a":1}}"#
        );
        assert_eq!(
            result_message(8, Ok(Reply::Json(String::new()))),
            r#"{"type":"result","id":8,"ok":true,"value":null}"#
        );
        let raw: Value =
            serde_json::from_str(&result_message(9, Ok(Reply::Raw(vec![1, 2])))).unwrap();
        assert_eq!(raw["raw"], "AQI=");
        let error: Value =
            serde_json::from_str(&result_message(3, Err(json!({ "code": "x" })))).unwrap();
        assert_eq!(error["ok"], false);
        assert_eq!(error["error"]["code"], "x");
    }

    #[test]
    fn the_mac_stays_awake_only_with_remote_access_and_the_switch() {
        let mut config = Config {
            keep_awake: true,
            ..Config::default()
        };
        assert!(!awake_wanted(&config), "remote access off");
        config.enabled = true;
        assert!(awake_wanted(&config));
        config.keep_awake = false;
        assert!(!awake_wanted(&config));
        // On power only, and released when Sirus exits.
        assert_eq!(caffeinate_args(42), ["-s", "-w", "42"]);
    }

    #[test]
    fn sirus_windows_pair_as_desktops_and_phones_do_not() {
        assert!(desktop_kind(Some("desktop"), "anything", false));
        assert!(!desktop_kind(Some("phone"), "Sirus", false));
        // Windows paired before the kind existed always sent "Sirus" from no page.
        assert!(desktop_kind(None, "Sirus", false));
        assert!(!desktop_kind(None, "Sirus", true));
        assert!(!desktop_kind(None, "iPhone · Sirus", false));
        assert!(!desktop_kind(None, "Renamed", false));

        let dir = tempfile::tempdir().unwrap();
        let remote = remote(dir.path());
        offer(&remote, "111111", Duration::from_secs(60));
        let (window, _) = redeem_as(&remote, "111111", "Sirus", None, true).unwrap();
        offer(&remote, "222222", Duration::from_secs(60));
        let (phone, _) = redeem(&remote, "222222", "iPhone · Sirus", None).unwrap();
        assert!(device_is_desktop(&remote, &window));
        assert!(!device_is_desktop(&remote, &phone));
        assert!(!device_is_desktop(&remote, "unknown"));
        // The kind is saved with the device.
        let saved = std::fs::read_to_string(&remote.path).unwrap();
        assert!(saved.contains(r#""kind": "desktop""#) || saved.contains(r#""kind":"desktop""#));
    }

    #[test]
    fn the_native_window_is_not_a_device_and_the_secure_port_joins_the_origin() {
        let dir = tempfile::tempdir().unwrap();
        let remote = remote(dir.path());
        remote.listen.lock().local_port = Some(7811);
        std::fs::write(dir.path().join("native-token"), "window-token\n").unwrap();
        offer(&remote, "111111", Duration::from_secs(60));
        let (window, _) = redeem(&remote, "111111", "Sirus Native", None).unwrap();
        remote.config.lock().devices[0].token_hash = hash("window-token");
        offer(&remote, "222222", Duration::from_secs(60));
        let (phone, _) = redeem(&remote, "222222", "Phone", None).unwrap();
        let listed: Vec<String> = status(&remote).devices.into_iter().map(|d| d.id).collect();
        assert_eq!(listed, std::slice::from_ref(&phone));
        assert!(refuse_window_device(&remote, &window).is_err());
        assert!(refuse_window_device(&remote, &phone).is_ok());
        assert_eq!(counted_devices(&remote, &remote.config.lock()), 1);
        // Without a window listener nothing is hidden.
        remote.listen.lock().local_port = None;
        assert_eq!(status(&remote).devices.len(), 2);
    }

    #[test]
    fn pairing_links_become_qr_codes() {
        let svg = qr_svg("https://host.example.com/?pair=abcdefghijklmnopqrstuv").unwrap();
        assert!(svg.starts_with("<?xml") || svg.starts_with("<svg"));
        assert!(svg.contains("#000000"));
    }

    #[test]
    fn tokens_are_url_safe_and_distinct() {
        let first = random_token(32);
        assert_eq!(first.len(), 43);
        assert!(first
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_')));
        assert_ne!(first, random_token(32));
        assert_eq!(device_name("   "), "Device");
        let code = pairing_code();
        assert_eq!(code.len(), 6);
        assert!(code.chars().all(|character| character.is_ascii_digit()));
    }
}
