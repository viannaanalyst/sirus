//! `sirus-host` (ADR-131): Sirus without a window: the back end the Mac window and the phone
//! talk to, or a server reached over Tailscale. It loads the state, runs the agents and serves
//! remote access (`remote.rs`); the command table is generated from `commands.list`
//! (`build.rs`), so every command takes the same code path.

use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};

use crate::commands::AppState;
use crate::remote::host as remote_host;
use crate::runtime::{AppHandle, Manager};

include!(concat!(env!("OUT_DIR"), "/host_commands.rs"));

const USAGE: &str = "\
sirus-host: Sirus Code without a window (ADR-131)

Usage:
  sirus-host serve [--data-dir DIR] [--bind ADDR] [--port PORT] [--assets-dir DIR] [--url URL]
                   [--local-port PORT] [--https-port PORT] [--label NAME]
  sirus-host pair [--data-dir DIR]      one-time six-digit code and QR for a new device
  sirus-host devices [--data-dir DIR]   paired devices
  sirus-host revoke <device-id> [--data-dir DIR]

Options:
  --data-dir DIR  state folder (default $SIRUS_DATA_DIR, else $XDG_DATA_HOME/sirus-code,
                  else ~/.local/share/sirus-code)
  --bind ADDR     address to listen on (default: this machine's Tailscale address,
                  else 127.0.0.1; 0.0.0.0 only when asked)
  --port PORT     default 7710
  --assets-dir DIR  the Mermaid and KaTeX files the Sirus window loads, served at /assets/;
                  default $SIRUS_ASSETS_DIR, else `reply-render/` next to the binary'
  --url URL       address to show in pairing links (e.g. http://host.tailnet.ts.net:7710)
  --local-port P  a window's own listener on 127.0.0.1:P, always on (Sirus Native, ADR-189).
                  --port is then the listener for other devices, on only while Settings →
                  Connections allows it (`remote.json`), on every address but Tailscale
                  peers only
  --https-port P  the `tailscale serve` HTTPS port (default 443; Sirus Native uses 8443 so a
                  Sirus Code on the same Mac keeps 443)
  --label NAME    appended to this computer's name on the phone's list
";

/// The command a paired device sent, run as the window would run it.
pub async fn invoke(app: &AppHandle, command: &str, args: &Value) -> Result<String, Value> {
    generated_call(app, command, args).await
}

/// Tauri's argument rule: a missing key reads as `null` (so `Option` is `None`).
fn arg<T: DeserializeOwned>(args: &Value, key: &str, command: &str) -> Result<T, Value> {
    serde_json::from_value(args.get(key).cloned().unwrap_or(Value::Null)).map_err(|error| {
        Value::String(format!(
            "invalid args `{key}` for command `{command}`: {error}"
        ))
    })
}

fn reply_result<T: Serialize, E: Serialize>(result: Result<T, E>) -> Result<String, Value> {
    match result {
        Ok(value) => reply_value(value),
        Err(error) => Err(serde_json::to_value(error).unwrap_or(Value::Null)),
    }
}

fn reply_value<T: Serialize>(value: T) -> Result<String, Value> {
    serde_json::to_string(&value)
        .map_err(|error| json!({ "code": "serialize", "message": error.to_string() }))
}

async fn blocking(
    run: impl FnOnce() -> Result<String, Value> + Send + 'static,
) -> Result<String, Value> {
    crate::runtime::async_runtime::spawn_blocking(run)
        .await
        .unwrap_or_else(|_| Err(json!({ "code": "host", "message": "The command failed." })))
}

fn unavailable(command: &str) -> Value {
    json!({
        "code": "remote",
        "message": format!("`{command}` is only available in the Mac app."),
    })
}

struct Cli {
    command: String,
    positional: Vec<String>,
    data_dir: Option<PathBuf>,
    bind: Option<IpAddr>,
    port: u16,
    assets_dir: Option<PathBuf>,
    url: Option<String>,
    local_port: Option<u16>,
    https_port: Option<u16>,
    label: Option<String>,
}

fn parse(arguments: &[String]) -> Result<Cli, String> {
    let mut cli = Cli {
        command: String::new(),
        positional: Vec::new(),
        data_dir: None,
        bind: None,
        port: crate::remote::DEFAULT_PORT,
        assets_dir: None,
        url: None,
        local_port: None,
        https_port: None,
        label: None,
    };
    let mut iter = arguments.iter();
    while let Some(argument) = iter.next() {
        let mut value = |name: &str| {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match argument.as_str() {
            "--data-dir" => cli.data_dir = Some(absolute(value("--data-dir")?)?),
            "--assets-dir" => cli.assets_dir = Some(absolute(value("--assets-dir")?)?),
            "--bind" => {
                cli.bind = Some(
                    value("--bind")?
                        .parse()
                        .map_err(|_| "--bind takes an IP address".to_string())?,
                )
            }
            "--port" => {
                cli.port = value("--port")?
                    .parse()
                    .ok()
                    .filter(|port| *port > 0)
                    .ok_or_else(|| "--port takes a number from 1 to 65535".to_string())?
            }
            "--local-port" => {
                cli.local_port = Some(
                    value("--local-port")?
                        .parse()
                        .ok()
                        .filter(|port| *port > 0)
                        .ok_or_else(|| "--local-port takes a number from 1 to 65535".to_string())?,
                )
            }
            "--https-port" => {
                cli.https_port = Some(
                    value("--https-port")?
                        .parse()
                        .ok()
                        .filter(|port| [443, 8443, 10000].contains(port))
                        .ok_or_else(|| "--https-port is 443, 8443 or 10000".to_string())?,
                )
            }
            "--label" => {
                let label = value("--label")?;
                if label.is_empty() || label.len() > 40 || label.chars().any(char::is_control) {
                    return Err("--label takes a short one-line name".into());
                }
                cli.label = Some(label);
            }
            "--url" => {
                let url = value("--url")?;
                if !(url.starts_with("http://") || url.starts_with("https://"))
                    || url.chars().any(char::is_whitespace)
                {
                    return Err("--url takes an http(s) address".into());
                }
                cli.url = Some(url.trim_end_matches('/').to_string());
            }
            "-h" | "--help" | "help" => cli.command = "help".into(),
            "-V" | "--version" => cli.command = "version".into(),
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}")),
            word if cli.command.is_empty() => cli.command = word.into(),
            word => cli.positional.push(word.into()),
        }
    }
    Ok(cli)
}

fn absolute(path: String) -> Result<PathBuf, String> {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        Ok(path)
    } else {
        std::env::current_dir()
            .map(|dir| dir.join(path))
            .map_err(|error| error.to_string())
    }
}

fn data_dir(cli: &Cli) -> PathBuf {
    if let Some(dir) = &cli.data_dir {
        return dir.clone();
    }
    if let Some(dir) = std::env::var_os("SIRUS_DATA_DIR").map(PathBuf::from) {
        if dir.is_absolute() {
            return dir;
        }
    }
    if let Some(dir) = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
        if dir.is_absolute() {
            return dir.join("sirus-code");
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    home.join(".local/share/sirus-code")
}

fn assets_dir(cli: &Cli) -> Option<PathBuf> {
    cli.assets_dir
        .clone()
        .or_else(|| std::env::var_os("SIRUS_ASSETS_DIR").map(PathBuf::from))
        .or_else(|| {
            let beside = std::env::current_exe().ok()?.parent()?.join("reply-render");
            beside.join("mermaid.js").is_file().then_some(beside)
        })
}

/// Entry point of the `sirus-host` binary; returns the process exit code.
pub fn main() -> i32 {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    // Provider CLIs start this binary as their Sirus tools server (`browser_mcp.rs`).
    if arguments.iter().any(|argument| argument == "--mcp-browser") {
        return crate::browser_mcp::run_stdio();
    }
    // The same binary is the computer-use tools server (`computer_mcp.rs`).
    if arguments
        .iter()
        .any(|argument| argument == "--mcp-computer")
    {
        return crate::computer_mcp::run_stdio();
    }
    let cli = match parse(&arguments) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("sirus-host: {message}\n\n{USAGE}");
            return 2;
        }
    };
    match cli.command.as_str() {
        "serve" => serve(&cli),
        "pair" => pair(&cli),
        "devices" => devices(&cli),
        "revoke" => revoke(&cli),
        "version" => {
            println!("sirus-host {}", env!("CARGO_PKG_VERSION"));
            0
        }
        "help" | "" => {
            print!("{USAGE}");
            0
        }
        other => {
            eprintln!("sirus-host: unknown command {other}\n\n{USAGE}");
            2
        }
    }
}

fn private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

fn serve(cli: &Cli) -> i32 {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("sirus_code=info")),
        )
        .init();
    let dir = data_dir(cli);
    if let Err(error) = private_dir(&dir) {
        eprintln!("sirus-host: cannot use {}: {error}", dir.display());
        return 1;
    }
    let assets = assets_dir(cli);
    if let Some(assets) = &assets {
        if !assets.join("mermaid.js").is_file() {
            eprintln!("sirus-host: {} has no mermaid.js", assets.display());
            return 1;
        }
    }
    let app = match start(&dir) {
        Ok(app) => app,
        Err(error) => {
            eprintln!(
                "sirus-host: cannot load state from {}: {error}",
                dir.display()
            );
            return 1;
        }
    };
    // Never every address by default: the tailnet address, or loopback for `tailscale serve`.
    // With a window's own listener, the one for other devices listens on every address like the
    // Mac app does and refuses all but loopback and Tailscale peers.
    let windowed = cli.local_port.is_some();
    let bind = cli.bind.unwrap_or_else(|| {
        if windowed {
            return IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED);
        }
        remote_host::tailnet_address()
            .map(IpAddr::V4)
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST))
    });
    let options = remote_host::Options {
        bind,
        port: cli.port,
        url: cli.url.clone(),
        assets_dir: assets,
        local_port: cli.local_port,
        https_port: cli.https_port,
        label: cli.label.clone(),
    };
    let code = crate::runtime::async_runtime::block_on(async {
        let urls = match remote_host::start_host(app.clone(), &dir, options).await {
            Ok(urls) => urls,
            Err(error) => {
                eprintln!("sirus-host: {error}");
                return 1;
            }
        };
        if let Err(error) = remote_host::serve_control(app.clone(), &dir).await {
            eprintln!("sirus-host: cannot open the control socket: {error}");
            return 1;
        }
        if let Some(local) = cli.local_port {
            println!("Sirus host: window on 127.0.0.1:{local}");
            if urls.is_empty() {
                println!(
                    "  other devices: off (Settings → Connections turns them on, port {})",
                    cli.port
                );
            } else {
                for url in &urls {
                    println!("  other devices: {url}");
                }
            }
        } else {
            println!("Sirus host listening on {bind}:{}", cli.port);
            for url in &urls {
                println!("  open {url}");
            }
            if urls.is_empty() {
                println!("  (no address to show: pass --url, or put `tailscale serve` in front)");
            }
        }
        println!("Data: {}", dir.display());
        println!("Pair a device: sirus-host pair{}", data_dir_hint(cli));
        wait_for_signal().await;
        0
    });
    stop(&app);
    let _ = std::fs::remove_file(dir.join(remote_host::CONTROL_SOCKET));
    code
}

fn data_dir_hint(cli: &Cli) -> String {
    cli.data_dir
        .as_ref()
        .map(|dir| format!(" --data-dir {}", dir.display()))
        .unwrap_or_default()
}

async fn wait_for_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    let mut terminate = signal(SignalKind::terminate()).ok();
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = async {
            match terminate.as_mut() {
                Some(terminate) => { terminate.recv().await; }
                None => std::future::pending::<()>().await,
            }
        } => {}
    }
    tracing::info!("sirus-host stopping");
}

/// The same start-up as the window's `setup` (lib.rs), without anything that needs a screen.
fn start(dir: &Path) -> crate::error::Result<AppHandle> {
    let app = AppHandle::new(dir.to_path_buf());
    app.manage(crate::html_preview::HtmlPreviews::default());
    let data_path = dir.join("state.json");
    crate::browser_mcp::init(app.clone(), dir.to_path_buf());
    crate::skills::publish(dir);
    crate::tool_images::init(dir);
    crate::secrets::init(dir);
    let worktree_root = dir.join("worktrees");
    std::fs::create_dir_all(&worktree_root)?;
    let mut data = crate::persist::load_or_create(&data_path)?;
    crate::sirus_tools::set_enabled(data.settings.agents_manage_sessions);
    crate::browser_mcp::set_agent_access(data.settings.agent_browser_access);
    crate::agent_pool::set_limit(data.settings.keep_idle_agents as usize);
    crate::connectors::start(dir, &data_path, &mut data);
    // Computer use drives this Mac's own apps (macOS only; elsewhere it answers "only on macOS").
    // The approval cards reach the local window client as `computer-state` (ADR-189).
    crate::computer_mcp::init(
        app.clone(),
        dir.to_path_buf(),
        data.settings.computer_use_enabled,
    );
    app.manage(Arc::new(AppState::new(data_path, worktree_root, data)));
    crate::archive_removal::delete_archived(app.state::<Arc<AppState>>().inner());
    crate::notifications::install(&app);
    crate::crash_resume::resume_interrupted(&app);
    crate::snooze::start(app.clone());
    crate::idle_compaction::start(app.clone());
    crate::auto_pull::start(app.clone());
    crate::ci_autofix::start(app.clone());
    crate::pr_watch::start(app.clone());
    Ok(app)
}

/// The same clean-up as the window's `RunEvent::Exit`.
fn stop(app: &AppHandle) {
    crate::secrets::clear_all();
    crate::remote::shutdown();
    let state = app.state::<Arc<AppState>>();
    if let Err(error) = state.shutdown() {
        tracing::error!(%error, "cannot persist host shutdown");
    }
    crate::process_tree::finish_pending();
    crate::keep_awake::release_all();
}

fn not_running(cli: &Cli) -> i32 {
    eprintln!(
        "sirus-host: no host is running for {}. Start `sirus-host serve` first.",
        data_dir(cli).display()
    );
    1
}

fn error_message(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| error.to_string())
}

fn pair(cli: &Cli) -> i32 {
    // `pair window`: the code Sirus Native's own window redeems (ADR-189).
    let window = cli.positional.first().is_some_and(|word| word == "window");
    let request = json!({ "type": if window { "pairWindow" } else { "pair" } });
    let Some(reply) = remote_host::control_request(&data_dir(cli), &request) else {
        return not_running(cli);
    };
    let pairing = match reply {
        Ok(pairing) => pairing,
        Err(error) => {
            eprintln!("sirus-host: {}", error_message(&error));
            return 1;
        }
    };
    let code = pairing["code"].as_str().unwrap_or_default();
    let urls: Vec<&str> = pairing["urls"]
        .as_array()
        .map(|urls| urls.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    // The iPhone app's link (public route, ADR-155) comes first: a scan opens the app.
    let app_url = pairing["appUrl"].as_str();
    if let Some(qr) = app_url
        .or(urls.first().copied())
        .and_then(remote_host::qr_text)
    {
        println!("{qr}");
    }
    println!(
        "Pairing code: {} {}",
        &code[..code.len().min(3)],
        &code[code.len().min(3)..]
    );
    if app_url.is_some() {
        println!("  Scan with the iPhone camera to add this computer to the Sirus app.");
    }
    for url in &urls {
        println!("  {url}");
    }
    println!(
        "Single use, valid until {}.",
        pairing["expiresAt"]
            .as_str()
            .unwrap_or("five minutes from now")
    );
    0
}

fn devices(cli: &Cli) -> i32 {
    let dir = data_dir(cli);
    let devices: Vec<String> =
        match remote_host::control_request(&dir, &json!({ "type": "status" })) {
            Some(Ok(status)) => status["devices"]
                .as_array()
                .map(|devices| {
                    devices
                        .iter()
                        .map(|device| {
                            format!(
                                "{}  {}  paired {}  last seen {}{}",
                                device["id"].as_str().unwrap_or_default(),
                                device["name"].as_str().unwrap_or_default(),
                                device["createdAt"].as_str().unwrap_or_default(),
                                device["lastSeen"].as_str().unwrap_or("never"),
                                if device["push"] == true {
                                    "  alerts on"
                                } else {
                                    ""
                                },
                            )
                        })
                        .collect()
                })
                .unwrap_or_default(),
            Some(Err(error)) => {
                eprintln!("sirus-host: {}", error_message(&error));
                return 1;
            }
            None => remote_host::saved_devices(&dir)
                .iter()
                .map(|device| device.summary())
                .collect(),
        };
    if devices.is_empty() {
        println!("No paired devices.");
    }
    for device in devices {
        println!("{device}");
    }
    0
}

fn revoke(cli: &Cli) -> i32 {
    let [id] = cli.positional.as_slice() else {
        eprintln!("sirus-host: revoke takes one device id (see `sirus-host devices`)");
        return 2;
    };
    let dir = data_dir(cli);
    match remote_host::control_request(&dir, &json!({ "type": "revoke", "deviceId": id })) {
        Some(Ok(_)) => {}
        Some(Err(error)) => {
            eprintln!("sirus-host: {}", error_message(&error));
            return 1;
        }
        None => match remote_host::revoke_saved(&dir, id) {
            Ok(true) => {}
            Ok(false) => {
                eprintln!("sirus-host: no paired device has this id");
                return 1;
            }
            Err(error) => {
                eprintln!("sirus-host: {error}");
                return 1;
            }
        },
    }
    println!("Revoked {id}; its open connections were closed.");
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        text.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn the_cli_reads_its_options() {
        let cli = parse(&words(
            "serve --bind 100.64.1.2 --port 7800 --data-dir /srv/sirus --assets-dir /srv/assets --url https://vps.example.ts.net/",
        ))
        .unwrap();
        assert_eq!(cli.command, "serve");
        assert_eq!(cli.bind, Some("100.64.1.2".parse().unwrap()));
        assert_eq!(cli.port, 7800);
        assert_eq!(cli.data_dir, Some(PathBuf::from("/srv/sirus")));
        assert_eq!(cli.url.as_deref(), Some("https://vps.example.ts.net"));
        let revoke = parse(&words("revoke abc")).unwrap();
        assert_eq!(revoke.positional, ["abc"]);
        for bad in [
            "serve --bind nowhere",
            "serve --port 0",
            "serve --url ftp://x",
            "serve --what",
            "serve --port",
        ] {
            assert!(parse(&words(bad)).is_err(), "{bad}");
        }
        assert_eq!(
            parse(&words("serve")).unwrap().port,
            crate::remote::DEFAULT_PORT
        );
    }

    #[test]
    fn arguments_follow_tauri_rules() {
        let args = json!({ "sessionId": "s1", "count": 2 });
        assert_eq!(arg::<String>(&args, "sessionId", "x").unwrap(), "s1");
        assert_eq!(arg::<Option<String>>(&args, "missing", "x").unwrap(), None);
        assert!(arg::<String>(&args, "missing", "x").is_err());
        assert!(arg::<String>(&args, "count", "x").is_err());
        assert_eq!(reply_value(json!({ "a": 1 })).unwrap(), r#"{"a":1}"#);
        let failed: Result<(), crate::error::Error> = Err(crate::error::Error::new("x", "nope"));
        assert_eq!(reply_result(failed).unwrap_err()["message"], "nope");
    }

    #[test]
    fn the_command_table_covers_what_the_phone_uses() {
        // The phone app's everyday commands run on the host…
        for command in [
            "load_state",
            "host_info",
            "transcript_action",
            "create_session",
            "send_prompt",
            "stop_agent",
            "respond_agent_request",
            "start_terminal",
            "write_terminal",
            "git_status",
            "git_diff",
            "list_dir",
            "detect_agents",
            "save_settings",
            "paste_prompt_attachments",
            "usage_summary",
            "phone_look_get",
            "phone_look_set",
        ] {
            assert!(HOST_COMMANDS.contains(&command), "{command}");
        }
        // …and nothing for a window or for web push (there is no Tauri window any more).
        for command in [
            "browser_open",
            "window_ready",
            "pick_folder",
            "open_external_url",
            "share_push_identity",
        ] {
            assert!(!HOST_COMMANDS.contains(&command), "{command}");
        }
    }

    #[test]
    fn unknown_commands_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let app = AppHandle::new(dir.path().to_path_buf());
        let reply =
            crate::runtime::async_runtime::block_on(invoke(&app, "browser_open", &json!({})));
        assert_eq!(reply.unwrap_err()["code"], "remote");
    }
}
