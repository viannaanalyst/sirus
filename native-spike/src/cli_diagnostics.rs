//! Settings → Provedores: what the app knows about each provider CLI. Path and version come from
//! `detect_agents`, the test from `probe_provider`, and the login from `provider_usage` (its account
//! is who the CLI is signed in as). This module reads those replies and says what they mean; the
//! page draws them (`settings_pages.rs`). Nothing here runs a CLI.

use crate::i18n::{t, tf};
use serde_json::Value;

/// The test of one CLI, from `probe_provider`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Probe {
    /// Not installed, so there is nothing to test.
    Absent,
    Pending,
    /// The CLI answered; its message is what the host reports.
    Ok(String),
    Failed(String),
}

/// Whether the CLI is signed in, from its own usage read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Login {
    Absent,
    Pending,
    /// Signed in as this account (an email, a name or a plan).
    SignedIn(String),
    SignedOut,
    /// The read failed or the CLI did not say.
    Unknown(String),
}

/// One provider's diagnostics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub path: Option<String>,
    pub version: Option<String>,
    pub probe: Probe,
    pub login: Login,
}

/// The probe reply `probe_provider` gave, or the error it failed with (`{"error": text}`).
pub fn probe_of(reply: &Value) -> Probe {
    if let Some(error) = reply["error"].as_str() {
        return Probe::Failed(error.to_owned());
    }
    let message = reply["message"].as_str().unwrap_or("").to_owned();
    if reply["ok"].as_bool() == Some(true) {
        Probe::Ok(message)
    } else {
        Probe::Failed(message)
    }
}

/// The login from a `provider_usage` reply. An account with an email, a name or a plan means
/// signed in; no account while the read worked means signed out.
pub fn login_of(usage: &Value) -> Login {
    if let Some(error) = usage["error"].as_str() {
        return Login::Unknown(error.to_owned());
    }
    if usage["status"].as_str() == Some("unavailable") {
        let note = usage["note"].as_str().unwrap_or("").to_owned();
        return Login::Unknown(note);
    }
    let account = &usage["account"];
    if account.is_object() {
        let who = ["email", "name", "plan"]
            .iter()
            .find_map(|key| account[*key].as_str().filter(|text| !text.is_empty()));
        return Login::SignedIn(who.unwrap_or("").to_owned());
    }
    Login::SignedOut
}

/// One entry per provider the host detected, in its order. `probes` and `logins` are keyed by
/// provider id; a missing reply means the check has not come back yet.
pub fn entries(agents: &Value, probes: &Value, logins: &Value) -> Vec<(String, Entry)> {
    agents
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|agent| {
            let id = agent["id"].as_str()?.to_owned();
            let installed = agent["installed"].as_bool() == Some(true);
            let text = |key: &str| agent[key].as_str().filter(|value| !value.is_empty());
            let entry = Entry {
                name: text("name").unwrap_or(&id).to_owned(),
                path: text("path").map(str::to_owned),
                version: text("version").map(str::to_owned),
                probe: match (installed, probes.get(&id)) {
                    (false, _) => Probe::Absent,
                    (true, None) => Probe::Pending,
                    (true, Some(reply)) => probe_of(reply),
                },
                login: match (installed, logins.get(&id)) {
                    (false, _) => Login::Absent,
                    (true, None) => Login::Pending,
                    (true, Some(reply)) => login_of(reply),
                },
            };
            Some((id, entry))
        })
        .collect()
}

/// The text of the probe line.
pub fn probe_text(probe: &Probe) -> String {
    match probe {
        Probe::Absent => "—".to_owned(),
        Probe::Pending => t("Verificando…").to_owned(),
        Probe::Ok(message) | Probe::Failed(message) if message.is_empty() => {
            t("Sem resposta").to_owned()
        }
        Probe::Ok(message) | Probe::Failed(message) => message.clone(),
    }
}

/// The text of the login line.
pub fn login_text(login: &Login) -> String {
    match login {
        Login::Absent => "—".to_owned(),
        Login::Pending => t("Verificando…").to_owned(),
        Login::SignedIn(who) if who.is_empty() => t("Conectado").to_owned(),
        Login::SignedIn(who) => tf("Conectado como {}", &[who]),
        Login::SignedOut => t("Sem login").to_owned(),
        Login::Unknown(note) if note.is_empty() => t("Não foi possível verificar").to_owned(),
        Login::Unknown(note) => note.clone(),
    }
}

/// The labelled lines of an entry, as the page shows them and the copy button sends them.
pub fn lines(entry: &Entry) -> Vec<(String, String)> {
    vec![
        (
            t("Caminho").to_owned(),
            entry
                .path
                .clone()
                .unwrap_or_else(|| t("Nenhum executável").to_owned()),
        ),
        (
            t("Versão").to_owned(),
            entry.version.clone().unwrap_or_else(|| "—".to_owned()),
        ),
        (t("Teste").to_owned(), probe_text(&entry.probe)),
        (t("Login").to_owned(), login_text(&entry.login)),
    ]
}

/// The copy button's text: the provider's name, then one labelled line per field.
pub fn copy_text(entry: &Entry) -> String {
    let mut text = entry.name.clone();
    for (label, value) in lines(entry) {
        text.push_str(&format!("\n{label}: {value}"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn probe_replies() {
        assert_eq!(
            probe_of(&json!({ "ok": true, "version": "1.2", "message": "Responded: 1.2" })),
            Probe::Ok("Responded: 1.2".into())
        );
        assert_eq!(
            probe_of(
                &json!({ "ok": false, "version": null, "message": "Executable is not a file" })
            ),
            Probe::Failed("Executable is not a file".into())
        );
        assert_eq!(
            probe_of(&json!({ "error": "host down" })),
            Probe::Failed("host down".into())
        );
    }

    #[test]
    fn login_from_usage() {
        let signed = json!({ "status": "ok", "account": { "email": "a@b.co", "plan": "max" } });
        assert_eq!(login_of(&signed), Login::SignedIn("a@b.co".into()));
        let plan_only = json!({ "status": "ok", "account": { "plan": "pro" } });
        assert_eq!(login_of(&plan_only), Login::SignedIn("pro".into()));
        let nameless = json!({ "status": "ok", "account": {} });
        assert_eq!(login_of(&nameless), Login::SignedIn(String::new()));
        assert_eq!(
            login_of(&json!({ "status": "ok", "account": null })),
            Login::SignedOut
        );
        assert_eq!(
            login_of(&json!({ "status": "unavailable", "note": "CLI timed out" })),
            Login::Unknown("CLI timed out".into())
        );
        assert_eq!(
            login_of(&json!({ "error": "boom" })),
            Login::Unknown("boom".into())
        );
    }

    #[test]
    fn entries_follow_install_and_replies() {
        let agents = json!([
            { "id": "claude", "name": "Claude Code", "installed": true, "path": "/bin/claude", "version": "1.0" },
            { "id": "codex", "name": "Codex", "installed": false, "path": null, "version": null },
            { "id": "pi", "name": "", "installed": true }
        ]);
        let probes = json!({ "claude": { "ok": true, "message": "Responded: 1.0" } });
        let logins = json!({ "claude": { "status": "ok", "account": null } });
        let rows = entries(&agents, &probes, &logins);
        assert_eq!(rows.len(), 3);
        let claude = &rows[0].1;
        assert_eq!(claude.probe, Probe::Ok("Responded: 1.0".into()));
        assert_eq!(claude.login, Login::SignedOut);
        let codex = &rows[1].1;
        assert_eq!(codex.probe, Probe::Absent);
        assert_eq!(codex.login, Login::Absent);
        assert_eq!(codex.path, None);
        let pi = &rows[2].1;
        assert_eq!(pi.name, "pi");
        assert_eq!(pi.probe, Probe::Pending);
        assert_eq!(pi.login, Login::Pending);
        assert!(entries(&Value::Null, &Value::Null, &Value::Null).is_empty());
    }

    #[test]
    fn copy_text_has_every_field_in_order() {
        let entry = Entry {
            name: "Claude Code".into(),
            path: Some("/bin/claude".into()),
            version: None,
            probe: Probe::Failed("Could not read a version".into()),
            login: Login::SignedIn("a@b.co".into()),
        };
        let text = copy_text(&entry);
        let mut rows = text.lines();
        assert_eq!(rows.next(), Some("Claude Code"));
        assert_eq!(rows.next().unwrap().split(": ").nth(1), Some("/bin/claude"));
        assert_eq!(rows.count(), 3);
        assert!(text.contains("Could not read a version"));
        assert!(text.contains("a@b.co"));
    }

    #[test]
    fn missing_values_have_words_not_blanks() {
        let entry = Entry {
            name: "Pi".into(),
            path: None,
            version: None,
            probe: Probe::Ok(String::new()),
            login: Login::Unknown(String::new()),
        };
        // Compared through `t`, so the test holds in either language.
        let shown = lines(&entry);
        assert_eq!(shown[0].1, t("Nenhum executável"));
        assert_eq!(shown[1].1, "—");
        assert_eq!(shown[2].1, t("Sem resposta"));
        assert_eq!(shown[3].1, t("Não foi possível verificar"));
    }
}
