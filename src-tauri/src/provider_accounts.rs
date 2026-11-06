//! Native-owned, named CLI profiles. Vendor login owns credentials; IPC never accepts them.
use crate::commands::AppState;
use crate::error::{Error, Result};
use crate::models::{AgentProviderId, AppData, ProviderAccount};
use crate::runtime::State;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use tokio::{io::AsyncReadExt, process::Command, sync::watch};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AccountScope {
    pub id: String,
    pub home: Option<PathBuf>,
}
#[derive(Default)]
pub struct AccountState {
    logins: Mutex<HashMap<String, LoginHandle>>,
}
struct LoginHandle {
    cancel: watch::Sender<bool>,
    pid: Option<u32>,
}
fn kill_login(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid {
        unsafe {
            libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
        }
    }
    #[cfg(not(unix))]
    let _ = pid;
}
impl AccountState {
    pub(crate) fn is_logging(&self, provider: &AgentProviderId, id: &str) -> bool {
        self.logins
            .lock()
            .contains_key(&format!("{}::{id}", provider.key()))
    }
    pub fn stop(&self) {
        for handle in self.logins.lock().values() {
            let _ = handle.cancel.send(true);
            // Tauri may exit without another async turn: kill owned groups now.
            kill_login(handle.pid);
        }
    }
}
pub fn supported(provider: &AgentProviderId) -> bool {
    matches!(provider, AgentProviderId::Codex | AgentProviderId::Claude)
}
pub fn selected(data: &AppData, provider: &AgentProviderId) -> String {
    data.selected_provider_accounts
        .get(provider)
        .cloned()
        .unwrap_or_else(crate::models::default_account_id)
}
pub(crate) fn bound_account(
    session: &mut crate::models::Session,
    provider: &AgentProviderId,
    selected: &str,
) -> String {
    session
        .account_bindings
        .entry(session.agent.clone())
        .or_insert_with(|| session.provider_account_id.clone());
    session
        .account_bindings
        .entry(provider.clone())
        .or_insert_with(|| selected.into())
        .clone()
}
fn validate(data: &AppData, provider: &AgentProviderId, id: &str) -> Result<()> {
    if id == "default" {
        return Ok(());
    }
    if !supported(provider)
        || id.len() != 36
        || uuid::Uuid::parse_str(id).is_err()
        || !data
            .provider_accounts
            .iter()
            .any(|a| a.id == id && &a.provider == provider)
    {
        return Err(Error::agent(
            "This provider account is unknown or unsupported.",
        ));
    }
    Ok(())
}
fn checked_dir(path: &Path, create: bool) -> Result<()> {
    if create && !path.exists() {
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder
            .create(path)
            .map_err(|_| Error::agent("Cannot create the isolated account profile."))?;
    }
    let meta = std::fs::symlink_metadata(path)
        .map_err(|_| Error::agent("The isolated account profile is missing."))?;
    if !meta.is_dir() || meta.file_type().is_symlink() {
        return Err(Error::agent("Unsafe account profile directory."));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.uid() != unsafe { libc::geteuid() } || meta.mode() & 0o077 != 0 {
            return Err(Error::agent(
                "The account profile must be private to the current user.",
            ));
        }
    }
    Ok(())
}
fn home(root: &Path, provider: &AgentProviderId, id: &str, create: bool) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for part in ["provider-accounts", provider.key(), id] {
        path.push(part);
        checked_dir(&path, create)?;
    }
    Ok(path)
}
pub(crate) fn scope(
    state: &AppState,
    provider: &AgentProviderId,
    id: &str,
) -> Result<AccountScope> {
    validate(&state.data.lock(), provider, id)?;
    if state.accounts.is_logging(provider, id) {
        return Err(Error::agent(
            "Account sign-in is in progress. Complete it before using this profile.",
        ));
    }
    let home = if id == "default" {
        None
    } else {
        Some(home(
            state
                .data_path
                .parent()
                .ok_or_else(|| Error::agent("Account storage unavailable"))?,
            provider,
            id,
            false,
        )?)
    };
    Ok(AccountScope {
        id: id.into(),
        home,
    })
}
/// The Claude config folder a session's CLI uses: a named account's private profile (the
/// `CLAUDE_CONFIG_DIR` `apply` sets), else the app's own `CLAUDE_CONFIG_DIR` (relative to the
/// workspace, as the CLI reads it), else `~/.claude`. `None` when it cannot be known.
pub(crate) fn claude_config_dir(state: &AppState, account: &str, cwd: &str) -> Option<PathBuf> {
    config_dir_in(
        state.data_path.parent()?,
        account,
        cwd,
        std::env::var_os("CLAUDE_CONFIG_DIR"),
        crate::new_project::home_dir(),
    )
}
fn config_dir_in(
    root: &Path,
    account: &str,
    cwd: &str,
    env: Option<std::ffi::OsString>,
    user_home: Option<PathBuf>,
) -> Option<PathBuf> {
    if account != "default" {
        // Only a well-formed account id names a folder (never `..` or a path).
        uuid::Uuid::parse_str(account).ok()?;
        return home(root, &AgentProviderId::Claude, account, false).ok();
    }
    match env.filter(|dir| !dir.is_empty()) {
        Some(dir) => Some(Path::new(cwd).join(dir)).filter(|dir| dir.is_absolute()),
        None => user_home.map(|home| home.join(".claude")),
    }
}
pub(crate) fn apply(command: &mut Command, provider: &AgentProviderId, home: Option<&Path>) {
    if let Some(home) = home {
        match provider {
            AgentProviderId::Codex => {
                crate::skills::share_with_account(".codex/skills", &home.join("skills"));
                command.env("CODEX_HOME", home);
                for key in ["OPENAI_API_KEY", "CODEX_API_KEY", "CODEX_ACCESS_TOKEN"] {
                    command.env_remove(key);
                }
            }
            AgentProviderId::Claude => {
                crate::skills::share_with_account(".claude/skills", &home.join("skills"));
                command
                    .env("CLAUDE_CONFIG_DIR", home)
                    .env("CLAUDE_SECURESTORAGE_CONFIG_DIR", home);
                for key in [
                    "ANTHROPIC_API_KEY",
                    "ANTHROPIC_AUTH_TOKEN",
                    "CLAUDE_CODE_OAUTH_TOKEN",
                ] {
                    command.env_remove(key);
                }
            }
            _ => {}
        }
    }
}
fn label(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > 48 || value.chars().any(char::is_control) {
        return Err(Error::agent("Account name must contain 1–48 characters."));
    }
    Ok(value.into())
}
pub async fn create_provider_account(
    state: State<'_, Arc<AppState>>,
    provider: AgentProviderId,
    name: String,
) -> Result<ProviderAccount> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let mut data = state.data.lock(); state.ensure_running()?;
        if !supported(&provider) || data.provider_accounts.len() >= 32 { return Err(Error::agent("Another account is not supported for this provider or the profile limit was reached.")); }
        let account = ProviderAccount { id: uuid::Uuid::new_v4().to_string(), provider, label: label(&name)? };
        home(state.data_path.parent().ok_or_else(|| Error::agent("Account storage unavailable"))?, &account.provider, &account.id, true)?;
        data.provider_accounts.push(account.clone());
        if let Err(error) = crate::persist::save(&state.data_path, &data) { data.provider_accounts.pop(); return Err(error); }
        Ok(account)
    }).await.map_err(|_| Error::agent("Cannot create account profile"))?
}
pub async fn select_provider_account(
    state: State<'_, Arc<AppState>>,
    provider: AgentProviderId,
    account_id: String,
) -> Result<()> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        scope(&state, &provider, &account_id)?;
        let mut data = state.data.lock();
        state.ensure_running()?;
        let previous = data
            .selected_provider_accounts
            .insert(provider.clone(), account_id);
        if let Err(error) = crate::persist::save(&state.data_path, &data) {
            if let Some(previous) = previous {
                data.selected_provider_accounts.insert(provider, previous);
            } else {
                data.selected_provider_accounts.remove(&provider);
            }
            return Err(error);
        }
        Ok(())
    })
    .await
    .map_err(|_| Error::agent("Cannot select account profile"))?
}
/// Removes an app-managed account and its folder (its sign-in and its copies of transcripts). The
/// default account is the machine's own CLI login, and a conversation still on the account would
/// lose its way back, so both refuse.
pub async fn remove_provider_account(
    state: State<'_, Arc<AppState>>,
    provider: AgentProviderId,
    account_id: String,
) -> Result<()> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let mut data = state.data.lock();
        state.ensure_running()?;
        validate(&data, &provider, &account_id)?;
        if account_id == crate::models::default_account_id() {
            return Err(Error::agent("The default account cannot be removed here."));
        }
        let using = data
            .sessions
            .iter()
            .filter(|session| {
                (session.agent == provider && session.provider_account_id == account_id)
                    || session.account_bindings.get(&provider) == Some(&account_id)
            })
            .count();
        if using > 0 {
            return Err(Error::agent(format!(
                "{using} conversation(s) still use this account. Move or delete them first."
            )));
        }
        let before = data.provider_accounts.clone();
        data.provider_accounts
            .retain(|account| !(account.id == account_id && account.provider == provider));
        let selected = data.selected_provider_accounts.get(&provider).cloned();
        if selected.as_deref() == Some(account_id.as_str()) {
            data.selected_provider_accounts.remove(&provider);
        }
        if let Err(error) = crate::persist::save(&state.data_path, &data) {
            data.provider_accounts = before;
            if let Some(selected) = selected {
                data.selected_provider_accounts.insert(provider, selected);
            }
            return Err(error);
        }
        let root = state
            .data_path
            .parent()
            .ok_or_else(|| Error::agent("Account storage unavailable"))?;
        if let Ok(folder) = home(root, &provider, &account_id, false) {
            let _ = std::fs::remove_dir_all(folder);
        }
        Ok(())
    })
    .await
    .map_err(|_| Error::agent("Cannot remove account profile"))?
}

pub async fn rename_provider_account(
    state: State<'_, Arc<AppState>>,
    provider: AgentProviderId,
    account_id: String,
    name: String,
) -> Result<ProviderAccount> {
    let state = state.inner().clone();
    tokio::task::spawn_blocking(move || {
        let mut data = state.data.lock();
        state.ensure_running()?;
        validate(&data, &provider, &account_id)?;
        let account = data
            .provider_accounts
            .iter_mut()
            .find(|a| a.id == account_id && a.provider == provider)
            .ok_or_else(|| Error::agent("The default account cannot be renamed here."))?;
        let previous = account.label.clone();
        account.label = label(&name)?;
        let result = account.clone();
        if let Err(error) = crate::persist::save(&state.data_path, &data) {
            data.provider_accounts
                .iter_mut()
                .find(|a| a.id == account_id)
                .unwrap()
                .label = previous;
            return Err(error);
        }
        Ok(result)
    })
    .await
    .map_err(|_| Error::agent("Cannot rename account profile"))?
}
struct LoginGuard {
    state: Arc<AppState>,
    key: String,
    provider: AgentProviderId,
    account_id: String,
    pid: Option<u32>,
}
impl Drop for LoginGuard {
    fn drop(&mut self) {
        kill_login(self.pid);
        self.state
            .usage
            .invalidate(&self.provider, &self.account_id);
        self.state.accounts.logins.lock().remove(&self.key);
    }
}
async fn drain(reader: impl tokio::io::AsyncRead + Unpin) -> Result<()> {
    let count = tokio::io::copy(&mut reader.take(131_073), &mut tokio::io::sink())
        .await
        .map_err(|_| Error::agent("Sign-in output failed."))?;
    if count > 131_072 {
        return Err(Error::agent("Sign-in output exceeded its safe limit."));
    }
    Ok(())
}
pub async fn login_provider_account(
    state: State<'_, Arc<AppState>>,
    provider: AgentProviderId,
    account_id: String,
) -> Result<()> {
    login(state.inner().clone(), provider, account_id).await
}
async fn login(state: Arc<AppState>, provider: AgentProviderId, account_id: String) -> Result<()> {
    state.ensure_running()?;
    // Never replace the user's global login or the credentials of a used session.
    if account_id == "default" {
        return Err(Error::agent(
            "Sign in to the default account using the provider's own CLI.",
        ));
    }
    let scope = scope(&state, &provider, &account_id)?;
    let (install, key, mut cancel) = {
        let data = state.data.lock();
        state.ensure_running()?;
        if data.sessions.iter().any(|s| {
            (s.agent == provider && s.provider_account_id == account_id
                || s.account_bindings.get(&provider) == Some(&account_id))
                && (!s.messages.is_empty() || s.native_thread.is_some() || s.status.is_active())
        }) {
            return Err(Error::agent(
                "This profile belongs to existing sessions. Add another account instead.",
            ));
        }
        let install =
            crate::detect::resolve_with_overrides(&provider, &data.settings.provider_paths)
                .filter(|i| i.installed)
                .ok_or_else(|| Error::agent("Provider CLI is not installed."))?;
        let key = format!("{}::{account_id}", provider.key());
        let mut logins = state.accounts.logins.lock();
        if !logins.is_empty() {
            return Err(Error::agent(
                "Another account sign-in is already in progress.",
            ));
        }
        let (sender, cancel) = watch::channel(false);
        logins.insert(
            key.clone(),
            LoginHandle {
                cancel: sender,
                pid: None,
            },
        );
        (install, key, cancel)
    };
    let mut guard = LoginGuard {
        state: state.clone(),
        key,
        provider: provider.clone(),
        account_id: account_id.clone(),
        pid: None,
    };
    state.usage.invalidate(&provider, &account_id);
    state.ensure_running()?;
    let mut command = Command::new(install.path.unwrap_or(install.binary));
    command.args(if provider == AgentProviderId::Codex {
        vec!["login"]
    } else {
        vec!["auth", "login", "--claudeai"]
    });
    apply(&mut command, &provider, scope.home.as_deref());
    command
        .current_dir(scope.home.as_ref().unwrap())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(unix)]
    command.process_group(0);
    let mut child = {
        // Same lock as native shutdown: a child is either refused or registered
        // before Quit can synchronously terminate owned login groups.
        let _data = state.data.lock();
        state.ensure_running()?;
        let child = command
            .spawn()
            .map_err(|_| Error::agent("Cannot start the provider sign-in."))?;
        guard.pid = child.id();
        if let Some(handle) = state.accounts.logins.lock().get_mut(&guard.key) {
            handle.pid = guard.pid;
        }
        child
    };
    let output = child.stdout.take().unwrap();
    let errors = child.stderr.take().unwrap();
    let result = tokio::select! {
        _ = cancel.changed() => Err(Error::agent("Account sign-in was cancelled.")),
        value = tokio::time::timeout(std::time::Duration::from_secs(600), async {
            let (status, _, _) = tokio::try_join!(async { child.wait().await.map_err(|_| Error::agent("Sign-in process failed.")) }, drain(output), drain(errors))?;
            if !status.success() { return Err(Error::agent("The provider did not complete sign-in. Try again.")); }
            Ok(())
        }) => value.map_err(|_| Error::agent("Account sign-in timed out. Try again."))?,
    };
    drop(guard);
    result
}
pub fn cancel_provider_account_login(
    state: State<Arc<AppState>>,
    provider: AgentProviderId,
    account_id: String,
) -> Result<()> {
    if account_id.len() != 36 || uuid::Uuid::parse_str(&account_id).is_err() {
        return Err(Error::agent("Invalid account profile."));
    }
    let key = format!("{}::{account_id}", provider.key());
    if let Some(handle) = state.accounts.logins.lock().get(&key) {
        let _ = handle.cancel.send(true);
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_config_folder_follows_the_session_account() {
        let root = tempfile::tempdir().unwrap();
        let id = uuid::Uuid::new_v4().to_string();
        let profile = home(root.path(), &AgentProviderId::Claude, &id, true).unwrap();
        let user = Some(PathBuf::from("/Users/me"));
        assert_eq!(
            config_dir_in(
                root.path(),
                &id,
                "/w",
                Some("/elsewhere".into()),
                user.clone()
            ),
            Some(profile),
            "a named account reads its own profile, never the app's folder"
        );
        assert_eq!(
            config_dir_in(root.path(), "../../x", "/w", None, user.clone()),
            None
        );
        let missing = uuid::Uuid::new_v4().to_string();
        assert_eq!(
            config_dir_in(root.path(), &missing, "/w", None, user.clone()),
            None
        );
        assert_eq!(
            config_dir_in(root.path(), "default", "/w", None, user.clone()),
            Some(PathBuf::from("/Users/me/.claude"))
        );
        assert_eq!(
            config_dir_in(
                root.path(),
                "default",
                "/w",
                Some("cfg".into()),
                user.clone()
            ),
            Some(PathBuf::from("/w/cfg"))
        );
        assert_eq!(
            config_dir_in(root.path(), "default", "/w", Some("/c".into()), user),
            Some(PathBuf::from("/c"))
        );
        assert_eq!(
            config_dir_in(root.path(), "default", "/w", None, None),
            None
        );
    }
    #[cfg(unix)]
    #[test]
    fn shutdown_kills_a_login_without_waiting_for_an_async_cancellation_turn() {
        use std::os::unix::process::CommandExt;
        let mut child = std::process::Command::new("/bin/sleep")
            .arg("60")
            .process_group(0)
            .spawn()
            .unwrap();
        let accounts = AccountState::default();
        let (cancel, _) = watch::channel(false);
        accounts.logins.lock().insert(
            "fixture".into(),
            LoginHandle {
                cancel,
                pid: Some(child.id()),
            },
        );
        accounts.stop();
        // wait() reaps the already synchronously killed child; no Tokio task exists.
        assert!(!child.wait().unwrap().success());
    }
    fn fixture_state(root: &Path, id: &str, binary: &Path) -> Arc<AppState> {
        let mut data = AppData::default();
        data.provider_accounts.push(ProviderAccount {
            id: id.into(),
            provider: AgentProviderId::Codex,
            label: "Fixture".into(),
        });
        data.settings
            .provider_paths
            .insert(AgentProviderId::Codex, binary.to_string_lossy().into());
        home(root, &AgentProviderId::Codex, id, true).unwrap();
        Arc::new(AppState {
            data_path: root.join("state.json"),
            worktree_root: root.join("trees"),
            data: Mutex::new(data),
            agents: Mutex::new(HashMap::new()),
            ptys: Mutex::new(HashMap::new()),
            closing: std::sync::atomic::AtomicBool::new(false),
            attachment_picker: std::sync::atomic::AtomicBool::new(false),
            attachments: Default::default(),
            usage: crate::provider_usage::UsageState::default(),
            accounts: AccountState::default(),
            close_guard: Mutex::new(crate::close::CloseGuard::default()),
            draft_checkpoint: Mutex::new(None),
            checkpoint_pending: Default::default(),
            catalogs: Mutex::new(HashMap::new()),
        })
    }
    #[cfg(unix)]
    #[tokio::test]
    async fn fixed_login_is_scoped_cancellable_and_refuses_default_or_used_profiles() {
        use std::os::unix::fs::PermissionsExt;
        let root = crate::git::tests::Repo::new();
        let id = uuid::Uuid::new_v4().to_string();
        let binary = root.0.join("login-fixture");
        std::fs::write(
            &binary,
            r#"#!/usr/bin/env python3
import os,sys,pathlib
assert sys.argv[1:]==['login']
assert os.path.realpath(os.environ['CODEX_HOME'])==os.getcwd()
assert all(k not in os.environ for k in ['OPENAI_API_KEY','CODEX_API_KEY','CODEX_ACCESS_TOKEN'])
pathlib.Path('login-marker').write_text('fixture')
"#,
        )
        .unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let state = fixture_state(&root.0, &id, &binary);
        assert!(
            login(state.clone(), AgentProviderId::Codex, "default".into())
                .await
                .is_err()
        );
        assert!(login(state.clone(), AgentProviderId::Claude, id.clone())
            .await
            .is_err());
        login(state.clone(), AgentProviderId::Codex, id.clone())
            .await
            .unwrap();
        assert!(home(&root.0, &AgentProviderId::Codex, &id, false)
            .unwrap()
            .join("login-marker")
            .exists());
        assert!(state.accounts.logins.lock().is_empty());
        std::fs::write(
            &binary,
            r#"#!/usr/bin/env python3
import time
time.sleep(60)
"#,
        )
        .unwrap();
        let work = login(state.clone(), AgentProviderId::Codex, id.clone());
        let cancel = async {
            for _ in 0..100 {
                if state.accounts.is_logging(&AgentProviderId::Codex, &id) {
                    state.accounts.stop();
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            panic!("Fixture login did not start");
        };
        let (result, _) = tokio::join!(work, cancel);
        assert!(result.is_err());
        assert!(state.accounts.logins.lock().is_empty());
        state.data.lock().sessions.push(serde_json::from_value(serde_json::json!({"id":"s","title":"Task","projectId":"p","agent":"codex","providerAccountId":id,"status":"completed","createdAt":"time","lastActivityAt":"time","worktree":{"path":"/unused","branch":"main","isolated":false},"messages":[{"id":"m","sessionId":"s","role":"user","content":"old task","createdAt":"time","streaming":false}]})).unwrap());
        assert!(login(state.clone(), AgentProviderId::Codex, id)
            .await
            .unwrap_err()
            .to_string()
            .contains("existing sessions"));
    }
    #[test]
    fn account_registry_rejects_foreign_ids_and_legacy_sessions_use_default() {
        let mut data = AppData::default();
        let id = uuid::Uuid::new_v4().to_string();
        data.provider_accounts.push(ProviderAccount {
            id: id.clone(),
            provider: AgentProviderId::Codex,
            label: "Work".into(),
        });
        assert!(validate(&data, &AgentProviderId::Codex, &id).is_ok());
        for (provider, id) in [
            (AgentProviderId::Claude, id.as_str()),
            (AgentProviderId::Codex, "../auth"),
            (AgentProviderId::Cursor, "unknown"),
        ] {
            assert!(validate(&data, &provider, id).is_err());
        }
        assert_eq!(selected(&data, &AgentProviderId::Codex), "default");
        assert!(label("\n").is_err());
        assert!(label(&"a".repeat(49)).is_err());
    }
    #[test]
    fn switching_providers_preserves_original_account_even_after_defaults_change() {
        let mut session: crate::models::Session = serde_json::from_value(serde_json::json!({
            "id":"s","title":"Task","projectId":"p","agent":"codex",
            "status":"idle","createdAt":"time","lastActivityAt":"time",
            "worktree":{"path":"/unused","branch":"main","isolated":false},"messages":[]
        }))
        .unwrap();
        assert_eq!(session.provider_account_id, "default");
        let claude = bound_account(&mut session, &AgentProviderId::Claude, "claude-original");
        session.agent = AgentProviderId::Claude;
        session.provider_account_id = claude;
        assert_eq!(
            bound_account(&mut session, &AgentProviderId::Codex, "codex-new"),
            "default"
        );
        assert_eq!(
            bound_account(&mut session, &AgentProviderId::Claude, "claude-new"),
            "claude-original"
        );
        let restored: crate::models::Session =
            serde_json::from_value(serde_json::to_value(session).unwrap()).unwrap();
        assert_eq!(
            restored.account_bindings[&AgentProviderId::Codex],
            "default"
        );
        assert_eq!(
            restored.account_bindings[&AgentProviderId::Claude],
            "claude-original"
        );
    }
    #[test]
    fn profiles_are_private_and_do_not_follow_directory_links() {
        let root = crate::git::tests::Repo::new();
        let id = uuid::Uuid::new_v4().to_string();
        let path = home(&root.0, &AgentProviderId::Codex, &id, true).unwrap();
        assert!(path.starts_with(&root.0));
        #[cfg(unix)]
        {
            let link = path.parent().unwrap().join("link");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(checked_dir(&link, false).is_err());
        }
    }
    #[tokio::test]
    async fn profile_environment_is_child_only_and_cannot_inherit_another_account() {
        for provider in [AgentProviderId::Codex, AgentProviderId::Claude] {
            let mut cmd = Command::new("/usr/bin/env");
            apply(&mut cmd, &provider, Some(Path::new("/fixture/profile")));
            let env = cmd.as_std().get_envs().collect::<HashMap<_, _>>();
            let name = if provider == AgentProviderId::Codex {
                "CODEX_HOME"
            } else {
                "CLAUDE_CONFIG_DIR"
            };
            assert_eq!(
                env.get(std::ffi::OsStr::new(name)).unwrap().unwrap(),
                "/fixture/profile"
            );
            let key = if provider == AgentProviderId::Codex {
                "OPENAI_API_KEY"
            } else {
                "ANTHROPIC_API_KEY"
            };
            assert_eq!(env.get(std::ffi::OsStr::new(key)), Some(&None));
        }
    }
}
