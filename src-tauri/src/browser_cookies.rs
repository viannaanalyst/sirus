//! Importing cookies from Chrome and Safari into a browser profile (ADR-152).
//!
//! This module finds the sources, reads them and decodes the cookies the owner selected. It never
//! touches WebKit or the network. Values live in memory only: Chrome's key, the Keychain password
//! and the decrypted values are dropped when the import returns. The one thing written to disk is
//! a temporary copy of Chrome's cookie database (Chrome locks the original), deleted after the read.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

use aes::cipher::{BlockDecrypt, KeyInit};
use hmac::{Hmac, Mac};
use serde::Serialize;
use sha1::Sha1;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// Domains one import may carry; the owner ticks them from a list, so this is generous.
pub const MAX_DOMAINS: usize = 500;
const CHROME_ROOT: &str = "Library/Application Support/Google/Chrome";
const SAFARI_COOKIES: &str =
    "Library/Containers/com.apple.Safari/Data/Library/Cookies/Cookies.binarycookies";
/// Seconds between 1970 and the Core Foundation epoch (2001), used by Safari.
const MAC_EPOCH_UNIX: f64 = 978_307_200.0;
/// Seconds between 1601 (Chrome's `expires_utc` epoch, in microseconds) and 1970.
const CHROME_EPOCH_UNIX: f64 = 11_644_473_600.0;
const CHROME_KEY_LEN: usize = 16;
/// The Keychain item Chrome keeps its cookie key under on macOS.
const CHROME_SAFE_STORAGE: &str = "Chrome Safe Storage";

pub type ChromeKey = [u8; CHROME_KEY_LEN];

/// A browser the owner can import from. `id` is what the renderer sends back: `safari`, or
/// `chrome:<folder>` where the folder is `Default` or `Profile N`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieSource {
    pub id: String,
    pub browser: String,
    /// Chrome's display name for the profile, or the folder name when Chrome has none.
    pub profile: String,
}

/// A domain that has cookies, with how many. Names and values are never listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieDomain {
    pub domain: String,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CookieImportReport {
    pub imported: u32,
    pub skipped: u32,
}

/// Where a source lives, resolved from its id against the real folders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    Chrome(PathBuf),
    Safari,
}

/// A cookie as the browser stores it. No `Debug`: it holds values.
#[derive(Clone, PartialEq)]
pub struct RawCookie {
    pub host: String,
    pub name: String,
    pub path: String,
    pub value: StoredValue,
    /// Unix seconds; `None` for a session cookie.
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
}

#[derive(Clone, PartialEq)]
pub enum StoredValue {
    Plain(Vec<u8>),
    /// Chrome's blob with its version prefix (`v10`, `v11`, ...).
    Encrypted(Vec<u8>),
}

/// A cookie ready for WebKit. No `Debug`: it holds the value.
#[derive(Clone, PartialEq)]
pub struct DecodedCookie {
    pub host: String,
    pub name: String,
    pub path: String,
    pub value: String,
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
}

pub fn now_unix() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or_default()
}

/// The host part a cookie belongs to, as the owner sees it: lowercase, no leading dot.
pub fn domain_key(host: &str) -> String {
    host.trim_start_matches('.').to_ascii_lowercase()
}

fn expired(cookie: &RawCookie, now: f64) -> bool {
    cookie.expires.is_some_and(|expires| expires <= now)
}

/// The domain list for the owner to tick: one row per domain with its unexpired cookie count.
pub fn domain_summary(cookies: &[RawCookie], now: f64) -> Vec<CookieDomain> {
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    for cookie in cookies.iter().filter(|cookie| !expired(cookie, now)) {
        let domain = domain_key(&cookie.host);
        if !domain.is_empty() {
            *counts.entry(domain).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .map(|(domain, count)| CookieDomain { domain, count })
        .collect()
}

/// The cookies whose domain the owner picked, nothing else.
pub fn select(cookies: Vec<RawCookie>, domains: &HashSet<String>) -> Vec<RawCookie> {
    cookies
        .into_iter()
        .filter(|cookie| domains.contains(&domain_key(&cookie.host)))
        .collect()
}

/// Checks the renderer's domain list: non-empty, bounded, and plain host names only.
pub fn normalize_domains(domains: &[String]) -> Result<HashSet<String>> {
    if domains.is_empty() {
        return Err(Error::new("invalid", "Pick at least one domain to import."));
    }
    if domains.len() > MAX_DOMAINS {
        return Err(Error::new("invalid", "Pick fewer domains for one import."));
    }
    domains
        .iter()
        .map(|domain| {
            let domain = domain_key(domain.trim());
            let plain = !domain.is_empty()
                && domain.len() <= 253
                && domain
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'.');
            if plain {
                Ok(domain)
            } else {
                Err(Error::new("invalid", "That is not a domain name."))
            }
        })
        .collect()
}

fn is_profile_folder(name: &str) -> bool {
    name == "Default"
        || name.strip_prefix("Profile ").is_some_and(|number| {
            !number.is_empty() && number.bytes().all(|byte| byte.is_ascii_digit())
        })
}

/// Chrome keeps cookies in `Network/Cookies` from Chrome 96; older profiles have `Cookies`.
fn chrome_cookie_file(profile_dir: &Path) -> Option<PathBuf> {
    ["Network/Cookies", "Cookies"]
        .iter()
        .map(|relative| profile_dir.join(relative))
        .find(|path| path.is_file())
}

/// Display names from Chrome's `Local State` file, keyed by profile folder.
fn chrome_display_names(root: &Path) -> BTreeMap<String, String> {
    let mut names = BTreeMap::new();
    let Ok(text) = std::fs::read_to_string(root.join("Local State")) else {
        return names;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return names;
    };
    if let Some(cache) = value
        .pointer("/profile/info_cache")
        .and_then(|cache| cache.as_object())
    {
        for (folder, info) in cache {
            if let Some(name) = info.get("name").and_then(|name| name.as_str()) {
                if !name.trim().is_empty() {
                    names.insert(folder.clone(), name.trim().to_string());
                }
            }
        }
    }
    names
}

/// The Chrome profiles under `root` that have a cookie file, `Default` first.
pub fn chrome_profiles(root: &Path) -> Vec<CookieSource> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let names = chrome_display_names(root);
    let mut folders: Vec<String> = entries
        .flatten()
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|folder| {
            is_profile_folder(folder) && chrome_cookie_file(&root.join(folder)).is_some()
        })
        .collect();
    folders.sort_by_key(|folder| (folder != "Default", folder.len(), folder.clone()));
    folders
        .into_iter()
        .map(|folder| CookieSource {
            profile: names
                .get(&folder)
                .cloned()
                .unwrap_or_else(|| folder.clone()),
            id: format!("chrome:{folder}"),
            browser: "chrome".into(),
        })
        .collect()
}

/// The sources the owner can choose from: every Chrome profile with cookies, then Safari.
pub fn list_sources(home: &Path) -> Vec<CookieSource> {
    let mut sources = chrome_profiles(&home.join(CHROME_ROOT));
    sources.push(CookieSource {
        id: "safari".into(),
        browser: "safari".into(),
        profile: "Safari".into(),
    });
    sources
}

/// Turns a source id into a path, accepting only the folder shapes Chrome uses (no `..`).
pub fn source_from_id(home: &Path, id: &str) -> Result<Source> {
    if id == "safari" {
        return Ok(Source::Safari);
    }
    let folder = id
        .strip_prefix("chrome:")
        .filter(|folder| is_profile_folder(folder))
        .ok_or_else(|| Error::not_found("That browser profile is not available."))?;
    Ok(Source::Chrome(home.join(CHROME_ROOT).join(folder)))
}

/// Reads every cookie of a source, still encrypted where the browser encrypts it.
pub fn read_source(home: &Path, source: &Source) -> Result<Vec<RawCookie>> {
    match source {
        Source::Chrome(profile_dir) => read_chrome(profile_dir),
        Source::Safari => read_safari(home),
    }
}

/// Copies Chrome's database (with its write-ahead log, if any) to a temporary folder and reads
/// the copy read-only. The folder and its copies are deleted when this returns.
fn read_chrome(profile_dir: &Path) -> Result<Vec<RawCookie>> {
    let original = chrome_cookie_file(profile_dir)
        .ok_or_else(|| Error::not_found("Chrome has no cookies for that profile."))?;
    let copy_dir = tempfile::Builder::new()
        .prefix("sirus-cookies-")
        .tempdir()
        .map_err(|_| Error::new("native", "A temporary folder could not be created."))?;
    let copy = copy_dir.path().join("Cookies");
    std::fs::copy(&original, &copy)
        .map_err(|_| Error::new("native", "Chrome's cookies could not be copied."))?;
    let mut wal_name = original.as_os_str().to_owned();
    wal_name.push("-wal");
    let wal = PathBuf::from(wal_name);
    if wal.is_file() {
        let _ = std::fs::copy(&wal, copy_dir.path().join("Cookies-wal"));
    }
    read_chrome_database(&copy)
}

fn read_chrome_database(path: &Path) -> Result<Vec<RawCookie>> {
    use rusqlite::types::ValueRef;
    let unreadable = || Error::new("native", "Chrome's cookies could not be read.");
    let database = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| unreadable())?;
    let mut statement = database
        .prepare(
            "SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly FROM cookies",
        )
        .map_err(|_| unreadable())?;
    let rows = statement
        .query_map([], |row| {
            let bytes = |index: usize| -> rusqlite::Result<Vec<u8>> {
                Ok(match row.get_ref(index)? {
                    ValueRef::Text(text) | ValueRef::Blob(text) => text.to_vec(),
                    _ => Vec::new(),
                })
            };
            let encrypted = bytes(3)?;
            let value = if encrypted.is_empty() {
                StoredValue::Plain(bytes(2)?)
            } else {
                StoredValue::Encrypted(encrypted)
            };
            let expires_utc: i64 = row.get(5)?;
            Ok(RawCookie {
                host: String::from_utf8_lossy(&bytes(0)?).into_owned(),
                name: String::from_utf8_lossy(&bytes(1)?).into_owned(),
                path: String::from_utf8_lossy(&bytes(4)?).into_owned(),
                value,
                expires: (expires_utc > 0)
                    .then(|| expires_utc as f64 / 1_000_000.0 - CHROME_EPOCH_UNIX),
                secure: row.get::<_, i64>(6)? != 0,
                http_only: row.get::<_, i64>(7)? != 0,
            })
        })
        .map_err(|_| unreadable())?;
    rows.collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|_| unreadable())
}

/// Safari's cookie file. Without Full Disk Access macOS refuses the read; the caller shows a
/// message that names the setting, and nothing else is tried.
fn read_safari(home: &Path) -> Result<Vec<RawCookie>> {
    let path = home.join(SAFARI_COOKIES);
    let data = match std::fs::read(&path) {
        Ok(data) => data,
        Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
            return Err(Error::new(
                "full_disk_access",
                "Sirus needs Full Disk Access to read Safari's cookies.",
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::not_found(
                "Safari has no cookies yet. Open Safari once, then try again.",
            ));
        }
        Err(_) => return Err(Error::new("native", "Safari's cookies could not be read.")),
    };
    parse_binarycookies(&data)
}

fn le_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn be_u32(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        data.get(at..at.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn le_f64(data: &[u8], at: usize) -> Option<f64> {
    Some(f64::from_le_bytes(
        data.get(at..at.checked_add(8)?)?.try_into().ok()?,
    ))
}

/// A NUL-terminated string at `offset` inside one cookie record.
fn c_string(record: &[u8], offset: u32) -> Option<String> {
    let rest = record.get(offset as usize..)?;
    let end = rest.iter().position(|&byte| byte == 0)?;
    String::from_utf8(rest[..end].to_vec()).ok()
}

/// Parses Safari's `Cookies.binarycookies`: `cook`, page count and page sizes (big-endian), then
/// pages of little-endian records. Malformed records are skipped; a truncated file keeps what it had.
pub fn parse_binarycookies(data: &[u8]) -> Result<Vec<RawCookie>> {
    if !data.starts_with(b"cook") {
        return Err(Error::new("invalid", "This is not a Safari cookies file."));
    }
    let pages = be_u32(data, 4).unwrap_or(0) as usize;
    let mut cursor = 8 + 4 * pages;
    let mut cookies = Vec::new();
    for index in 0..pages {
        let Some(size) = be_u32(data, 8 + 4 * index) else {
            break;
        };
        let Some(page) = data.get(cursor..cursor + size as usize) else {
            break;
        };
        cursor += size as usize;
        parse_page(page, &mut cookies);
    }
    Ok(cookies)
}

fn parse_page(page: &[u8], out: &mut Vec<RawCookie>) {
    let Some(count) = le_u32(page, 4) else {
        return;
    };
    for index in 0..count as usize {
        let Some(offset) = le_u32(page, 8 + 4 * index) else {
            return;
        };
        if let Some(cookie) = parse_record(page, offset as usize) {
            out.push(cookie);
        }
    }
}

/// One record: size, flags at 8 (1 secure, 4 HttpOnly), string offsets at 16 (URL), 20 (name),
/// 24 (path), 28 (value), expiry at 40 and creation at 48 (Core Foundation seconds). Strings follow
/// the 56-byte header.
fn parse_record(page: &[u8], start: usize) -> Option<RawCookie> {
    let size = le_u32(page, start)? as usize;
    if size < 56 {
        return None;
    }
    let record = page.get(start..start.checked_add(size)?)?;
    let flags = le_u32(record, 8)?;
    let expires = le_f64(record, 40)?;
    Some(RawCookie {
        host: c_string(record, le_u32(record, 16)?)?,
        name: c_string(record, le_u32(record, 20)?)?,
        path: c_string(record, le_u32(record, 24)?)?,
        value: StoredValue::Plain(c_string(record, le_u32(record, 28)?)?.into_bytes()),
        expires: (expires.is_finite() && expires > 0.0).then_some(expires + MAC_EPOCH_UNIX),
        secure: flags & 1 != 0,
        http_only: flags & 4 != 0,
    })
}

/// Chrome's cookie key: PBKDF2-HMAC-SHA1 of the Keychain password (salt `saltysalt`, 1003 rounds).
pub fn chrome_key(password: &[u8]) -> ChromeKey {
    let mut key = [0u8; CHROME_KEY_LEN];
    pbkdf2_hmac_sha1(password, b"saltysalt", 1003, &mut key);
    key
}

fn pbkdf2_hmac_sha1(password: &[u8], salt: &[u8], rounds: u32, out: &mut [u8]) {
    for (index, chunk) in out.chunks_mut(20).enumerate() {
        let mut mac =
            <Hmac<Sha1> as Mac>::new_from_slice(password).expect("HMAC accepts any key length");
        mac.update(salt);
        mac.update(&((index as u32) + 1).to_be_bytes());
        let mut block = mac.finalize().into_bytes();
        let mut result = block;
        for _ in 1..rounds {
            let mut mac =
                <Hmac<Sha1> as Mac>::new_from_slice(password).expect("HMAC accepts any key length");
            mac.update(&block);
            block = mac.finalize().into_bytes();
            for (accumulated, byte) in result.iter_mut().zip(block.iter()) {
                *accumulated ^= byte;
            }
        }
        chunk.copy_from_slice(&result[..chunk.len()]);
    }
}

/// AES-128-CBC decryption with Chrome's fixed IV (sixteen spaces) and PKCS#7 padding.
fn aes_cbc_decrypt(key: &ChromeKey, data: &[u8]) -> Option<Vec<u8>> {
    if data.is_empty() || !data.len().is_multiple_of(16) {
        return None;
    }
    let cipher = aes::Aes128::new(key.into());
    let mut previous = [b' '; 16];
    let mut out = Vec::with_capacity(data.len());
    for chunk in data.as_chunks::<16>().0 {
        let mut block = aes::Block::clone_from_slice(chunk);
        cipher.decrypt_block(&mut block);
        for (plain, prior) in block.iter().zip(previous.iter()) {
            out.push(plain ^ prior);
        }
        previous.copy_from_slice(chunk);
    }
    let padding = *out.last()? as usize;
    if padding == 0 || padding > 16 || padding > out.len() {
        return None;
    }
    if !out[out.len() - padding..]
        .iter()
        .all(|&byte| byte as usize == padding)
    {
        return None;
    }
    out.truncate(out.len() - padding);
    Some(out)
}

/// Decrypts a `v10` value. Chrome 130 and later prefix the plaintext with SHA-256 of the host key;
/// that prefix is removed only when it matches, so a value is never cut by a guess.
pub fn decrypt_chrome_value(key: &ChromeKey, host: &str, stored: &[u8]) -> Option<String> {
    let body = stored.strip_prefix(b"v10")?;
    let mut plain = aes_cbc_decrypt(key, body)?;
    let digest = Sha256::digest(host.as_bytes());
    if plain.len() >= 32 && plain[..32] == digest[..] {
        plain.drain(..32);
    }
    String::from_utf8(plain).ok()
}

fn expired_at(expires: Option<f64>, now: f64) -> bool {
    expires.is_some_and(|expires| expires <= now)
}

/// Decodes one cookie. `None` means skipped: expired, a value this build cannot decrypt
/// (`v11`, an unknown prefix, or no key), or a value that is not text.
pub fn decode(cookie: RawCookie, key: Option<&ChromeKey>, now: f64) -> Option<DecodedCookie> {
    if expired_at(cookie.expires, now) {
        return None;
    }
    let value = match &cookie.value {
        StoredValue::Plain(bytes) => String::from_utf8(bytes.clone()).ok()?,
        StoredValue::Encrypted(stored) => decrypt_chrome_value(key?, &cookie.host, stored)?,
    };
    Some(DecodedCookie {
        host: cookie.host,
        name: cookie.name,
        path: cookie.path,
        value,
        expires: cookie.expires,
        secure: cookie.secure,
        http_only: cookie.http_only,
    })
}

/// Whether a stored Chrome value needs the key (only `v10` does on macOS).
fn needs_key(cookie: &RawCookie, now: f64) -> bool {
    matches!(&cookie.value, StoredValue::Encrypted(stored) if stored.starts_with(b"v10"))
        && !expired_at(cookie.expires, now)
}

/// The Keychain password for Chrome's cookies. macOS asks the owner to allow the read; that prompt
/// is the consent for the key, and it appears only when a Chrome cookie needs decrypting.
pub fn chrome_safe_storage_password() -> Result<Vec<u8>> {
    let output = std::process::Command::new("/usr/bin/security")
        .args(["find-generic-password", "-w", "-s", CHROME_SAFE_STORAGE])
        .output()
        .map_err(|_| Error::new("native", "The Keychain could not be opened."))?;
    if !output.status.success() {
        return Err(Error::new(
            "keychain",
            "The Keychain did not release Chrome's key. Allow Sirus when macOS asks, then try again.",
        ));
    }
    let mut password = output.stdout;
    while matches!(password.last(), Some(b'\n' | b'\r')) {
        password.pop();
    }
    Ok(password)
}

/// Selects the owner's domains and decodes them. The Keychain is asked for only when a selected
/// cookie is a `v10` value. Returns the decoded cookies and how many were skipped.
pub fn collect_import(
    cookies: Vec<RawCookie>,
    domains: &HashSet<String>,
    now: f64,
    fetch_key: impl FnOnce() -> Result<ChromeKey>,
) -> Result<(Vec<DecodedCookie>, u32)> {
    let selected = select(cookies, domains);
    let key = if selected.iter().any(|cookie| needs_key(cookie, now)) {
        Some(fetch_key()?)
    } else {
        None
    };
    let total = selected.len();
    let decoded: Vec<DecodedCookie> = selected
        .into_iter()
        .filter_map(|cookie| decode(cookie, key.as_ref(), now))
        .collect();
    let skipped = (total - decoded.len()) as u32;
    Ok((decoded, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes::cipher::BlockEncrypt;

    const NOW: f64 = 1_800_000_000.0;

    fn raw(host: &str, name: &str, value: &str, expires: Option<f64>) -> RawCookie {
        RawCookie {
            host: host.into(),
            name: name.into(),
            path: "/".into(),
            value: StoredValue::Plain(value.as_bytes().to_vec()),
            expires,
            secure: true,
            http_only: false,
        }
    }

    /// A Safari record in the layout `parse_record` reads, with strings after the 56-byte header.
    fn safari_record(host: &str, name: &str, value: &str, flags: u32, expires: f64) -> Vec<u8> {
        let mut strings = Vec::new();
        let mut offsets = [0u32; 4];
        for (slot, text) in [host, name, "/", value].iter().enumerate() {
            offsets[slot] = 56 + strings.len() as u32;
            strings.extend_from_slice(text.as_bytes());
            strings.push(0);
        }
        let size = 56 + strings.len() as u32;
        let mut record = Vec::new();
        record.extend_from_slice(&size.to_le_bytes());
        record.extend_from_slice(&0u32.to_le_bytes());
        record.extend_from_slice(&flags.to_le_bytes());
        record.extend_from_slice(&0u32.to_le_bytes());
        for offset in offsets {
            record.extend_from_slice(&offset.to_le_bytes());
        }
        record.extend_from_slice(&[0u8; 8]);
        record.extend_from_slice(&expires.to_le_bytes());
        record.extend_from_slice(&0f64.to_le_bytes());
        record.extend_from_slice(&strings);
        record
    }

    /// A `cook` file with one page holding `records`.
    fn binarycookies(records: &[Vec<u8>]) -> Vec<u8> {
        let header = 8 + 4 * records.len() + 4;
        let mut offsets = Vec::new();
        let mut body = Vec::new();
        for record in records {
            offsets.push((header + body.len()) as u32);
            body.extend_from_slice(record);
        }
        let mut page = Vec::new();
        page.extend_from_slice(&0x0000_0100u32.to_be_bytes());
        page.extend_from_slice(&(records.len() as u32).to_le_bytes());
        for offset in &offsets {
            page.extend_from_slice(&offset.to_le_bytes());
        }
        page.extend_from_slice(&[0u8; 4]);
        page.extend_from_slice(&body);
        let mut file = Vec::new();
        file.extend_from_slice(b"cook");
        file.extend_from_slice(&1u32.to_be_bytes());
        file.extend_from_slice(&(page.len() as u32).to_be_bytes());
        file.extend_from_slice(&page);
        file
    }

    fn chrome_encrypt(key: &ChromeKey, plain: &[u8]) -> Vec<u8> {
        let padding = 16 - plain.len() % 16;
        let mut padded = plain.to_vec();
        padded.extend(std::iter::repeat_n(padding as u8, padding));
        let cipher = aes::Aes128::new(key.into());
        let mut previous = [b' '; 16];
        let mut out = Vec::new();
        for chunk in padded.as_chunks::<16>().0 {
            let mut block = [0u8; 16];
            for (index, byte) in block.iter_mut().enumerate() {
                *byte = chunk[index] ^ previous[index];
            }
            let mut block = aes::Block::clone_from_slice(&block);
            cipher.encrypt_block(&mut block);
            out.extend_from_slice(&block);
            previous.copy_from_slice(&block);
        }
        let mut stored = b"v10".to_vec();
        stored.extend_from_slice(&out);
        stored
    }

    #[test]
    fn binarycookies_parse_fixture_records() {
        let data = binarycookies(&[
            safari_record(
                ".example.com",
                "session",
                "abc",
                5,
                NOW - MAC_EPOCH_UNIX + 3600.0,
            ),
            safari_record("shop.test", "cart", "", 0, 0.0),
        ]);
        let cookies = parse_binarycookies(&data).unwrap();
        assert_eq!(cookies.len(), 2);
        assert_eq!(cookies[0].host, ".example.com");
        assert_eq!(cookies[0].name, "session");
        assert!(cookies[0].secure && cookies[0].http_only);
        assert!((cookies[0].expires.unwrap() - (NOW + 3600.0)).abs() < 1e-3);
        assert!(matches!(&cookies[0].value, StoredValue::Plain(value) if value == b"abc"));
        assert_eq!(
            cookies[1].expires, None,
            "a zero expiry is a session cookie"
        );
        assert!(!cookies[1].secure && !cookies[1].http_only);
    }

    #[test]
    fn binarycookies_reject_other_files_and_keep_what_parsed() {
        assert!(parse_binarycookies(b"SQLite format 3\0").is_err());
        let mut data = binarycookies(&[safari_record("a.test", "x", "1", 0, 0.0)]);
        data.truncate(data.len() - 4);
        assert!(parse_binarycookies(&data).unwrap().is_empty());
    }

    #[test]
    fn chrome_key_matches_rfc6070_pbkdf2_vector() {
        // RFC 6070, PBKDF2-HMAC-SHA1, "password", "salt", 2 iterations, 20 bytes.
        let mut out = [0u8; 20];
        pbkdf2_hmac_sha1(b"password", b"salt", 2, &mut out);
        let expected: [u8; 20] = [
            0xea, 0x6c, 0x01, 0x4d, 0xc7, 0x2d, 0x6f, 0x8c, 0xcd, 0x1e, 0xd9, 0x2a, 0xce, 0x1d,
            0x41, 0xf0, 0xd8, 0xde, 0x89, 0x57,
        ];
        assert_eq!(out, expected);
    }

    #[test]
    fn chrome_values_decrypt_with_and_without_the_host_hash() {
        let key = chrome_key(b"keychain-password");
        let host = ".example.com";
        let stored = chrome_encrypt(&key, b"token-123");
        assert_eq!(
            decrypt_chrome_value(&key, host, &stored).as_deref(),
            Some("token-123")
        );

        // Chrome 130+: the plaintext starts with SHA-256 of the host key.
        let mut prefixed = Sha256::digest(host.as_bytes()).to_vec();
        prefixed.extend_from_slice(b"token-456");
        let stored = chrome_encrypt(&key, &prefixed);
        assert_eq!(
            decrypt_chrome_value(&key, host, &stored).as_deref(),
            Some("token-456")
        );
        // A different host does not match the prefix, so nothing is cut: the hash bytes are not
        // text, and the value is refused rather than imported with garbage in it.
        assert_eq!(decrypt_chrome_value(&key, "other.test", &stored), None);
    }

    #[test]
    fn unknown_chrome_prefixes_and_missing_keys_are_skipped() {
        let key = chrome_key(b"keychain-password");
        let mut v11 = chrome_encrypt(&key, b"x");
        v11[1] = b'1';
        v11[2] = b'1';
        let cookie = |stored: Vec<u8>| RawCookie {
            host: "a.test".into(),
            name: "n".into(),
            path: "/".into(),
            value: StoredValue::Encrypted(stored),
            expires: None,
            secure: false,
            http_only: false,
        };
        assert!(decode(cookie(v11), Some(&key), NOW).is_none());
        assert!(decode(cookie(chrome_encrypt(&key, b"x")), None, NOW).is_none());
        assert!(decode(cookie(b"v10garbage".to_vec()), Some(&key), NOW).is_none());
    }

    #[test]
    fn domains_filter_and_summarise_without_values() {
        let cookies = vec![
            raw(".example.com", "a", "1", None),
            raw("example.com", "b", "2", Some(NOW + 60.0)),
            raw("example.com", "old", "3", Some(NOW - 60.0)),
            raw("other.test", "c", "4", None),
        ];
        let summary = domain_summary(&cookies, NOW);
        assert_eq!(
            summary,
            vec![
                CookieDomain {
                    domain: "example.com".into(),
                    count: 2
                },
                CookieDomain {
                    domain: "other.test".into(),
                    count: 1
                },
            ]
        );
        let wanted = normalize_domains(&["Example.com".to_string()]).unwrap();
        let picked = select(cookies, &wanted);
        assert_eq!(picked.len(), 3, "a leading dot and case do not matter");
        assert!(picked
            .iter()
            .all(|cookie| domain_key(&cookie.host) == "example.com"));
    }

    #[test]
    fn domain_lists_are_bounded_and_plain() {
        assert!(normalize_domains(&[]).is_err());
        assert!(normalize_domains(&["bad/path".to_string()]).is_err());
        assert!(normalize_domains(&["has space.com".to_string()]).is_err());
        assert!(normalize_domains(&["".to_string()]).is_err());
        let many: Vec<String> = (0..=MAX_DOMAINS)
            .map(|index| format!("d{index}.test"))
            .collect();
        assert!(normalize_domains(&many).is_err());
    }

    #[test]
    fn chrome_profiles_are_found_by_folder_and_named_from_local_state() {
        let root = tempfile::tempdir().unwrap();
        for folder in [
            "Default/Network",
            "Profile 2",
            "Guest Profile",
            "Profile 10",
        ] {
            std::fs::create_dir_all(root.path().join(folder)).unwrap();
        }
        std::fs::write(root.path().join("Default/Network/Cookies"), b"").unwrap();
        std::fs::write(root.path().join("Profile 2/Cookies"), b"").unwrap();
        std::fs::write(
            root.path().join("Local State"),
            r#"{"profile":{"info_cache":{"Profile 2":{"name":"Work"}}}}"#,
        )
        .unwrap();
        let sources = chrome_profiles(root.path());
        let ids: Vec<&str> = sources.iter().map(|source| source.id.as_str()).collect();
        assert_eq!(ids, ["chrome:Default", "chrome:Profile 2"]);
        assert_eq!(sources[0].profile, "Default");
        assert_eq!(sources[1].profile, "Work");
        assert!(source_from_id(root.path(), "chrome:../Local State").is_err());
        assert!(source_from_id(root.path(), "chrome:Profile 2").is_ok());
    }

    #[test]
    fn chrome_database_is_read_from_a_copy_as_encrypted_values() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Cookies");
        let database = rusqlite::Connection::open(&path).unwrap();
        database
            .execute_batch(
                "CREATE TABLE cookies (host_key TEXT, name TEXT, value TEXT, encrypted_value BLOB, path TEXT, expires_utc INTEGER, is_secure INTEGER, is_httponly INTEGER);
                 INSERT INTO cookies VALUES ('.example.com', 'sid', '', x'763130', '/', 13000000000000000, 1, 1);
                 INSERT INTO cookies VALUES ('plain.test', 'p', 'open', x'', '/x', 0, 0, 0);",
            )
            .unwrap();
        drop(database);
        let cookies = read_chrome_database(&path).unwrap();
        assert_eq!(cookies.len(), 2);
        assert!(matches!(&cookies[0].value, StoredValue::Encrypted(stored) if stored == b"v10"));
        assert!(cookies[0].secure && cookies[0].http_only);
        assert!(cookies[0].expires.unwrap() > NOW - 1e9);
        assert!(matches!(&cookies[1].value, StoredValue::Plain(value) if value == b"open"));
        assert_eq!(cookies[1].expires, None);
    }

    #[test]
    fn collect_import_asks_for_the_key_only_when_needed() {
        let key = chrome_key(b"keychain-password");
        let encrypted = RawCookie {
            host: ".example.com".into(),
            name: "sid".into(),
            path: "/".into(),
            value: StoredValue::Encrypted(chrome_encrypt(&key, b"secret")),
            expires: None,
            secure: true,
            http_only: true,
        };
        let domains = normalize_domains(&["example.com".to_string()]).unwrap();
        let (decoded, skipped) =
            collect_import(vec![encrypted.clone()], &domains, NOW, || Ok(key)).unwrap();
        assert_eq!(decoded.len(), 1);
        assert_eq!(decoded[0].value, "secret");
        assert_eq!(skipped, 0);

        let plain = raw("example.com", "p", "v", None);
        let (decoded, _) =
            collect_import(vec![plain], &domains, NOW, || panic!("no key needed")).unwrap();
        assert_eq!(decoded.len(), 1);

        let unrelated = raw("other.test", "u", "v", None);
        let (decoded, skipped) =
            collect_import(vec![unrelated], &domains, NOW, || panic!("not selected")).unwrap();
        assert!(decoded.is_empty());
        assert_eq!(skipped, 0, "cookies outside the selection are not counted");
    }
}
