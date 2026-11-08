//! `/stage` and `/commit` typed in the composer: Git without opening Alterações, as Monocode's
//! slash commands. `/stage` prepares every change; `/commit` prepares everything when nothing is
//! staged yet and commits with the typed message, or with a title written from the diff.

use super::*;

#[derive(Debug, PartialEq)]
pub(crate) enum GitSlash {
    Stage,
    Commit(Option<String>),
}

/// The command a draft is, if it is one: the word alone, any case; `/commit` keeps the rest.
pub(crate) fn parse(text: &str) -> Option<GitSlash> {
    let text = text.trim();
    let (word, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
    let rest = rest.trim();
    if word.eq_ignore_ascii_case("/stage") && rest.is_empty() {
        Some(GitSlash::Stage)
    } else if word.eq_ignore_ascii_case("/commit") {
        Some(GitSlash::Commit(
            (!rest.is_empty()).then(|| rest.to_owned()),
        ))
    } else {
        None
    }
}

async fn settle(rx: async_channel::Receiver<Result<Value, Value>>) -> Result<Value, String> {
    match rx.recv().await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(crate::i18n::from_host(
            error
                .as_str()
                .or(error["message"].as_str())
                .unwrap_or("O Git não respondeu."),
        )
        .to_owned()),
        Err(_) => Err(t("O motor do Sirus não respondeu.").into()),
    }
}

/// The index token, what is staged, and what could be.
fn read(value: &Value) -> (String, usize, Vec<String>) {
    let snapshot = &value["snapshot"];
    let token = snapshot["indexToken"].as_str().unwrap_or("").to_owned();
    let staged = snapshot["staged"].as_array().map_or(0, Vec::len);
    let loose = snapshot["unstaged"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| entry["actionable"].as_bool().unwrap_or(false))
        .filter_map(|entry| entry["path"].as_str().map(str::to_owned))
        .collect();
    (token, staged, loose)
}

fn files(count: usize) -> String {
    if count == 1 {
        t("1 arquivo").to_owned()
    } else {
        t("{n} arquivos").replace("{n}", &count.to_string())
    }
}

impl Shell {
    /// Runs a typed `/stage` or `/commit`; false when the draft is no such command.
    pub(super) fn typed_git(
        &mut self,
        ix: usize,
        text: &str,
        has_attachments: bool,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(command) = parse(text) else {
            return false;
        };
        if has_attachments {
            return false;
        }
        let session = self.conversations[ix].session_id.clone();
        if session.is_empty() {
            return false;
        }
        self.drafts.remove(&ix);
        self.touch_drafts();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let outcome = run(&backend, &session, command).await;
            let _ = this.update(cx, |this, cx| {
                match outcome {
                    Ok(done) => this.note(done),
                    Err(problem) => this.problem = Some(problem),
                }
                this.refresh_diff();
                cx.notify();
            });
        })
        .detach();
        cx.notify();
        true
    }

    /// A toast that reports instead of warning.
    pub(super) fn note(&mut self, text: String) {
        self.notice = Some(text.clone());
        self.problem = Some(text);
    }
}

async fn run(backend: &Backend, session: &str, command: GitSlash) -> Result<String, String> {
    let snapshot = settle(backend.request(
        "git_workspace_action",
        json!({ "action": { "type": "snapshot", "sessionId": session } }),
    ))
    .await?;
    if !snapshot["snapshot"]["identity"]["isRepo"]
        .as_bool()
        .unwrap_or(false)
    {
        return Err(t("Esta pasta não é um repositório Git.").into());
    }
    let (mut token, mut staged, loose) = read(&snapshot);
    let stage = matches!(command, GitSlash::Stage) || staged == 0;
    let mut added = 0;
    if stage && !loose.is_empty() {
        let after = settle(backend.request(
            "git_workspace_action",
            json!({ "action": { "type": "stage", "sessionId": session, "paths": loose, "expectedIndex": token } }),
        ))
        .await?;
        added = loose.len();
        (token, staged, _) = read(&after);
    }
    let message = match command {
        GitSlash::Stage => {
            return Ok(if added == 0 {
                t("Nada novo para preparar.").into()
            } else {
                t("{files} preparados para o commit.").replace("{files}", &files(added))
            });
        }
        GitSlash::Commit(message) => message,
    };
    if staged == 0 {
        return Err(t("Nada para o commit: não há alterações.").into());
    }
    let message = match message {
        Some(message) => message,
        None => {
            let reply = settle(backend.request(
                "commit_title_action",
                json!({ "action": { "type": "generate", "sessionId": session, "expectedIndex": token, "requestId": crate::panel_sidechat::uuid() } }),
            ))
            .await?;
            reply["title"]
                .as_str()
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned)
                .ok_or_else(|| {
                    t("Não deu para escrever o título. Use /commit seguido da mensagem.").to_owned()
                })?
        }
    };
    settle(backend.request(
        "git_commit",
        json!({ "sessionId": session, "message": message, "expectedIndex": token }),
    ))
    .await?;
    Ok(t("Commit feito: {title}").replace("{title}", &message))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_bare_words_are_commands() {
        assert_eq!(parse(" /stage "), Some(GitSlash::Stage));
        assert_eq!(parse("/STAGE"), Some(GitSlash::Stage));
        assert_eq!(parse("/stage tudo"), None);
        assert_eq!(parse("/commit"), Some(GitSlash::Commit(None)));
        assert_eq!(
            parse("/commit  corrige o login "),
            Some(GitSlash::Commit(Some("corrige o login".into())))
        );
        assert_eq!(parse("/commits"), None);
        assert_eq!(parse("faça /commit"), None);
    }
}
