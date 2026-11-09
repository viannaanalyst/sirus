//! The computers this window can use (ADR-133, option A): this Mac, and the Sirus hosts paired
//! from Settings → Conexões. One is active and the whole window follows it. The list (name and
//! address, no secrets) is a small file in the data folder; each token is a Keychain item of its
//! own, read and written in this process only.
use crate::i18n::t;
use crate::remote_tls::Address;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::PathBuf;

/// The id of this Mac in every list and route.
pub const HOME: &str = "home";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Computer {
    pub id: String,
    pub name: String,
    /// `https://host[:port]` as `Address::display` writes it.
    pub address: String,
}

impl Computer {
    pub fn address(&self) -> Result<Address, String> {
        Address::parse(&self.address)
    }
}

#[derive(Default, Serialize, Deserialize)]
struct File {
    #[serde(default)]
    computers: Vec<Computer>,
}

fn file() -> PathBuf {
    crate::backend::data_dir().join("native-computers.json")
}

/// The paired computers, without this Mac.
pub fn load() -> Vec<Computer> {
    std::fs::read_to_string(file())
        .ok()
        .and_then(|text| serde_json::from_str::<File>(&text).ok())
        .map(|file| file.computers)
        .unwrap_or_default()
}

fn save(computers: &[Computer]) -> Result<(), String> {
    let text = serde_json::to_string_pretty(&File {
        computers: computers.to_vec(),
    })
    .map_err(|e| e.to_string())?;
    let path = file();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    std::fs::write(path, text).map_err(|e| e.to_string())
}

/// Adds a computer, or replaces the one at the same address (pairing again after a revoke).
pub fn upsert(list: &mut Vec<Computer>, name: &str, address: &Address) -> Computer {
    let shown = address.display();
    if let Some(known) = list.iter_mut().find(|c| c.address == shown) {
        known.name = name.to_owned();
        return known.clone();
    }
    let computer = Computer {
        id: format!("pc-{}", address.host.replace('.', "-")),
        name: name.to_owned(),
        address: shown,
    };
    let mut id = computer.id.clone();
    let mut n = 2;
    while list.iter().any(|c| c.id == id) {
        id = format!("{}-{n}", computer.id);
        n += 1;
    }
    let computer = Computer { id, ..computer };
    list.push(computer.clone());
    computer
}

/// Where the tokens are kept. The Keychain in the app, a map in tests.
pub trait Tokens {
    fn get(&self, id: &str) -> Option<String>;
    fn set(&self, id: &str, token: &str) -> Result<(), String>;
    fn remove(&self, id: &str);
}

/// One Keychain service for the window's computers, apart from the host's own items
/// (`SIRUS_KEYCHAIN_APP="Sirus Native"` keeps naming those `Sirus Native <kind>`); the account is
/// the computer's id, so every computer has its own item.
pub struct Keychain;

const SERVICE: &str = "Sirus Native computers";

#[cfg(target_os = "macos")]
impl Tokens for Keychain {
    fn get(&self, id: &str) -> Option<String> {
        let bytes = security_framework::passwords::get_generic_password(SERVICE, id).ok()?;
        String::from_utf8(bytes).ok().filter(|t| !t.is_empty())
    }
    fn set(&self, id: &str, token: &str) -> Result<(), String> {
        security_framework::passwords::set_generic_password(SERVICE, id, token.as_bytes())
            .map_err(|e| e.to_string())
    }
    fn remove(&self, id: &str) {
        let _ = security_framework::passwords::delete_generic_password(SERVICE, id);
    }
}

#[cfg(not(target_os = "macos"))]
impl Tokens for Keychain {
    fn get(&self, _: &str) -> Option<String> {
        None
    }
    fn set(&self, _: &str, _: &str) -> Result<(), String> {
        Err(t("O Keychain só existe no macOS.").into())
    }
    fn remove(&self, _: &str) {}
}

/// Pairs with the computer at `address` using its six-digit code and stores the token. Returns
/// the computer as listed. Nothing here logs or returns the token.
pub fn pair(
    address: &Address,
    code: &str,
    list: &mut Vec<Computer>,
    tokens: &dyn Tokens,
) -> Result<Computer, String> {
    let code = normalize_code(code).ok_or(t("O código tem seis dígitos."))?;
    let body = json!({ "code": code, "name": "Sirus" }).to_string();
    let (status, reply) = crate::remote_tls::post_json(address, "/api/pair", &body)?;
    let value: Value = serde_json::from_str(&reply).unwrap_or(Value::Null);
    let token = match value["token"].as_str() {
        Some(token) if status == 200 => token,
        _ => {
            return Err(value["message"]
                .as_str()
                .unwrap_or(t("O computador recusou o código."))
                .to_owned());
        }
    };
    let name = value["host"]["hostName"]
        .as_str()
        .filter(|n| !n.is_empty())
        .unwrap_or(&address.host);
    let computer = upsert(list, name, address);
    tokens.set(&computer.id, token)?;
    save(list)?;
    Ok(computer)
}

/// Six digits, typed or pasted as "482 913" or "482-913".
pub fn normalize_code(text: &str) -> Option<String> {
    let digits: String = text
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect();
    (digits.len() == 6 && digits.bytes().all(|b| b.is_ascii_digit())).then_some(digits)
}

/// Forgets a computer here: the list entry and its token. The host keeps the device until it is
/// revoked there.
pub fn forget(list: &mut Vec<Computer>, id: &str, tokens: &dyn Tokens) -> Result<(), String> {
    list.retain(|c| c.id != id);
    tokens.remove(id);
    save(list)
}

/// Commands that always run on this Mac, whatever computer is active: they act on the window, its
/// pickers, the embedded browser and devices, saved logins and remote access itself. This is
/// `LOCAL_COMMANDS` of `src/lib/computers.ts` plus what a host refuses to a paired device
/// (`DENIED` in `remote.rs`), and this app's own `native_*` actions.
const LOCAL_COMMANDS: &[&str] = &[
    "capture_prompt_window",
    "chat_background_action",
    "chat_background_image",
    "computer_action",
    "device_action",
    "dictation_level",
    "dictation_status",
    "drop_prompt_attachments",
    "app_update_action",
    "dismiss_provider_update",
    "notification_action",
    "phone_look_get",
    "phone_look_set",
    "provider_updates",
    "register_browser_client",
    "share_push_identity",
    "update_providers",
    "start_dictation",
    "stop_dictation",
    "take_prompt_link",
    "vscode_themes_import_local",
    "window_ready",
    "window_snap_action",
];
const LOCAL_PREFIXES: &[&str] = &["browser_", "login_vault_", "native_", "pick_", "remote_"];

pub fn runs_here(command: &str) -> bool {
    LOCAL_COMMANDS.contains(&command) || LOCAL_PREFIXES.iter().any(|p| command.starts_with(p))
}

/// Events only this Mac's window raises; they keep arriving from it while a server is active.
const LOCAL_EVENTS: &[&str] = &[
    "app-update",
    "browser-record-request",
    "browser-state",
    "browser_tool_request",
    "computer-state",
    "device-changed",
    "login-vault-state",
    "memory-pressure",
    "notification-open",
    "prompt-link",
    "provider-updates",
    "quit-armed",
    "remote-changed",
    "window-snap",
];

pub fn event_is_local(event: &str) -> bool {
    LOCAL_EVENTS.contains(&event)
}

/// Where a connection to another computer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Connecting,
    Open,
    /// Dropped; the connection retries by itself.
    Lost,
    /// The host no longer knows this device: pair again.
    Unauthorized,
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[derive(Default)]
    pub struct Memory(pub RefCell<HashMap<String, String>>);
    impl Tokens for Memory {
        fn get(&self, id: &str) -> Option<String> {
            self.0.borrow().get(id).cloned()
        }
        fn set(&self, id: &str, token: &str) -> Result<(), String> {
            self.0.borrow_mut().insert(id.into(), token.into());
            Ok(())
        }
        fn remove(&self, id: &str) {
            self.0.borrow_mut().remove(id);
        }
    }

    #[test]
    fn local_commands_follow_the_denied_list_and_prefixes() {
        for c in [
            "remote_action",
            "remote_pair",
            "pick_folder",
            "browser_open",
            "device_action",
            "native_anything",
            "login_vault_list",
            "capture_prompt_window",
            "window_ready",
            "register_browser_client",
            "phone_look_set",
        ] {
            assert!(runs_here(c), "{c}");
        }
        for c in [
            "load_state",
            "send_prompt",
            "transcript_action",
            "import_session",
        ] {
            assert!(!runs_here(c), "{c}");
        }
        assert!(event_is_local("remote-changed") && !event_is_local("session-updated"));
    }

    #[test]
    fn one_computer_per_address_with_distinct_ids() {
        let mut list = Vec::new();
        let a = upsert(
            &mut list,
            "vps",
            &Address::parse("vps.example.com").unwrap(),
        );
        let again = upsert(
            &mut list,
            "vps2",
            &Address::parse("https://vps.example.com").unwrap(),
        );
        assert_eq!(
            (a.id.as_str(), list.len(), again.name.as_str()),
            ("pc-vps-example-com", 1, "vps2")
        );
        let other_port = upsert(
            &mut list,
            "x",
            &Address::parse("vps.example.com:8443").unwrap(),
        );
        assert_ne!(other_port.id, a.id);
        assert_eq!(list.len(), 2);
    }

    #[test]
    fn codes_and_tokens() {
        assert_eq!(normalize_code("482 913").as_deref(), Some("482913"));
        assert_eq!(normalize_code("482-913").as_deref(), Some("482913"));
        assert_eq!(normalize_code("48291"), None);
        assert_eq!(normalize_code("12345a"), None);
        let memory = Memory::default();
        memory.set("pc", "secret").unwrap();
        assert_eq!(memory.get("pc").as_deref(), Some("secret"));
        memory.remove("pc");
        assert_eq!(memory.get("pc"), None);
    }
}
