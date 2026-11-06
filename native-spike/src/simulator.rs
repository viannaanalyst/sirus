//! The Simulador pane's engine, after Sirus Code's DevicesPane and `device-stream.ts` (ADR-128).
//! `sirus-host` has no device commands (the `devices` module is desktop-only), so this app runs
//! Expo's pinned `expo-device-hub` itself, on loopback, and talks to it directly: boot and attach
//! over HTTP, the screen as the helper's MJPEG stream, input as `[tag][json]` WebSocket packets.
//! The hub runs only while a visible pane holds a lease. Pure parts (device list, JPEG framing,
//! key and touch mapping, rate gate) are tested; the I/O is not.
use gpui::RenderImage;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};
use tungstenite::Message;

pub const HUB_PACKAGE: &str = "expo-device-hub";
pub const HUB_VERSION: &str = "0.15.3";
pub const NODE_MAJOR: u32 = 22;
const SENTINEL: &str = ".install-complete";
/// The screen is drawn at most this often; frames that arrive sooner are dropped unseen.
pub const FRAME_INTERVAL: Duration = Duration::from_millis(66);

const TAG_TOUCH: u8 = 0x03;
const TAG_BUTTON: u8 = 0x04;
const TAG_KEY: u8 = 0x06;
const TAG_ORIENTATION: u8 = 0x07;
const TAG_SCREEN_CONFIG: u8 = 0x82;
const TAG_INPUT_ADMITTED: u8 = 0x83;

#[derive(Clone, Debug, PartialEq)]
pub struct SimDevice {
    pub udid: String,
    pub name: String,
    pub runtime: String,
    pub booted: bool,
}

pub fn valid_udid(udid: &str) -> bool {
    !udid.is_empty() && udid.len() <= 64 && udid.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// iOS simulators from `simctl list devices available -j`: booted first, then newest runtime.
pub fn parse_devices(list: &Value) -> Vec<SimDevice> {
    let mut devices = Vec::new();
    let Some(runtimes) = list.get("devices").and_then(Value::as_object) else {
        return devices;
    };
    for (runtime, entries) in runtimes {
        let Some(version) = runtime
            .rsplit('.')
            .next()
            .and_then(|t| t.strip_prefix("iOS-"))
        else {
            continue;
        };
        let runtime = format!("iOS {}", version.replace('-', "."));
        for entry in entries.as_array().into_iter().flatten() {
            let (Some(udid), Some(name)) = (
                entry.get("udid").and_then(Value::as_str),
                entry.get("name").and_then(Value::as_str),
            ) else {
                continue;
            };
            if !valid_udid(udid) || entry.get("isAvailable").and_then(Value::as_bool) == Some(false)
            {
                continue;
            }
            devices.push(SimDevice {
                udid: udid.into(),
                name: name.chars().take(80).collect(),
                runtime: runtime.clone(),
                booted: entry.get("state").and_then(Value::as_str) == Some("Booted"),
            });
        }
    }
    let version = |d: &SimDevice| -> Vec<u32> {
        d.runtime
            .trim_start_matches("iOS ")
            .split('.')
            .filter_map(|p| p.parse().ok())
            .collect()
    };
    devices.sort_by(|a, b| {
        b.booted
            .cmp(&a.booted)
            .then_with(|| version(b).cmp(&version(a)))
            .then_with(|| a.name.cmp(&b.name))
    });
    devices
}

// ---------------------------------------------------------------- toolchain

#[derive(Clone, Debug)]
pub struct Node {
    pub path: PathBuf,
    pub version: String,
    pub major: u32,
}

impl Node {
    pub fn ok(&self) -> bool {
        self.major >= NODE_MAJOR && self.npm().is_some()
    }
    pub fn npm(&self) -> Option<PathBuf> {
        let npm = self.path.parent()?.join("npm");
        npm.exists().then_some(npm)
    }
    fn child_path(&self) -> std::ffi::OsString {
        let mut dirs: Vec<PathBuf> = self
            .path
            .parent()
            .map(Path::to_path_buf)
            .into_iter()
            .collect();
        dirs.extend(node_dirs());
        dirs.extend(["/usr/bin", "/bin", "/usr/sbin", "/sbin"].map(PathBuf::from));
        std::env::join_paths(dirs).unwrap_or_default()
    }
}

/// `v22.11.0` gives 22.
pub fn node_major(version: &str) -> Option<u32> {
    version
        .trim()
        .strip_prefix('v')?
        .split('.')
        .next()?
        .parse()
        .ok()
}

/// Where Node usually lives when the app starts from Finder without the shell's PATH.
fn node_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(["/opt/homebrew/bin", "/usr/local/bin"].map(PathBuf::from));
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        let mut versions: Vec<(Vec<u32>, PathBuf)> =
            std::fs::read_dir(home.join(".nvm/versions/node"))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().into_owned();
                    let parts = name
                        .strip_prefix('v')?
                        .split('.')
                        .map(|p| p.parse().ok())
                        .collect::<Option<Vec<u32>>>()?;
                    Some((parts, entry.path().join("bin")))
                })
                .collect();
        versions.sort_by(|a, b| b.0.cmp(&a.0));
        dirs.extend(versions.into_iter().map(|(_, dir)| dir));
        for relative in [
            ".local/bin",
            ".volta/bin",
            ".local/share/fnm/aliases/default/bin",
            ".local/share/mise/shims",
            ".asdf/shims",
        ] {
            dirs.push(home.join(relative));
        }
    }
    let mut seen = HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    dirs
}

/// The first Node 22+ with npm beside it, else the newest found (so the pane can name it).
pub fn find_node() -> Option<Node> {
    let mut best: Option<Node> = None;
    for dir in node_dirs() {
        let path = dir.join("node");
        if !path.is_file() {
            continue;
        }
        let Ok(out) = Command::new(&path).arg("--version").output() else {
            continue;
        };
        let version = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        let Some(major) = node_major(&version) else {
            continue;
        };
        let node = Node {
            path,
            version,
            major,
        };
        if node.ok() {
            return Some(node);
        }
        if best.as_ref().is_none_or(|b| node.major > b.major) {
            best = Some(node);
        }
    }
    best
}

pub fn xcode_available() -> bool {
    Command::new("/usr/bin/xcrun")
        .args(["--find", "simctl"])
        .stdin(Stdio::null())
        .output()
        .is_ok_and(|o| o.status.success())
}

fn install_dir(tools: &Path) -> PathBuf {
    tools.join(HUB_PACKAGE).join(HUB_VERSION)
}

fn entry(tools: &Path) -> PathBuf {
    install_dir(tools)
        .join("node_modules")
        .join(HUB_PACKAGE)
        .join("dist/server/cli.mjs")
}

fn installed(tools: &Path) -> bool {
    std::fs::read_to_string(install_dir(tools).join(SENTINEL))
        .is_ok_and(|s| s.trim() == HUB_VERSION)
        && entry(tools).is_file()
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

pub fn tools_dir() -> PathBuf {
    crate::backend::data_dir().join("tools")
}

/// The hub's entry file: this app's own install, else the one Sirus Code made (same pinned
/// version, only read here).
pub fn find_entry() -> Option<PathBuf> {
    [
        tools_dir(),
        home().join("Library/Application Support/com.siruscode.app/tools"),
    ]
    .iter()
    .find(|tools| installed(tools))
    .map(|tools| entry(tools))
}

/// npm-installs the pinned hub into this app's data folder, staged and renamed into place.
pub fn install(node: &Node) -> Result<(), String> {
    let npm = node.npm().ok_or("npm")?;
    let tools = tools_dir();
    let parent = tools.join(HUB_PACKAGE);
    let target = install_dir(&tools);
    std::fs::create_dir_all(&parent).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_dir_all(&target);
    let staging = parent.join(format!(".staging-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging).map_err(|e| e.to_string())?;
    let result = (|| {
        let output = Command::new(&npm)
            .arg("install")
            .arg("--prefix")
            .arg(&staging)
            .args([
                "--no-fund",
                "--no-audit",
                "--ignore-scripts",
                "--omit=dev",
                "--no-update-notifier",
                "--loglevel=error",
            ])
            .arg(format!("{HUB_PACKAGE}@{HUB_VERSION}"))
            .current_dir(&staging)
            .env("PATH", node.child_path())
            .stdin(Stdio::null())
            .output()
            .map_err(|e| e.to_string())?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let last = stderr
                .lines()
                .rev()
                .find(|l| !l.trim().is_empty())
                .unwrap_or("no output");
            return Err(last.chars().take(300).collect::<String>());
        }
        if !staging
            .join("node_modules")
            .join(HUB_PACKAGE)
            .join("dist/server/cli.mjs")
            .is_file()
        {
            return Err("entry file missing".into());
        }
        std::fs::write(staging.join(SENTINEL), format!("{HUB_VERSION}\n"))
            .map_err(|e| e.to_string())?;
        std::fs::rename(&staging, &target).map_err(|e| e.to_string())
    })();
    let _ = std::fs::remove_dir_all(&staging);
    result
}

/// What the pane shows before a device can be chosen.
#[derive(Debug, PartialEq)]
pub enum Setup {
    NeedsXcode,
    NeedsNode(Option<String>),
    NeedsInstall,
    Ready,
}

pub struct Toolchain {
    pub xcode: bool,
    pub node: Option<Node>,
    pub entry: Option<PathBuf>,
}

pub fn check() -> Toolchain {
    Toolchain {
        xcode: xcode_available(),
        node: find_node(),
        entry: find_entry(),
    }
}

pub fn setup(toolchain: &Toolchain) -> Setup {
    if !toolchain.xcode {
        return Setup::NeedsXcode;
    }
    match &toolchain.node {
        Some(node) if node.ok() => {}
        other => return Setup::NeedsNode(other.as_ref().map(|n| n.version.clone())),
    }
    if toolchain.entry.is_none() {
        Setup::NeedsInstall
    } else {
        Setup::Ready
    }
}

pub fn list_devices() -> Vec<SimDevice> {
    Command::new("/usr/bin/xcrun")
        .args(["simctl", "list", "devices", "available", "-j"])
        .stdin(Stdio::null())
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice::<Value>(&o.stdout).ok())
        .map(|v| parse_devices(&v))
        .unwrap_or_default()
}

fn simctl(args: &[&str]) -> Result<(), String> {
    let out = Command::new("/usr/bin/xcrun")
        .arg("simctl")
        .args(args)
        .stdin(Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr)
            .trim()
            .chars()
            .take(300)
            .collect())
    }
}

pub fn set_appearance(udid: &str, dark: bool) -> Result<(), String> {
    if !valid_udid(udid) {
        return Err("udid".into());
    }
    simctl(&[
        "ui",
        udid,
        "appearance",
        if dark { "dark" } else { "light" },
    ])
}

/// A PNG of the screen into Downloads; the path.
pub fn screenshot(udid: &str) -> Result<PathBuf, String> {
    if !valid_udid(udid) {
        return Err("udid".into());
    }
    let dir = home().join("Downloads");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!(
        "Simulador {}.png",
        chrono::Local::now().format("%Y-%m-%d %H.%M.%S")
    ));
    simctl(&["io", udid, "screenshot", &path.to_string_lossy()])?;
    Ok(path)
}

// ---------------------------------------------------------------------- hub

struct HubState {
    child: Option<Child>,
    port: u16,
    leases: Vec<u64>,
}

static HUB: Mutex<HubState> = Mutex::new(HubState {
    child: None,
    port: 0,
    leases: Vec::new(),
});
static NEXT_LEASE: AtomicU64 = AtomicU64::new(1);

pub fn new_lease() -> u64 {
    NEXT_LEASE.fetch_add(1, Ordering::Relaxed)
}

/// Request over HTTP/1.0 so a streaming body is close-delimited, never chunked.
fn http(
    port: u16,
    method: &str,
    path: &str,
    body: Option<&Value>,
    wait: Duration,
) -> Result<(u16, String), String> {
    let mut stream =
        TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(wait))
        .map_err(|e| e.to_string())?;
    let body = body.map(Value::to_string).unwrap_or_default();
    write!(stream, "{method} {path} HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}", body.len()).map_err(|e| e.to_string())?;
    let mut reply = String::new();
    let _ = stream.read_to_string(&mut reply);
    let status = reply
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Ok((
        status,
        reply
            .split_once("\r\n\r\n")
            .map(|(_, b)| b.to_owned())
            .unwrap_or_default(),
    ))
}

/// Takes a lease and returns the hub's port, starting the hub (and waiting for `/readyz`) first.
pub fn acquire(lease: u64, node: &Node, entry: &Path) -> Result<u16, String> {
    use std::os::unix::process::CommandExt;
    let mut hub = HUB.lock().unwrap();
    let alive = match hub.child.as_mut() {
        Some(child) => child.try_wait().ok().flatten().is_none(),
        None => false,
    };
    if !alive {
        hub.child = None;
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|l| l.local_addr())
            .map_err(|e| e.to_string())?
            .port();
        let child = Command::new(&node.path)
            .arg(entry)
            .args([
                "--port",
                &port.to_string(),
                "--host",
                "127.0.0.1",
                "--hide-sidebar",
                "--hide-boot-device",
                "--platform",
                "ios",
            ])
            .env("PATH", node.child_path())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|e| e.to_string())?;
        hub.child = Some(child);
        hub.port = port;
        let mut ready = false;
        for _ in 0..120 {
            if http(port, "GET", "/readyz", None, Duration::from_secs(2))
                .is_ok_and(|(s, _)| s == 200)
            {
                ready = true;
                break;
            }
            if hub
                .child
                .as_mut()
                .is_none_or(|c| c.try_wait().ok().flatten().is_some())
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        if !ready {
            kill(&mut hub);
            return Err("hub".into());
        }
    }
    if !hub.leases.contains(&lease) {
        hub.leases.push(lease);
    }
    Ok(hub.port)
}

fn kill(hub: &mut HubState) {
    if let Some(mut child) = hub.child.take() {
        let pid = child.id() as i32;
        unsafe { libc::kill(-pid, libc::SIGTERM) };
        std::thread::spawn(move || {
            for _ in 0..30 {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            unsafe { libc::kill(-pid, libc::SIGKILL) };
            let _ = child.wait();
        });
    }
    hub.leases.clear();
}

/// Drops a lease; the hub stops with the last one.
pub fn release(lease: u64) {
    let mut hub = HUB.lock().unwrap();
    hub.leases.retain(|l| *l != lease);
    if hub.leases.is_empty() {
        kill(&mut hub);
    }
}

/// Boots the device if it is off, then attaches the stream helper (idempotent).
pub fn attach(port: u16, device: &SimDevice) -> Result<(), String> {
    if !valid_udid(&device.udid) {
        return Err("udid".into());
    }
    let long = Duration::from_secs(120);
    if !device.booted {
        let (status, body) = http(
            port,
            "POST",
            "/api/devices/boot",
            Some(&json!({ "platform": "ios", "id": device.udid })),
            long,
        )?;
        if !(200..300).contains(&status) {
            return Err(body.chars().take(200).collect());
        }
    }
    let (status, body) = http(
        port,
        "POST",
        "/vendor/serve-sim/grid/api/start",
        Some(&json!({ "udid": device.udid })),
        long,
    )?;
    if (200..300).contains(&status) {
        Ok(())
    } else {
        Err(body.chars().take(200).collect())
    }
}

pub fn shutdown(port: Option<u16>, udid: &str) -> Result<(), String> {
    if !valid_udid(udid) {
        return Err("udid".into());
    }
    if let Some(port) = port {
        if http(
            port,
            "POST",
            "/vendor/serve-sim/grid/api/shutdown",
            Some(&json!({ "udid": udid })),
            Duration::from_secs(30),
        )
        .is_ok_and(|(s, _)| (200..300).contains(&s))
        {
            return Ok(());
        }
    }
    simctl(&["shutdown", udid])
}

// ------------------------------------------------------------------- frames

/// Cuts JPEG frames out of the helper's MJPEG body by their start and end markers; only the
/// newest complete frame of a push is kept.
#[derive(Default)]
pub struct JpegSplitter {
    buf: Vec<u8>,
}

impl JpegSplitter {
    pub fn push(&mut self, bytes: &[u8]) -> Option<Vec<u8>> {
        self.buf.extend_from_slice(bytes);
        let mut latest = None;
        loop {
            let Some(start) = self.buf.windows(2).position(|w| w == [0xFF, 0xD8]) else {
                let keep = self.buf.last().copied().filter(|b| *b == 0xFF);
                self.buf.clear();
                self.buf.extend(keep);
                break;
            };
            self.buf.drain(..start);
            let Some(end) = self.buf[2..].windows(2).position(|w| w == [0xFF, 0xD9]) else {
                if self.buf.len() > 16 << 20 {
                    self.buf.clear();
                }
                break;
            };
            let frame: Vec<u8> = self.buf.drain(..end + 4).collect();
            latest = Some(frame);
        }
        latest
    }
}

/// Lets a frame through only when `FRAME_INTERVAL` has passed since the last one.
pub struct FrameGate {
    last: Option<Instant>,
    interval: Duration,
}

impl FrameGate {
    pub fn new(interval: Duration) -> Self {
        Self {
            last: None,
            interval,
        }
    }
    pub fn allow(&mut self, now: Instant) -> bool {
        if self
            .last
            .is_none_or(|last| now.duration_since(last) >= self.interval)
        {
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub width: f64,
    pub height: f64,
    pub orientation: String,
}

pub const ORIENTATIONS: [&str; 4] = [
    "portrait",
    "landscape_left",
    "portrait_upside_down",
    "landscape_right",
];

pub fn parse_screen(payload: &Value) -> Option<Screen> {
    let (w, h) = (
        payload.get("width")?.as_f64()?,
        payload.get("height")?.as_f64()?,
    );
    let orientation = payload.get("orientation")?.as_str()?;
    (w > 0. && h > 0. && ORIENTATIONS.contains(&orientation)).then(|| Screen {
        width: w,
        height: h,
        orientation: orientation.into(),
    })
}

pub fn next_orientation(current: Option<&str>) -> &'static str {
    let at = ORIENTATIONS
        .iter()
        .position(|o| Some(*o) == current)
        .unwrap_or(0);
    ORIENTATIONS[(at + 1) % ORIENTATIONS.len()]
}

/// Quarter turns clockwise that show the raw portrait frame as the device is held.
pub fn display_turns(screen: Option<&Screen>) -> u8 {
    match screen {
        Some(s) if s.width <= s.height => match s.orientation.as_str() {
            "landscape_left" => 1,
            "portrait_upside_down" => 2,
            "landscape_right" => 3,
            _ => 0,
        },
        _ => 0,
    }
}

/// A point in the picture as shown (0..1) mapped back into the raw portrait frame.
pub fn raw_point(screen: Option<&Screen>, x: f64, y: f64) -> (f64, f64) {
    match display_turns(screen) {
        1 => (y, 1. - x),
        3 => (1. - y, x),
        2 => (1. - x, 1. - y),
        _ => (x, y),
    }
}

/// The largest box at `aspect` (width over height) inside the stage.
pub fn fit_frame(aspect: f32, width: f32, height: f32) -> (f32, f32) {
    if width <= 0. || height <= 0. || aspect <= 0. {
        return (0., 0.);
    }
    if width / height > aspect {
        (height * aspect, height)
    } else {
        (width, width / aspect)
    }
}

pub fn tagged(tag: u8, payload: &Value) -> Vec<u8> {
    let mut packet = vec![tag];
    packet.extend_from_slice(payload.to_string().as_bytes());
    packet
}

/// USB HID usage of a GPUI key name, or None for keys the simulator has no use for.
pub fn hid_usage(key: &str) -> Option<u8> {
    let mut chars = key.chars();
    if let (Some(c), None) = (chars.next(), chars.next()) {
        match c {
            'a'..='z' => return Some(0x04 + (c as u8 - b'a')),
            'A'..='Z' => return Some(0x04 + (c.to_ascii_lowercase() as u8 - b'a')),
            '1'..='9' => return Some(0x1e + (c as u8 - b'1')),
            '0' => return Some(0x27),
            _ => {}
        }
    }
    Some(match key {
        "enter" => 0x28,
        "escape" => 0x29,
        "backspace" => 0x2a,
        "tab" => 0x2b,
        "space" | " " => 0x2c,
        "-" => 0x2d,
        "=" => 0x2e,
        "[" => 0x2f,
        "]" => 0x30,
        "\\" => 0x31,
        ";" => 0x33,
        "'" => 0x34,
        "`" => 0x35,
        "," => 0x36,
        "." => 0x37,
        "/" => 0x38,
        "delete" => 0x4c,
        "right" => 0x4f,
        "left" => 0x50,
        "down" => 0x51,
        "up" => 0x52,
        _ => return None,
    })
}

pub fn key_packets(usage: u8, shift: bool) -> Vec<Vec<u8>> {
    let key = |kind: &str, usage: u8| tagged(TAG_KEY, &json!({ "type": kind, "usage": usage }));
    let mut packets = Vec::new();
    if shift {
        packets.push(key("down", 0xe1));
    }
    packets.push(key("down", usage));
    packets.push(key("up", usage));
    if shift {
        packets.push(key("up", 0xe1));
    }
    packets
}

pub fn touch_packet(screen: Option<&Screen>, phase: &str, x: f64, y: f64) -> Vec<u8> {
    let (x, y) = raw_point(screen, x.clamp(0., 1.), y.clamp(0., 1.));
    tagged(TAG_TOUCH, &json!({ "type": phase, "x": x, "y": y }))
}

pub fn button_packet(button: &str) -> Vec<u8> {
    tagged(TAG_BUTTON, &json!({ "button": button }))
}

pub fn orientation_packet(next: &str) -> Vec<u8> {
    tagged(TAG_ORIENTATION, &json!({ "orientation": next }))
}

// --------------------------------------------------------------------- feed

pub struct Frame {
    pub image: Arc<RenderImage>,
    pub width: u32,
    pub height: u32,
}

/// The live screen of one device: a video thread (MJPEG, newest frame only) and an input thread
/// (WebSocket). `stop` ends both; the pane drops it as soon as it is not shown.
pub struct Feed {
    stop: AtomicBool,
    pub seq: AtomicU64,
    frame: Mutex<Option<Frame>>,
    screen: Mutex<Option<Screen>>,
    input: AtomicBool,
    problem: Mutex<Option<String>>,
    out: Mutex<mpsc::Sender<Vec<u8>>>,
}

impl Feed {
    pub fn start(port: u16, udid: &str) -> Arc<Feed> {
        let (tx, rx) = mpsc::channel();
        let feed = Arc::new(Feed {
            stop: AtomicBool::new(false),
            seq: AtomicU64::new(0),
            frame: Mutex::new(None),
            screen: Mutex::new(None),
            input: AtomicBool::new(false),
            problem: Mutex::new(None),
            out: Mutex::new(tx),
        });
        let (video, input, udid) = (feed.clone(), feed.clone(), udid.to_owned());
        let device = udid.clone();
        std::thread::spawn(move || video.run_video(port, &device));
        std::thread::spawn(move || input.run_input(port, &udid, rx));
        feed
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
    pub fn take_frame(&self) -> Option<Frame> {
        self.frame.lock().unwrap().take()
    }
    pub fn screen(&self) -> Option<Screen> {
        self.screen.lock().unwrap().clone()
    }
    pub fn input_ready(&self) -> bool {
        self.input.load(Ordering::SeqCst)
    }
    pub fn problem(&self) -> Option<String> {
        self.problem.lock().unwrap().clone()
    }
    /// Sent only while the helper admits input.
    pub fn send(&self, packet: Vec<u8>) {
        if self.input_ready() {
            let _ = self.out.lock().unwrap().send(packet);
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn run_video(&self, port: u16, udid: &str) {
        let mut gate = FrameGate::new(FRAME_INTERVAL);
        while !self.stopped() {
            let result = self.read_video(port, udid, &mut gate);
            *self.problem.lock().unwrap() = result.err();
            for _ in 0..10 {
                if self.stopped() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    fn read_video(&self, port: u16, udid: &str, gate: &mut FrameGate) -> Result<(), String> {
        let mut stream =
            TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
        stream
            .set_read_timeout(Some(Duration::from_millis(500)))
            .map_err(|e| e.to_string())?;
        write!(stream, "GET /vendor/serve-sim/helper/{udid}/stream.mjpeg HTTP/1.0\r\nHost: 127.0.0.1:{port}\r\n\r\n").map_err(|e| e.to_string())?;
        let mut splitter = JpegSplitter::default();
        let mut chunk = vec![0u8; 64 * 1024];
        let mut quiet = Instant::now();
        loop {
            if self.stopped() {
                return Ok(());
            }
            match stream.read(&mut chunk) {
                Ok(0) => return Err("closed".into()),
                Ok(n) => {
                    quiet = Instant::now();
                    if let Some(jpeg) = splitter.push(&chunk[..n]) {
                        if gate.allow(Instant::now()) {
                            let screen = self.screen();
                            if let Some(frame) = decode(&jpeg, display_turns(screen.as_ref())) {
                                *self.frame.lock().unwrap() = Some(frame);
                                self.seq.fetch_add(1, Ordering::SeqCst);
                            }
                        }
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) =>
                {
                    if quiet.elapsed() > Duration::from_secs(15) {
                        return Err("silent".into());
                    }
                }
                Err(e) => return Err(e.to_string()),
            }
        }
    }

    fn run_input(&self, port: u16, udid: &str, rx: mpsc::Receiver<Vec<u8>>) {
        while !self.stopped() {
            let _ = self.read_input(port, udid, &rx);
            self.input.store(false, Ordering::SeqCst);
            for _ in 0..10 {
                if self.stopped() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }

    fn read_input(
        &self,
        port: u16,
        udid: &str,
        rx: &mpsc::Receiver<Vec<u8>>,
    ) -> Result<(), String> {
        let (mut socket, _) = tungstenite::connect(format!(
            "ws://127.0.0.1:{port}/vendor/serve-sim/helper/ws?device={udid}"
        ))
        .map_err(|e| e.to_string())?;
        if let tungstenite::stream::MaybeTlsStream::Plain(stream) = socket.get_ref() {
            stream
                .set_read_timeout(Some(Duration::from_millis(20)))
                .map_err(|e| e.to_string())?;
        }
        loop {
            if self.stopped() {
                let _ = socket.close(None);
                return Ok(());
            }
            while let Ok(packet) = rx.try_recv() {
                socket
                    .send(Message::binary(packet))
                    .map_err(|e| e.to_string())?;
            }
            match socket.read() {
                Ok(Message::Binary(bytes)) => match bytes.first().copied() {
                    Some(TAG_INPUT_ADMITTED) => self.input.store(true, Ordering::SeqCst),
                    Some(TAG_SCREEN_CONFIG) => {
                        if let Ok(payload) = serde_json::from_slice::<Value>(&bytes[1..]) {
                            if let Some(screen) = parse_screen(&payload) {
                                *self.screen.lock().unwrap() = Some(screen);
                            }
                            if payload.get("inputUnavailable").and_then(Value::as_bool)
                                == Some(true)
                            {
                                self.input.store(false, Ordering::SeqCst);
                            }
                        }
                    }
                    _ => {}
                },
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(e) => return Err(e.to_string()),
            }
        }
    }
}

/// A JPEG as the texture GPUI draws (BGRA), turned by `turns` quarter turns clockwise.
fn decode(jpeg: &[u8], turns: u8) -> Option<Frame> {
    let mut pixels = image::load_from_memory_with_format(jpeg, image::ImageFormat::Jpeg)
        .ok()?
        .to_rgba8();
    pixels = match turns {
        1 => image::imageops::rotate90(&pixels),
        2 => image::imageops::rotate180(&pixels),
        3 => image::imageops::rotate270(&pixels),
        _ => pixels,
    };
    for pixel in pixels.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    let (width, height) = pixels.dimensions();
    Some(Frame {
        image: Arc::new(RenderImage::new(vec![image::Frame::new(pixels)])),
        width,
        height,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn devices_parse_sort_and_skip_unavailable() {
        let list = json!({ "devices": {
            "com.apple.CoreSimulator.SimRuntime.iOS-17-5": [
                { "udid": "AAAA-1111", "name": "iPhone 15", "state": "Shutdown", "isAvailable": true },
                { "udid": "BBBB-2222", "name": "Old", "state": "Shutdown", "isAvailable": false }
            ],
            "com.apple.CoreSimulator.SimRuntime.iOS-18-1": [
                { "udid": "CCCC-3333", "name": "iPhone 16", "state": "Shutdown" },
                { "udid": "DDDD-4444", "name": "iPhone 16 Pro", "state": "Booted" }
            ],
            "com.apple.CoreSimulator.SimRuntime.watchOS-11": [
                { "udid": "EEEE-5555", "name": "Watch", "state": "Shutdown" }
            ],
            "com.apple.CoreSimulator.SimRuntime.iOS-16-0": [
                { "udid": "not hex!", "name": "Bad", "state": "Shutdown" }
            ]
        }});
        let devices = parse_devices(&list);
        let names: Vec<_> = devices.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["iPhone 16 Pro", "iPhone 16", "iPhone 15"]);
        assert!(devices[0].booted);
        assert_eq!(devices[1].runtime, "iOS 18.1");
    }

    #[test]
    fn splitter_keeps_the_newest_complete_frame_across_pushes() {
        let mut splitter = JpegSplitter::default();
        assert_eq!(
            splitter.push(b"--boundary\r\nContent-Type: image/jpeg\r\n\r\n"),
            None
        );
        assert_eq!(splitter.push(&[0xFF, 0xD8, 1, 2]), None);
        let frame = splitter.push(&[
            3, 0xFF, 0xD9, b'\r', 0xFF, 0xD8, 9, 0xFF, 0xD9, 0xFF, 0xD8, 5,
        ]);
        assert_eq!(frame, Some(vec![0xFF, 0xD8, 9, 0xFF, 0xD9]));
        assert_eq!(
            splitter.push(&[0xFF, 0xD9]),
            Some(vec![0xFF, 0xD8, 5, 0xFF, 0xD9])
        );
    }

    #[test]
    fn splitter_handles_a_marker_split_between_pushes() {
        let mut splitter = JpegSplitter::default();
        assert_eq!(splitter.push(&[0xFF]), None);
        assert_eq!(splitter.push(&[0xD8, 7, 0xFF]), None);
        assert_eq!(
            splitter.push(&[0xD9]),
            Some(vec![0xFF, 0xD8, 7, 0xFF, 0xD9])
        );
    }

    #[test]
    fn gate_bounds_the_frame_rate() {
        let mut gate = FrameGate::new(Duration::from_millis(66));
        let start = Instant::now();
        assert!(gate.allow(start));
        assert!(!gate.allow(start + Duration::from_millis(30)));
        assert!(gate.allow(start + Duration::from_millis(70)));
        assert!(!gate.allow(start + Duration::from_millis(100)));
    }

    #[test]
    fn touches_map_back_into_the_raw_frame() {
        let screen = |o: &str| Screen {
            width: 1170.,
            height: 2532.,
            orientation: o.into(),
        };
        assert_eq!(raw_point(None, 0.2, 0.3), (0.2, 0.3));
        assert_eq!(raw_point(Some(&screen("portrait")), 0.2, 0.3), (0.2, 0.3));
        assert_eq!(
            raw_point(Some(&screen("landscape_left")), 0.25, 0.5),
            (0.5, 0.75)
        );
        assert_eq!(
            raw_point(Some(&screen("landscape_right")), 0.25, 0.5),
            (0.5, 0.25)
        );
        assert_eq!(
            raw_point(Some(&screen("portrait_upside_down")), 0.25, 0.5),
            (0.75, 0.5)
        );
        // A landscape raw frame is already shown as is.
        let wide = Screen {
            width: 2532.,
            height: 1170.,
            orientation: "landscape_left".into(),
        };
        assert_eq!(raw_point(Some(&wide), 0.25, 0.5), (0.25, 0.5));
    }

    #[test]
    fn packets_are_tag_then_json() {
        let packet = touch_packet(None, "begin", 1.5, -1.0);
        assert_eq!(packet[0], 0x03);
        let body: Value = serde_json::from_slice(&packet[1..]).unwrap();
        assert_eq!(body, json!({ "type": "begin", "x": 1.0, "y": 0.0 }));
        assert_eq!(button_packet("home")[0], 0x04);
        assert_eq!(&button_packet("lock")[1..], br#"{"button":"lock"}"#);
        assert_eq!(orientation_packet("portrait")[0], 0x07);
    }

    #[test]
    fn keys_map_to_hid_and_shift_wraps_the_press() {
        assert_eq!(hid_usage("a"), Some(0x04));
        assert_eq!(hid_usage("z"), Some(0x1d));
        assert_eq!(hid_usage("1"), Some(0x1e));
        assert_eq!(hid_usage("0"), Some(0x27));
        assert_eq!(hid_usage("enter"), Some(0x28));
        assert_eq!(hid_usage("backspace"), Some(0x2a));
        assert_eq!(hid_usage("f5"), None);
        let plain = key_packets(0x04, false);
        assert_eq!(plain.len(), 2);
        let shifted = key_packets(0x04, true);
        assert_eq!(shifted.len(), 4);
        assert_eq!(&shifted[0][1..], br#"{"type":"down","usage":225}"#);
        assert_eq!(&shifted[3][1..], br#"{"type":"up","usage":225}"#);
    }

    #[test]
    fn orientation_cycles_and_screen_validates() {
        assert_eq!(next_orientation(None), "landscape_left");
        assert_eq!(next_orientation(Some("landscape_right")), "portrait");
        assert!(
            parse_screen(&json!({ "width": 10, "height": 20, "orientation": "sideways" }))
                .is_none()
        );
        assert!(
            parse_screen(&json!({ "width": 0, "height": 20, "orientation": "portrait" })).is_none()
        );
        assert!(
            parse_screen(&json!({ "width": 10, "height": 20, "orientation": "portrait" }))
                .is_some()
        );
    }

    #[test]
    fn decoded_frames_turn_by_quarter_turns() {
        let mut jpeg = Vec::new();
        let source = image::RgbImage::from_pixel(8, 16, image::Rgb([200, 30, 30]));
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .encode_image(&source)
            .unwrap();
        let upright = decode(&jpeg, 0).unwrap();
        assert_eq!((upright.width, upright.height), (8, 16));
        let sideways = decode(&jpeg, 1).unwrap();
        assert_eq!((sideways.width, sideways.height), (16, 8));
        assert!(decode(b"not a jpeg", 0).is_none());
    }

    #[test]
    fn frame_fits_the_stage_at_the_device_aspect() {
        assert_eq!(fit_frame(0.5, 400., 400.), (200., 400.));
        assert_eq!(fit_frame(0.5, 100., 400.), (100., 200.));
        assert_eq!(fit_frame(0.5, 0., 400.), (0., 0.));
    }

    #[test]
    fn setup_follows_the_missing_piece() {
        let node = |major| Node {
            path: "/nonexistent/node".into(),
            version: format!("v{major}.0.0"),
            major,
        };
        let tc = |xcode, node: Option<Node>, entry: bool| Toolchain {
            xcode,
            node,
            entry: entry.then(|| "/x".into()),
        };
        assert_eq!(setup(&tc(false, None, false)), Setup::NeedsXcode);
        assert_eq!(setup(&tc(true, None, false)), Setup::NeedsNode(None));
        // Node 20 is too old; Node 22 without npm beside it is unusable too.
        assert_eq!(
            setup(&tc(true, Some(node(20)), true)),
            Setup::NeedsNode(Some("v20.0.0".into()))
        );
        assert_eq!(
            setup(&tc(true, Some(node(22)), true)),
            Setup::NeedsNode(Some("v22.0.0".into()))
        );
        assert_eq!(node_major("v22.11.0"), Some(22));
        assert_eq!(node_major("22"), None);
        assert!(valid_udid("AB12-34cd") && !valid_udid("a b") && !valid_udid(""));
    }
}
