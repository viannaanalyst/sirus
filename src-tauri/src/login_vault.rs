//! Browser login vault (ADR-186). Saved sign-ins (site origin, username, password and an
//! optional TOTP seed) live in the macOS Keychain as one item; nothing is written to the app
//! data folder or `state.db`. The renderer only ever receives a `LoginSummary` (the password
//! is a fixed mask), and an agent never receives a value at all: `browser_fill_login` hands
//! the credentials from here straight to the page. This module is the pure core (origin
//! matching, storage behind a trait, TOTP, save offers and per-session approvals); the page
//! scripts and the webview calls are in `browser_login.rs`.

use std::collections::HashSet;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::runtime::{AppHandle, Emitter};
use hmac::{Hmac, Mac};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use sha1::Sha1;

use crate::error::{Error, Result as IpcResult};

pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(120);
const OFFER_LIFETIME: Duration = Duration::from_secs(10 * 60);
const MAX_OFFERS: usize = 4;
const MAX_APPROVALS_PER_SESSION: usize = 2;
const MAX_LOGINS: usize = 500;
const MAX_USERNAME: usize = 256;
const MAX_PASSWORD: usize = 1024;
pub const PASSWORD_MASK: &str = "••••••••";

// ---- Origins ----

/// `scheme://host[:port]` of a web address, lower case with the default port dropped, or
/// `None` when it is not a page that may hold a saved login: only `https`, and `http` for
/// loopback development servers; no user info, no IP literal in brackets, ASCII hosts only
/// (the browser hands over punycode, so a look-alike never matches a different site).
pub fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.trim().split_once("://")?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next()?;
    if authority.is_empty() || authority.contains('@') || authority.starts_with('[') {
        return None;
    }
    let (host, port) = match authority.split_once(':') {
        Some((host, port)) => (host, Some(port.parse::<u16>().ok()?)),
        None => (authority, None),
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty()
        || host.len() > 253
        || host.starts_with(['.', '-'])
        || !host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        || host.contains("..")
    {
        return None;
    }
    let loopback = host == "localhost" || host.ends_with(".localhost") || host == "127.0.0.1";
    if scheme == "http" && !loopback {
        return None;
    }
    let default = if scheme == "https" { 443 } else { 80 };
    Some(match port.filter(|port| *port != default) {
        Some(port) => format!("{scheme}://{host}:{port}"),
        None => format!("{scheme}://{host}"),
    })
}

/// The origin a person typed in Settings: a bare host means `https`.
pub fn origin_from_input(input: &str) -> Option<String> {
    let input = input.trim();
    if input.contains("://") {
        origin_of(input)
    } else {
        origin_of(&format!("https://{input}"))
    }
}

// ---- TOTP (RFC 6238: SHA-1, 30 seconds, 6 digits) ----

fn base32_decode(text: &str) -> Option<Vec<u8>> {
    let mut bits = 0u32;
    let mut count = 0u32;
    let mut bytes = Vec::new();
    for c in text.chars() {
        let value = match c {
            'A'..='Z' => c as u32 - 'A' as u32,
            '2'..='7' => c as u32 - '2' as u32 + 26,
            _ => return None,
        };
        bits = (bits << 5) | value;
        count += 5;
        if count >= 8 {
            count -= 8;
            bytes.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Some(bytes)
}

/// A seed as typed or pasted (spaces, lower case, or a whole `otpauth://` link), cleaned up;
/// `None` when it is not a Base32 secret of at least 80 bits.
pub fn normalize_totp_seed(input: &str) -> Option<String> {
    let input = input.trim();
    let raw = if input.to_ascii_lowercase().starts_with("otpauth://") {
        let query = input.split_once('?')?.1;
        query
            .split('&')
            .find_map(|pair| pair.strip_prefix("secret="))?
    } else {
        input
    };
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-' && *c != '=')
        .map(|c| c.to_ascii_uppercase())
        .collect();
    (base32_decode(&cleaned)?.len() >= 10).then_some(cleaned)
}

pub fn totp_code(seed: &str, unix_seconds: u64) -> Option<String> {
    let key = base32_decode(seed)?;
    let mut mac = Hmac::<Sha1>::new_from_slice(&key).ok()?;
    mac.update(&(unix_seconds / 30).to_be_bytes());
    let digest = mac.finalize().into_bytes();
    let offset = (digest[19] & 0x0f) as usize;
    let number = u32::from_be_bytes([
        digest[offset] & 0x7f,
        digest[offset + 1],
        digest[offset + 2],
        digest[offset + 3],
    ]);
    Some(format!("{:06}", number % 1_000_000))
}

// ---- Storage ----

/// Where the vault's single JSON document lives. The Keychain in the app, a double in tests.
pub trait SecretStore: Send + Sync {
    fn read(&self) -> Result<Option<String>, String>;
    fn write(&self, value: &str) -> Result<(), String>;
}

#[cfg(target_os = "macos")]
pub struct KeychainStore;

#[cfg(target_os = "macos")]
impl SecretStore for KeychainStore {
    fn read(&self) -> Result<Option<String>, String> {
        use security_framework::passwords::get_generic_password;
        let found =
            get_generic_password(&crate::keychain_scope::service("browser logins"), "vault");
        let found = match found {
            Err(error) if error.code() == -25300 => {
                crate::keychain_scope::adopt_shared("browser logins", "vault").ok_or(error)
            }
            other => other,
        };
        match found {
            Ok(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| "The saved logins could not be read.".to_string()),
            // errSecItemNotFound
            Err(error) if error.code() == -25300 => Ok(None),
            Err(_) => Err("The Keychain could not be opened.".into()),
        }
    }

    fn write(&self, value: &str) -> Result<(), String> {
        use security_framework::passwords::set_generic_password;
        set_generic_password(
            &crate::keychain_scope::service("browser logins"),
            "vault",
            value.as_bytes(),
        )
        .map_err(|_| "The Keychain refused to save the logins.".to_string())
    }
}

#[cfg(not(target_os = "macos"))]
pub struct KeychainStore;

#[cfg(not(target_os = "macos"))]
impl SecretStore for KeychainStore {
    fn read(&self) -> Result<Option<String>, String> {
        Err("Saved logins need the macOS Keychain.".into())
    }
    fn write(&self, _value: &str) -> Result<(), String> {
        Err("Saved logins need the macOS Keychain.".into())
    }
}

// ---- Logins ----

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Login {
    id: String,
    origin: String,
    username: String,
    password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    totp: Option<String>,
    #[serde(default)]
    agent_use: bool,
    created_at: i64,
    updated_at: i64,
}

impl Drop for Login {
    fn drop(&mut self) {
        let mut bytes = std::mem::take(&mut self.password).into_bytes();
        bytes.iter_mut().for_each(|byte| *byte = 0);
        std::hint::black_box(&bytes);
    }
}

/// What the renderer sees of a login: never the password, never the TOTP seed.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginSummary {
    pub id: String,
    pub origin: String,
    pub username: String,
    pub password_mask: &'static str,
    pub has_totp: bool,
    pub agent_use: bool,
    pub updated_at: i64,
}

impl From<&Login> for LoginSummary {
    fn from(login: &Login) -> Self {
        Self {
            id: login.id.clone(),
            origin: login.origin.clone(),
            username: login.username.clone(),
            password_mask: PASSWORD_MASK,
            has_totp: login.totp.is_some(),
            agent_use: login.agent_use,
            updated_at: login.updated_at,
        }
    }
}

/// Settings form. On an edit an empty or missing `password` keeps the saved one, and an
/// empty `totp` clears the seed while a missing one keeps it.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginInput {
    pub id: Option<String>,
    pub origin: String,
    pub username: String,
    pub password: Option<String>,
    pub totp: Option<String>,
    #[serde(default)]
    pub agent_use: bool,
}

/// Credentials on their way into a page. Not `Serialize`, and `Debug` hides everything.
pub struct Credentials {
    pub username: String,
    pub password: String,
    pub totp: Option<String>,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Credentials(..)")
    }
}

impl Drop for Credentials {
    fn drop(&mut self) {
        for text in [&mut self.username, &mut self.password] {
            let mut bytes = std::mem::take(text).into_bytes();
            bytes.iter_mut().for_each(|byte| *byte = 0);
            std::hint::black_box(&bytes);
        }
    }
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn clean_text(value: &str, limit: usize) -> Option<String> {
    let value = value.trim();
    (!value.is_empty() && value.chars().count() <= limit && !value.chars().any(char::is_control))
        .then(|| value.to_string())
}

pub struct LoginVault {
    store: Box<dyn SecretStore>,
    cache: Option<Vec<Login>>,
}

impl LoginVault {
    pub fn new(store: Box<dyn SecretStore>) -> Self {
        Self { store, cache: None }
    }

    fn loaded(&mut self) -> Result<&Vec<Login>, String> {
        if self.cache.is_none() {
            let logins = match self.store.read()? {
                Some(text) => serde_json::from_str(&text)
                    .map_err(|_| "The saved logins could not be read.".to_string())?,
                None => Vec::new(),
            };
            self.cache = Some(logins);
        }
        Ok(self.cache.as_ref().expect("loaded above"))
    }

    /// Applies a change to a copy, saves it, and only then adopts it.
    fn mutate<T>(
        &mut self,
        change: impl FnOnce(&mut Vec<Login>) -> Result<T, String>,
    ) -> Result<T, String> {
        let mut next = self.loaded()?.clone();
        let out = change(&mut next)?;
        let text = serde_json::to_string(&next)
            .map_err(|_| "The logins could not be saved.".to_string())?;
        self.store.write(&text)?;
        self.cache = Some(next);
        Ok(out)
    }

    pub fn list(&mut self) -> Result<Vec<LoginSummary>, String> {
        let mut all: Vec<LoginSummary> = self.loaded()?.iter().map(LoginSummary::from).collect();
        all.sort_by(|a, b| {
            (a.origin.as_str(), a.username.to_lowercase())
                .cmp(&(b.origin.as_str(), b.username.to_lowercase()))
        });
        Ok(all)
    }

    /// Logins whose origin is exactly the page's, for the person's key icon.
    pub fn matching(&mut self, url: &str) -> Result<Vec<LoginSummary>, String> {
        let Some(origin) = origin_of(url) else {
            return Ok(Vec::new());
        };
        Ok(self
            .list()?
            .into_iter()
            .filter(|login| login.origin == origin)
            .collect())
    }

    pub fn upsert(&mut self, input: LoginInput) -> Result<LoginSummary, String> {
        let origin = origin_from_input(&input.origin)
            .ok_or("Enter a site such as https://example.com (https only; http for localhost).")?;
        let username = clean_text(&input.username, MAX_USERNAME)
            .ok_or("Enter a username of up to 256 characters.")?;
        let password = match input.password.as_deref() {
            Some("") | None => None,
            Some(text) if text.chars().count() <= MAX_PASSWORD && !text.contains('\0') => {
                Some(text.to_string())
            }
            Some(_) => return Err("The password is too long.".into()),
        };
        let totp = match input.totp.as_deref().map(str::trim) {
            None => None,
            Some("") => Some(None),
            Some(seed) => Some(Some(
                normalize_totp_seed(seed).ok_or("That is not a valid authenticator secret.")?,
            )),
        };
        self.mutate(|logins| {
            let same = |login: &Login| {
                login.origin == origin && login.username.eq_ignore_ascii_case(&username)
            };
            let position = match &input.id {
                Some(id) => Some(
                    logins
                        .iter()
                        .position(|login| &login.id == id)
                        .ok_or("That login no longer exists.")?,
                ),
                None => None,
            };
            if let Some(clash) = logins.iter().position(same) {
                if Some(clash) != position {
                    return Err("A login for that site and username already exists.".into());
                }
            }
            match position {
                Some(at) => {
                    let login = &mut logins[at];
                    login.origin = origin;
                    login.username = username;
                    if let Some(password) = password {
                        login.password = password;
                    }
                    if let Some(totp) = totp {
                        login.totp = totp;
                    }
                    login.agent_use = input.agent_use;
                    login.updated_at = now();
                    Ok(LoginSummary::from(&*login))
                }
                None => {
                    let password = password.ok_or("Enter the password.")?;
                    if logins.len() >= MAX_LOGINS {
                        return Err("The vault is full.".into());
                    }
                    let login = Login {
                        id: uuid::Uuid::new_v4().to_string(),
                        origin,
                        username,
                        password,
                        totp: totp.flatten(),
                        agent_use: input.agent_use,
                        created_at: now(),
                        updated_at: now(),
                    };
                    let summary = LoginSummary::from(&login);
                    logins.push(login);
                    Ok(summary)
                }
            }
        })
    }

    pub fn delete(&mut self, id: &str) -> Result<(), String> {
        self.mutate(|logins| {
            logins.retain(|login| login.id != id);
            Ok(())
        })
    }

    pub fn set_agent_use(&mut self, id: &str, enabled: bool) -> Result<LoginSummary, String> {
        self.mutate(|logins| {
            let login = logins
                .iter_mut()
                .find(|login| login.id == id)
                .ok_or("That login no longer exists.")?;
            login.agent_use = enabled;
            login.updated_at = now();
            Ok(LoginSummary::from(&*login))
        })
    }

    /// The login an agent may use on `page_url`: exact origin, marked usable by agents.
    /// With several candidates the agent must name the username. The reason in `Err` is safe
    /// to show the agent.
    pub fn for_agent(
        &mut self,
        page_url: &str,
        username: Option<&str>,
    ) -> Result<(LoginSummary, Credentials), String> {
        let origin = origin_of(page_url)
            .ok_or("This page cannot have a saved login (only https pages and localhost).")?;
        let candidates: Vec<&Login> = self
            .loaded()?
            .iter()
            .filter(|login| login.origin == origin && login.agent_use)
            .collect();
        let chosen = match (candidates.as_slice(), username) {
            ([], _) => {
                return Err(format!(
                    "No saved login for {origin} is marked as usable by agents. Ask the person to save one in Settings > Browser > Saved logins, or use request_secret."
                ))
            }
            ([only], None) => *only,
            (many, None) => {
                let names: Vec<&str> = many.iter().map(|login| login.username.as_str()).collect();
                return Err(format!(
                    "Several logins are saved for {origin}; call again with `username` set to one of: {}.",
                    names.join(", ")
                ));
            }
            (many, Some(name)) => many
                .iter()
                .copied()
                .find(|login| login.username.eq_ignore_ascii_case(name.trim()))
                .ok_or_else(|| format!("No agent-usable login for {origin} has that username."))?,
        };
        Ok((LoginSummary::from(chosen), credentials_of(chosen)))
    }

    /// The person's own autofill: any login of exactly this page's origin.
    pub fn for_person(
        &mut self,
        page_url: &str,
        id: &str,
    ) -> Result<(LoginSummary, Credentials), String> {
        let origin = origin_of(page_url).ok_or("This page cannot have a saved login.")?;
        let login = self
            .loaded()?
            .iter()
            .find(|login| login.id == id)
            .ok_or("That login no longer exists.")?;
        if login.origin != origin {
            return Err("That login belongs to a different site.".into());
        }
        Ok((LoginSummary::from(login), credentials_of(login)))
    }

    fn classify(&mut self, origin: &str, username: &str, password: &str) -> Result<Offer, String> {
        Ok(
            match self.loaded()?.iter().find(|login| {
                login.origin == origin && login.username.eq_ignore_ascii_case(username)
            }) {
                Some(login) if login.password == password => Offer::Skip,
                Some(login) => Offer::Update(login.id.clone()),
                None => Offer::New,
            },
        )
    }
}

fn credentials_of(login: &Login) -> Credentials {
    Credentials {
        username: login.username.clone(),
        password: login.password.clone(),
        totp: login.totp.clone(),
    }
}

/// What to do with a sign-in the person just submitted.
#[derive(Debug, PartialEq)]
enum Offer {
    /// Already saved exactly like this.
    Skip,
    New,
    /// Same site and username with a different password: offer to update this login.
    Update(String),
}

// ---- Global vault ----

static VAULT: Mutex<Option<LoginVault>> = Mutex::new(None);

fn with_vault<T>(f: impl FnOnce(&mut LoginVault) -> T) -> T {
    f(VAULT
        .lock()
        .get_or_insert_with(|| LoginVault::new(Box::new(KeychainStore))))
}

/// For the browser tools: the agent's credentials for the page it is on.
pub fn agent_credentials(
    page_url: &str,
    username: Option<&str>,
) -> Result<(LoginSummary, Credentials), String> {
    with_vault(|vault| vault.for_agent(page_url, username))
}

pub fn person_credentials(page_url: &str, id: &str) -> Result<(LoginSummary, Credentials), String> {
    with_vault(|vault| vault.for_person(page_url, id))
}

// ---- Save offers and agent approvals ----

struct PendingOffer {
    info: LoginOffer,
    password: Credentials,
    created: Instant,
}

struct PendingApproval {
    info: LoginApproval,
    reply: mpsc::Sender<bool>,
}

#[derive(Default)]
struct Prompts {
    offers: Vec<PendingOffer>,
    approvals: Vec<PendingApproval>,
    /// (session, origin) pairs the person already allowed this app run.
    allowed: HashSet<(String, String)>,
}

static PROMPTS: Mutex<Option<Prompts>> = Mutex::new(None);

fn with_prompts<T>(f: impl FnOnce(&mut Prompts) -> T) -> T {
    f(PROMPTS.lock().get_or_insert_with(Prompts::default))
}

/// "Save login for this site?" in the browser pane. No password.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginOffer {
    pub id: String,
    pub session_id: String,
    pub origin: String,
    pub username: String,
    /// The site and username are saved already with another password.
    pub update: bool,
}

/// "Let the agent sign in to this site?" in the conversation.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginApproval {
    pub id: String,
    pub session_id: String,
    pub origin: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginPrompts {
    pub offers: Vec<LoginOffer>,
    pub approvals: Vec<LoginApproval>,
}

pub fn prompts() -> LoginPrompts {
    with_prompts(|prompts| {
        prompts
            .offers
            .retain(|offer| offer.created.elapsed() < OFFER_LIFETIME);
        LoginPrompts {
            offers: prompts.offers.iter().map(|o| o.info.clone()).collect(),
            approvals: prompts.approvals.iter().map(|a| a.info.clone()).collect(),
        }
    })
}

fn publish(app: &AppHandle) {
    let _ = app.emit("login-vault-state", prompts());
}

/// The sign-in a page reported when the person submitted a password form. `frame_url` is
/// the address the webview loaded (never taken from the page), `raw` the page script's
/// JSON `{ "u": username, "p": password }`.
pub fn parse_submission(frame_url: &str, raw: &str) -> Option<(String, String, String)> {
    let origin = origin_of(frame_url)?;
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let username = value
        .get("u")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();
    let password = value.get("p").and_then(|v| v.as_str())?.to_string();
    if password.is_empty()
        || password.chars().count() > MAX_PASSWORD
        || password.contains('\0')
        || username.chars().count() > MAX_USERNAME
        || username.chars().any(char::is_control)
    {
        return None;
    }
    Some((origin, username, password))
}

/// Called when a page reports a submitted sign-in: queues "Save login?" unless it is
/// already saved identically or the vault is unavailable. A sign-in with no username field
/// is not offered (the saved login could not be matched or edited sensibly).
pub fn note_submission(app: &AppHandle, session_id: &str, frame_url: &str, raw: &str) {
    let Some((origin, username, password)) = parse_submission(frame_url, raw) else {
        return;
    };
    if username.is_empty() {
        return;
    }
    let Ok(kind) = with_vault(|vault| vault.classify(&origin, &username, &password)) else {
        return;
    };
    let update = match kind {
        Offer::Skip => return,
        Offer::New => false,
        Offer::Update(_) => true,
    };
    with_prompts(|prompts| {
        // The same site and username replaces an earlier, unanswered offer.
        prompts.offers.retain(|offer| {
            !(offer.info.session_id == session_id
                && offer.info.origin == origin
                && offer.info.username == username)
        });
        prompts.offers.push(PendingOffer {
            info: LoginOffer {
                id: uuid::Uuid::new_v4().to_string(),
                session_id: session_id.into(),
                origin: origin.clone(),
                username: username.clone(),
                update,
            },
            password: Credentials {
                username: String::new(),
                password,
                totp: None,
            },
            created: Instant::now(),
        });
        while prompts.offers.len() > MAX_OFFERS {
            prompts.offers.remove(0);
        }
    });
    publish(app);
}

/// The person answered a save offer. `agent_use` also lets agents use the saved login.
pub fn answer_offer(app: &AppHandle, id: &str, save: bool, agent_use: bool) -> Result<(), String> {
    let offer = with_prompts(|prompts| {
        let at = prompts
            .offers
            .iter()
            .position(|offer| offer.info.id == id)?;
        Some(prompts.offers.remove(at))
    });
    publish(app);
    let Some(offer) = offer else {
        return Err("That offer expired.".into());
    };
    if !save {
        return Ok(());
    }
    with_vault(|vault| {
        let existing = vault
            .loaded()?
            .iter()
            .find(|login| {
                login.origin == offer.info.origin
                    && login.username.eq_ignore_ascii_case(&offer.info.username)
            })
            .map(|login| (login.id.clone(), login.agent_use));
        vault
            .upsert(LoginInput {
                id: existing.as_ref().map(|(id, _)| id.clone()),
                origin: offer.info.origin.clone(),
                username: offer.info.username.clone(),
                password: Some(offer.password.password.clone()),
                totp: None,
                agent_use: agent_use || existing.is_some_and(|(_, usable)| usable),
            })
            .map(|_| ())
    })
}

/// Blocks until the person allows or refuses an agent signing in to `origin`; asked once
/// per session and site, then remembered until the app quits (or the login changes).
pub fn require_approval(
    app: &AppHandle,
    session_id: &str,
    origin: &str,
    username: &str,
) -> Result<(), String> {
    let key = (session_id.to_string(), origin.to_string());
    if with_prompts(|prompts| prompts.allowed.contains(&key)) {
        return Ok(());
    }
    let (reply, wait) = mpsc::channel();
    let id = uuid::Uuid::new_v4().to_string();
    let admitted = with_prompts(|prompts| {
        let open = prompts
            .approvals
            .iter()
            .filter(|approval| approval.info.session_id == session_id)
            .count();
        if open >= MAX_APPROVALS_PER_SESSION {
            return false;
        }
        prompts.approvals.push(PendingApproval {
            info: LoginApproval {
                id: id.clone(),
                session_id: session_id.into(),
                origin: origin.into(),
                username: username.into(),
            },
            reply,
        });
        true
    });
    if !admitted {
        return Err("Another sign-in approval is already waiting for the person.".into());
    }
    publish(app);
    let allowed = wait.recv_timeout(APPROVAL_TIMEOUT).unwrap_or(false);
    with_prompts(|prompts| {
        prompts.approvals.retain(|approval| approval.info.id != id);
        if allowed {
            prompts.allowed.insert(key);
        }
    });
    publish(app);
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "The person did not allow signing in to {origin}. Do not try again; ask them to sign in themselves or explain what is needed."
        ))
    }
}

pub fn answer_approval(app: &AppHandle, id: &str, allow: bool) -> Result<(), String> {
    let reply = with_prompts(|prompts| {
        let at = prompts
            .approvals
            .iter()
            .position(|approval| approval.info.id == id)?;
        Some(prompts.approvals.remove(at).reply)
    })
    .ok_or("That request is no longer waiting.")?;
    publish(app);
    reply
        .send(allow)
        .map_err(|_| "That request is no longer waiting.".to_string())
}

/// A turn ended: its waiting approvals are declined. Allowances last until the app quits.
pub fn end_turn(session_id: &str) {
    let gone = with_prompts(|prompts| {
        let (gone, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut prompts.approvals)
            .into_iter()
            .partition(|approval| approval.info.session_id == session_id);
        prompts.approvals = kept;
        gone
    });
    for approval in gone {
        let _ = approval.reply.send(false);
    }
}

/// App quit.
pub fn clear_all() {
    let taken = with_prompts(std::mem::take);
    for approval in taken.approvals {
        let _ = approval.reply.send(false);
    }
}

/// The current UNIX time, for a TOTP code.
pub fn unix_now() -> u64 {
    chrono::Utc::now().timestamp().max(0) as u64
}

// ---- IPC ----

fn ipc(error: String) -> Error {
    Error::new("login-vault", error)
}

pub async fn login_vault_list() -> IpcResult<Vec<LoginSummary>> {
    with_vault(|vault| vault.list()).map_err(ipc)
}

pub async fn login_vault_save(input: LoginInput) -> IpcResult<LoginSummary> {
    with_vault(|vault| vault.upsert(input)).map_err(ipc)
}

pub async fn login_vault_delete(id: String) -> IpcResult<()> {
    with_vault(|vault| vault.delete(&id)).map_err(ipc)
}

pub async fn login_vault_set_agent_use(id: String, enabled: bool) -> IpcResult<LoginSummary> {
    with_vault(|vault| vault.set_agent_use(&id, enabled)).map_err(ipc)
}

/// Logins that exactly match a page's origin (the key icon in the address bar).
pub async fn login_vault_matches(url: String) -> IpcResult<Vec<LoginSummary>> {
    with_vault(|vault| vault.matching(&url)).map_err(ipc)
}

pub async fn login_vault_prompts() -> IpcResult<LoginPrompts> {
    Ok(prompts())
}

pub async fn login_vault_answer_offer(
    app: AppHandle,
    id: String,
    save: bool,
    agent_use: bool,
) -> IpcResult<()> {
    answer_offer(&app, &id, save, agent_use).map_err(ipc)
}

pub async fn login_vault_answer_approval(app: AppHandle, id: String, allow: bool) -> IpcResult<()> {
    answer_approval(&app, &id, allow).map_err(ipc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// A Keychain double: the document in memory, optionally failing writes.
    #[derive(Default, Clone)]
    struct MemoryStore {
        text: Arc<Mutex<Option<String>>>,
        fail_writes: Arc<Mutex<bool>>,
    }

    impl SecretStore for MemoryStore {
        fn read(&self) -> Result<Option<String>, String> {
            Ok(self.text.lock().clone())
        }
        fn write(&self, value: &str) -> Result<(), String> {
            if *self.fail_writes.lock() {
                return Err("denied".into());
            }
            *self.text.lock() = Some(value.to_string());
            Ok(())
        }
    }

    fn input(origin: &str, username: &str, password: &str) -> LoginInput {
        LoginInput {
            id: None,
            origin: origin.into(),
            username: username.into(),
            password: Some(password.into()),
            totp: None,
            agent_use: true,
        }
    }

    fn vault() -> (LoginVault, MemoryStore) {
        let store = MemoryStore::default();
        (LoginVault::new(Box::new(store.clone())), store)
    }

    #[test]
    fn origins_match_exactly() {
        assert_eq!(
            origin_of("https://Example.com/login?x=1#y").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("https://example.com:443/").as_deref(),
            Some("https://example.com")
        );
        assert_eq!(
            origin_of("https://example.com:8443").as_deref(),
            Some("https://example.com:8443")
        );
        assert_eq!(
            origin_of("http://localhost:3000/a").as_deref(),
            Some("http://localhost:3000")
        );
        assert_eq!(
            origin_of("http://127.0.0.1:80/").as_deref(),
            Some("http://127.0.0.1")
        );
        // No subdomain guessing: these are all different origins.
        let base = origin_of("https://example.com").unwrap();
        for other in [
            "https://www.example.com",
            "https://login.example.com",
            "https://example.com:444",
            "https://example.co",
            "https://notexample.com",
            "https://example.com.evil.test",
        ] {
            assert_ne!(origin_of(other).unwrap(), base, "{other}");
        }
        // Not eligible at all.
        for bad in [
            "http://example.com",
            "ftp://example.com",
            "about:blank",
            "https://user@example.com",
            "https://example.com@evil.test/",
            "https://[::1]/",
            "https://exämple.com",
            "https://",
            "https://exa mple.com",
            "https://example.com:99999",
            "https://.example.com",
            "not a url",
        ] {
            assert_eq!(origin_of(bad), None, "{bad}");
        }
        assert_eq!(
            origin_from_input("example.com").as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn totp_matches_rfc_6238() {
        // "12345678901234567890" in Base32; the RFC's 8-digit codes end in the 6 digits below.
        let seed = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(totp_code(seed, 59).as_deref(), Some("287082"));
        assert_eq!(totp_code(seed, 1_111_111_109).as_deref(), Some("081804"));
        assert_eq!(totp_code(seed, 20_000_000_000).as_deref(), Some("353130"));
        assert_eq!(
            normalize_totp_seed("gezd gnbv-gy3t qojq gezd gnbv gy3t qojq=").as_deref(),
            Some(seed)
        );
        assert_eq!(
            normalize_totp_seed(&format!("otpauth://totp/a:b?secret={seed}&issuer=a")).as_deref(),
            Some(seed)
        );
        assert_eq!(normalize_totp_seed("short"), None);
        assert_eq!(normalize_totp_seed("not base32 !!"), None);
    }

    #[test]
    fn storage_round_trip_keeps_secrets_out_of_summaries() {
        let (mut vault, store) = vault();
        let saved = vault
            .upsert(input("example.com", "ana@example.com", "hunter2-correct"))
            .unwrap();
        assert_eq!(saved.origin, "https://example.com");
        assert_eq!(saved.password_mask, PASSWORD_MASK);
        let json = serde_json::to_string(&vault.list().unwrap()).unwrap();
        assert!(
            !json.contains("hunter2"),
            "summaries never carry the password"
        );

        // A fresh vault over the same store sees the login (the Keychain round trip).
        let mut reopened = LoginVault::new(Box::new(store.clone()));
        let (summary, credentials) = reopened
            .for_agent("https://example.com/sign-in", None)
            .unwrap();
        assert_eq!(summary.username, "ana@example.com");
        assert_eq!(credentials.password, "hunter2-correct");
        assert_eq!(format!("{credentials:?}"), "Credentials(..)");

        // Edit: an empty password keeps the saved one, a TOTP seed is kept until cleared.
        let seed = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        let mut edit = input("https://example.com", "ana@example.com", "");
        edit.id = Some(saved.id.clone());
        edit.totp = Some(seed.into());
        edit.agent_use = false;
        let edited = vault.upsert(edit).unwrap();
        assert!(edited.has_totp && !edited.agent_use);
        assert!(vault
            .for_agent("https://example.com", None)
            .unwrap_err()
            .contains("No saved login"));
        assert_eq!(
            vault
                .for_person("https://example.com", &saved.id)
                .unwrap()
                .1
                .password,
            "hunter2-correct"
        );

        vault.delete(&saved.id).unwrap();
        assert!(LoginVault::new(Box::new(store)).list().unwrap().is_empty());
    }

    #[test]
    fn a_failed_write_changes_nothing() {
        let (mut vault, store) = vault();
        vault.upsert(input("example.com", "a", "pw-one")).unwrap();
        *store.fail_writes.lock() = true;
        assert!(vault.upsert(input("example.org", "b", "pw-two")).is_err());
        assert_eq!(vault.list().unwrap().len(), 1);
    }

    #[test]
    fn duplicates_and_bad_input_are_refused() {
        let (mut vault, _) = vault();
        vault.upsert(input("example.com", "Ana", "pw")).unwrap();
        assert!(vault
            .upsert(input("https://example.com", "ana", "pw2"))
            .is_err());
        assert!(vault
            .upsert(input("http://example.com", "x", "pw"))
            .is_err());
        assert!(vault.upsert(input("example.com", "", "pw")).is_err());
        let mut no_password = input("example.org", "x", "");
        no_password.password = None;
        assert!(vault.upsert(no_password).is_err());
        let mut bad_seed = input("example.org", "x", "pw");
        bad_seed.totp = Some("nope".into());
        assert!(vault.upsert(bad_seed).is_err());
    }

    #[test]
    fn agents_get_only_exact_origin_logins_marked_usable() {
        let (mut vault, _) = vault();
        vault.upsert(input("example.com", "ana", "pw-ana")).unwrap();
        let mut private = input("example.com", "bob", "pw-bob");
        private.agent_use = false;
        vault.upsert(private).unwrap();
        vault
            .upsert(input("www.example.com", "carol", "pw-carol"))
            .unwrap();

        // Only ana is usable on example.com; bob is not shared and carol is another origin.
        let (summary, credentials) = vault.for_agent("https://example.com/", None).unwrap();
        assert_eq!(
            (summary.username.as_str(), credentials.password.as_str()),
            ("ana", "pw-ana")
        );
        let error = vault
            .for_agent("https://example.com", Some("bob"))
            .unwrap_err();
        assert!(!error.contains("pw-"));
        assert!(vault.for_agent("https://app.example.com", None).is_err());
        assert!(vault.for_agent("http://example.com", None).is_err());
        assert!(vault.for_agent("about:blank", None).is_err());

        // Two usable logins: the agent has to say which, and the error names only usernames.
        vault.upsert(input("example.com", "dan", "pw-dan")).unwrap();
        let error = vault.for_agent("https://example.com", None).unwrap_err();
        assert!(error.contains("ana") && error.contains("dan") && !error.contains("pw-"));
        assert_eq!(
            vault
                .for_agent("https://example.com", Some("DAN"))
                .unwrap()
                .0
                .username,
            "dan"
        );

        // The person's key icon lists all exact matches, usable by agents or not.
        let shown: Vec<_> = vault
            .matching("https://example.com/x")
            .unwrap()
            .into_iter()
            .map(|login| login.username)
            .collect();
        assert_eq!(shown, ["ana", "bob", "dan"]);
        // ... and refuses another site's login by id.
        let carol = vault.matching("https://www.example.com").unwrap().remove(0);
        assert!(vault.for_person("https://example.com", &carol.id).is_err());
    }

    #[test]
    fn submissions_become_offers_only_when_new_or_changed() {
        let (mut vault, _) = vault();
        vault.upsert(input("example.com", "ana", "old-pw")).unwrap();
        let classify = |vault: &mut LoginVault, user: &str, pw: &str| {
            vault.classify("https://example.com", user, pw).unwrap()
        };
        assert_eq!(classify(&mut vault, "ana", "old-pw"), Offer::Skip);
        assert_eq!(classify(&mut vault, "ANA", "old-pw"), Offer::Skip);
        assert!(matches!(
            classify(&mut vault, "ana", "new-pw"),
            Offer::Update(_)
        ));
        assert_eq!(classify(&mut vault, "bob", "pw"), Offer::New);
    }

    #[test]
    fn page_reports_are_parsed_against_the_frame_address() {
        let ok = parse_submission("https://Example.com/login", r#"{"u":" ana ","p":"s3cret"}"#);
        assert_eq!(
            ok,
            Some(("https://example.com".into(), "ana".into(), "s3cret".into()))
        );
        // The origin comes from the webview's address, never from the report.
        let spoof = parse_submission(
            "https://evil.test/",
            r#"{"u":"a","p":"x","origin":"https://bank.test"}"#,
        );
        assert_eq!(spoof.unwrap().0, "https://evil.test");
        for (url, raw) in [
            ("http://example.com/", r#"{"u":"a","p":"x"}"#),
            ("https://example.com/", r#"{"u":"a","p":""}"#),
            ("https://example.com/", r#"{"u":"a"}"#),
            ("https://example.com/", "not json"),
            ("https://example.com/", r#"{"u":"a\u0007","p":"x"}"#),
        ] {
            assert_eq!(parse_submission(url, raw), None, "{url} {raw}");
        }
        let long = format!(r#"{{"u":"a","p":"{}"}}"#, "x".repeat(MAX_PASSWORD + 1));
        assert_eq!(parse_submission("https://example.com/", &long), None);
    }

    #[test]
    fn prompts_never_carry_passwords() {
        let offer = LoginOffer {
            id: "1".into(),
            session_id: "s".into(),
            origin: "https://example.com".into(),
            username: "ana".into(),
            update: false,
        };
        let json = serde_json::to_string(&LoginPrompts {
            offers: vec![offer],
            approvals: vec![],
        })
        .unwrap();
        assert!(!json.contains("assword"));
    }
}
