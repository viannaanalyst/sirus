//! HTTPS for remote access (ADR-082) through the person's own Tailscale:
//! `tailscale serve` puts a certificate for `<mac>.<tailnet>.ts.net` in front of
//! the local port. Fixed argv only; the CLI is looked up in fixed places.

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio::io::AsyncReadExt;

/// The app's own binary first: `/usr/local/bin/tailscale` is often a shell wrapper,
/// and stopping the wrapper would leave a waiting `serve` behind.
const CLI: &[&str] = &[
    "/Applications/Tailscale.app/Contents/MacOS/Tailscale",
    "/opt/homebrew/bin/tailscale",
    "/usr/local/bin/tailscale",
    // Linux packages (the headless host, ADR-131).
    "/usr/bin/tailscale",
];
const QUICK: Duration = Duration::from_secs(8);
/// `serve` waits while the tailnet still needs HTTPS turned on; its message is enough.
const SERVE: Duration = Duration::from_secs(20);
const OUTPUT_LIMIT: usize = 256 * 1024;

pub fn cli() -> Option<PathBuf> {
    CLI.iter().map(PathBuf::from).find(|path| path.is_file())
}

/// Combined output and whether the command exited successfully in time.
async fn run(args: &[&str], limit: Duration) -> Option<(bool, String)> {
    run_until(args, limit, |_| false).await
}

/// Like [`run`], but stops as soon as the output says enough (`serve` keeps waiting otherwise).
async fn run_until(
    args: &[&str],
    limit: Duration,
    enough: fn(&str) -> bool,
) -> Option<(bool, String)> {
    let mut child = tokio::process::Command::new(cli()?)
        .args(args)
        // Started from an app (no terminal environment), Tailscale's macOS binary tries to
        // open its GUI and fails ("CLIError error 3"); this makes it act as the CLI.
        .env("TAILSCALE_BE_CLI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let output = Arc::new(parking_lot::Mutex::new(Vec::new()));
    let mut readers = Vec::new();
    for mut pipe in [
        child
            .stdout
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
        child
            .stderr
            .take()
            .map(|pipe| Box::new(pipe) as Box<dyn tokio::io::AsyncRead + Unpin + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let output = output.clone();
        readers.push(tokio::spawn(async move {
            let mut chunk = [0u8; 4096];
            while let Ok(read) = pipe.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                let mut output = output.lock();
                if output.len() < OUTPUT_LIMIT {
                    output.extend_from_slice(&chunk[..read]);
                }
            }
        }));
    }
    let deadline = tokio::time::Instant::now() + limit;
    let success = loop {
        match tokio::time::timeout(Duration::from_millis(200), child.wait()).await {
            Ok(Ok(status)) => break status.success(),
            Ok(Err(_)) => break false,
            Err(_) => {
                let seen = enough(&String::from_utf8_lossy(&output.lock()));
                if seen || tokio::time::Instant::now() >= deadline {
                    let _ = child.kill().await;
                    break false;
                }
            }
        }
    };
    for reader in readers {
        let _ = tokio::time::timeout(Duration::from_millis(500), reader).await;
    }
    let text = String::from_utf8_lossy(&output.lock()).into_owned();
    Some((success, text))
}

/// This Mac's MagicDNS name, without the trailing dot.
pub async fn dns_name() -> Option<String> {
    let (ok, text) = run(&["status", "--json"], QUICK).await?;
    if !ok {
        return None;
    }
    let value: Value = serde_json::from_str(&text).ok()?;
    let name = value["Self"]["DNSName"].as_str()?.trim_end_matches('.');
    valid_host(name).then(|| name.to_ascii_lowercase())
}

/// Whether `serve` already proxies HTTPS to this port.
pub async fn serving(port: u16) -> bool {
    match run(&["serve", "status", "--json"], QUICK).await {
        Some((true, text)) => proxies_port(&text, port),
        _ => false,
    }
}

pub fn proxies_port(status: &str, port: u16) -> bool {
    let target = format!("127.0.0.1:{port}");
    let local = format!("localhost:{port}");
    status.contains(&format!("http://{target}")) || status.contains(&format!("http://{local}"))
}

pub enum Enable {
    Done,
    /// The tailnet must allow HTTPS first; the page that does it, when the CLI named one.
    NeedsSetup(Option<String>),
    Failed(String),
}

pub async fn enable(port: u16, https_port: u16) -> Enable {
    let target = format!("http://127.0.0.1:{port}");
    let flag = format!("--https={https_port}");
    let Some((ok, text)) = run_until(&["serve", "--bg", &flag, &target], SERVE, |text| {
        setup_url(text).is_some()
    })
    .await
    else {
        return Enable::Failed("Tailscale's command-line tool was not found.".into());
    };
    if ok && serving(port).await {
        return Enable::Done;
    }
    let url = setup_url(&text);
    if url.is_some() || text.to_ascii_lowercase().contains("not enabled") {
        return Enable::NeedsSetup(url);
    }
    Enable::Failed(
        text.lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("Tailscale could not serve HTTPS.")
            .chars()
            .take(240)
            .collect(),
    )
}

pub async fn disable(https_port: u16) {
    let _ = run(&["serve", &format!("--https={https_port}"), "off"], QUICK).await;
}

/// The first Tailscale admin link in the CLI's message.
pub fn setup_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|word| {
            word.trim_matches(|character: char| {
                matches!(character, '"' | '\'' | '(' | ')' | ',' | '.' | '<' | '>')
            })
        })
        .find(|word| is_setup_url(word))
        .map(str::to_string)
}

/// Only Tailscale's own admin pages may be opened from here.
pub fn is_setup_url(url: &str) -> bool {
    url.strip_prefix("https://login.tailscale.com/")
        .is_some_and(|rest| {
            !rest.is_empty()
                && rest.chars().all(|character| {
                    character.is_ascii_alphanumeric() || "/?=&-_.%".contains(character)
                })
        })
}

pub fn valid_host(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 253
        && name.ends_with(".ts.net")
        && name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serve_status_names_the_port() {
        let status = r#"{"TCP":{"443":{"HTTPS":true}},"Web":{"mac.tail6e87c3.ts.net:443":{"Handlers":{"/":{"Proxy":"http://127.0.0.1:7710"}}}}}"#;
        assert!(proxies_port(status, 7710));
        assert!(!proxies_port(status, 7799));
        assert!(!proxies_port("{}", 7710));
    }

    #[test]
    fn setup_links_come_only_from_tailscale() {
        // As Tailscale 1.102 prints it, then waits until HTTPS is allowed.
        let message = "\nServe is not enabled on your tailnet.\nTo enable, visit:\n\n         https://login.tailscale.com/f/serve?node=n8xuaFxRTV11CNTRL\n";
        assert_eq!(
            setup_url(message).as_deref(),
            Some("https://login.tailscale.com/f/serve?node=n8xuaFxRTV11CNTRL")
        );
        assert_eq!(setup_url("visit https://evil.example/f/serve"), None);
        assert!(!is_setup_url("https://login.tailscale.com.evil.example/x"));
        assert!(!is_setup_url("https://login.tailscale.com/"));
    }

    #[test]
    fn hosts_are_tailnet_names() {
        assert!(valid_host("macbook-air-de-gabriel.tail6e87c3.ts.net"));
        assert!(!valid_host("example.com"));
        assert!(!valid_host("bad host.ts.net"));
    }
}
