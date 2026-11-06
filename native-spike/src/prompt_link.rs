//! Prompt links (ADR-122) in Sirus Native: `sirus://new?prompt=…` opens a new conversation of the
//! right project with the prompt in its composer, and "Copiar link do prompt" builds one from the
//! open composer. The host reads and checks a link (`resolve_prompt_link`, the same parser and
//! project matching as Sirus, hidden characters removed); this module only decides where the
//! checked prompt lands. A link never sends anything: the person reads the draft and sends it.
//!
//! macOS hands the link to this app as an Apple Event (`application:openURLs:`); main.rs turns it
//! into a message for the window. A link that starts the app waits until the host has loaded the
//! projects. A child of `shell` (`#[path]`), so it reaches the window's private state.
//!
//! Where the draft goes: a conversation with no messages and no draft in the project, else a new
//! one, so a link never replaces what the person was writing. The link's provider and model apply
//! to a new conversation only when that provider is installed and enabled here.

use super::*;

/// What the host made of a link.
#[derive(Debug, PartialEq)]
pub(crate) enum Outcome {
    Open(Open),
    /// The host's reason key (`LinkError`, camelCase).
    Invalid(String),
}

#[derive(Debug, PartialEq)]
pub(crate) struct Open {
    pub prompt: String,
    pub hidden: usize,
    pub project_id: Option<String>,
    /// The project or repository the link asked for and no project matched.
    pub missing: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

/// The first `sirus://` link among the URLs macOS delivered.
pub(crate) fn link_in(urls: &[String]) -> Option<String> {
    urls.iter()
        .find(|url| {
            url.get(..6)
                .is_some_and(|scheme| scheme.eq_ignore_ascii_case("sirus:"))
        })
        .cloned()
}

pub(crate) fn outcome_of(value: &Value) -> Option<Outcome> {
    let text = |key: &str| value[key].as_str().map(str::to_owned);
    match value["kind"].as_str()? {
        "invalid" => Some(Outcome::Invalid(text("reason")?)),
        "open" => Some(Outcome::Open(Open {
            prompt: text("prompt")?,
            hidden: value["hiddenCharacters"].as_u64().unwrap_or(0) as usize,
            project_id: text("projectId"),
            missing: text("missingProject"),
            provider: text("provider"),
            model: text("model"),
        })),
        _ => None,
    }
}

/// Why a link was refused, in the person's words (Sirus's `promptLink.reason.*`).
pub(crate) fn reason_text(reason: &str) -> &'static str {
    match reason {
        "malformed" => t("Não é um link sirus://new."),
        "tooLong" => t("O prompt tem mais de 8.000 caracteres."),
        "unknownParameter" => t("Ele tem um parâmetro que o Sirus não conhece."),
        "repeatedParameter" => t("Ele repete um parâmetro."),
        "noPrompt" => t("Ele não tem prompt."),
        "hiddenOnly" => t("O prompt só tinha caracteres ocultos, então pode ser malicioso."),
        "project" => t("O projeto não é válido."),
        "repo" => t("O repositório não está no formato dono/nome."),
        "provider" => t("Ele cita um provedor que o Sirus não tem."),
        "model" => t("O modelo não é válido ou não tem provedor."),
        "pullRequest" => t("O número do pull request não é válido."),
        _ => t("O link não pôde ser lido."),
    }
}

/// What came of the link's provider and model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderNote {
    /// The link named none.
    Silent,
    Applied,
    /// Not installed or turned off here: the current provider stays.
    Unavailable,
    /// The provider is used, with its default model.
    ModelUnknown,
}

/// The provider and model a new conversation takes from a link. `models` is the provider's loaded
/// list as (id, name); an empty list cannot refute a model, so it passes (Sirus's
/// `linkedModel`).
pub(crate) fn pick_provider(
    wanted: Option<&str>,
    model: Option<&str>,
    enabled: &[&'static str],
    models: &[(String, String)],
) -> (Option<&'static str>, Option<String>, ProviderNote) {
    let Some(wanted) = wanted else {
        return (None, None, ProviderNote::Silent);
    };
    let Some(provider) = enabled.iter().copied().find(|id| *id == wanted) else {
        return (None, None, ProviderNote::Unavailable);
    };
    let Some(model) = model else {
        return (Some(provider), None, ProviderNote::Applied);
    };
    if models.is_empty() {
        return (
            Some(provider),
            Some(model.to_owned()),
            ProviderNote::Applied,
        );
    }
    let lower = model.to_lowercase();
    let found = models
        .iter()
        .find(|(id, _)| id.to_lowercase() == lower)
        .or_else(|| models.iter().find(|(_, name)| name.to_lowercase() == lower));
    match found {
        Some((id, _)) => (Some(provider), Some(id.clone()), ProviderNote::Applied),
        None => (Some(provider), None, ProviderNote::ModelUnknown),
    }
}

/// The sentences the notice shows; none means the link opened plainly.
pub(crate) fn notes(open: &Open, provider: ProviderNote) -> Vec<String> {
    let mut notes = Vec::new();
    if open.hidden > 0 {
        notes.push(tf(
            "O Sirus removeu {} caractere(s) invisível(is), que podem esconder instruções para o agente. O link pode ser malicioso: leia o prompt antes de enviar.",
            &[&open.hidden],
        ));
    }
    if let Some(missing) = &open.missing {
        notes.push(tf(
            "Nenhum projeto aqui corresponde a “{}”, então o rascunho está no projeto atual.",
            &[missing],
        ));
    }
    let name = open.provider.as_deref().unwrap_or("");
    match provider {
        ProviderNote::Silent => {}
        ProviderNote::Applied => notes.push(tf("O link escolheu {} para esta conversa.", &[&name])),
        ProviderNote::Unavailable => notes.push(tf(
            "O link pediu {}, que não está configurado aqui; o seu padrão continua.",
            &[&name],
        )),
        ProviderNote::ModelUnknown => notes.push(tf(
            "O link escolheu {}, mas o modelo não está disponível aqui; o modelo padrão é usado.",
            &[&name],
        )),
    }
    notes
}

/// The arguments of `build_prompt_link` for the open composer: its project, trimmed draft,
/// provider and model. None when the draft is empty.
pub(crate) fn copy_args(
    project_id: Option<&str>,
    draft: &str,
    provider: &str,
    model: Option<&str>,
) -> Option<Value> {
    let prompt = draft.trim();
    if prompt.is_empty() {
        return None;
    }
    Some(json!({
        "projectId": project_id,
        "prompt": prompt,
        "provider": provider,
        "model": model,
    }))
}

impl Shell {
    /// A `sirus://` link arrived (macOS, or the command line). It waits for the host's state, then
    /// goes to the host to be checked.
    pub(crate) fn prompt_link_received(&mut self, url: String, cx: &mut Context<Self>) {
        self.link_pending = Some(url);
        self.take_prompt_link(cx);
    }

    /// Sends the waiting link, once the host is connected and the installed providers are known
    /// (the link may name one).
    pub(super) fn take_prompt_link(&mut self, cx: &mut Context<Self>) {
        if !self.connected || self.agent_versions.is_empty() {
            return;
        }
        let Some(url) = self.link_pending.take() else {
            return;
        };
        let call = self
            .backend
            .invoke("resolve_prompt_link", json!({ "url": url }));
        self.waiting.insert(call, Waiting::PromptLink);
        cx.notify();
    }

    pub(super) fn prompt_link_resolved(&mut self, ok: bool, value: &Value, cx: &mut Context<Self>) {
        if !ok {
            self.problem = Some(error_text(value));
            return;
        }
        match outcome_of(value) {
            Some(Outcome::Invalid(reason)) => {
                self.problem = Some(format!(
                    "{} {}",
                    t("Este link de prompt não foi aberto."),
                    reason_text(&reason)
                ));
            }
            Some(Outcome::Open(open)) => self.open_prompt_link(open, cx),
            None => self.problem = Some(t("O link não pôde ser lido.").to_owned()),
        }
        cx.notify();
    }

    fn open_prompt_link(&mut self, open: Open, cx: &mut Context<Self>) {
        let matched = open
            .project_id
            .as_deref()
            .and_then(|id| self.projects.iter().position(|p| p.id == id));
        let project = matched
            .or(self.current_project)
            .or_else(|| self.project_order.first().copied());
        let Some(project) = project else {
            self.problem = Some(t("Adicione um projeto para abrir links de prompt.").to_owned());
            return;
        };
        let active = self.active_conversation();
        let enabled = self.enabled_providers();
        let models: Vec<(String, String)> = open
            .provider
            .as_deref()
            .and_then(|wanted| self.models.get(wanted))
            .map(|list| {
                list.iter()
                    .map(|m| (m.id.clone(), m.name.clone()))
                    .collect()
            })
            .unwrap_or_default();
        let (agent, model, note) = pick_provider(
            open.provider.as_deref(),
            open.model.as_deref(),
            &enabled,
            &models,
        );
        let shown = notes(&open, note);

        // A conversation with nothing in it and no draft is already a new one; the link's own
        // provider or model needs a conversation made for it.
        let blank = (agent.is_none()).then(|| {
            self.projects[project].conversations.iter().find_map(|&ix| {
                let conversation = &self.conversations[ix];
                let free = conversation.is_blank()
                    && !conversation.working()
                    && self
                        .drafts
                        .get(&ix)
                        .is_none_or(|d| d.text.trim().is_empty());
                free.then(|| {
                    self.tabs
                        .iter()
                        .find(|t| t.conversation == ix && !t.closing)
                })
                .flatten()
                .map(|tab| (ix, tab.id))
            })
        });
        if let Some(Some((ix, tab))) = blank {
            self.select(tab, cx);
            self.place_link_draft(ix, open.prompt);
        } else {
            let provider = agent.unwrap_or(self.conversations[active].provider);
            let model = if agent.is_some() {
                model
            } else {
                self.conversations[active].model.clone()
            };
            let call = self.backend.invoke(
                "create_session",
                json!({ "request": {
                    "projectId": self.projects[project].id,
                    "agent": provider,
                    "isolatedWorktree": false,
                    "model": model,
                } }),
            );
            self.waiting.insert(call, Waiting::LinkCreated(open.prompt));
        }
        self.explain_link(shown, cx);
    }

    /// The new conversation the link asked for exists: it opens with the prompt in its composer.
    pub(super) fn link_created(&mut self, prompt: String, value: &Value, cx: &mut Context<Self>) {
        let ix = self.add_session(value);
        self.conversations[ix].loaded = true;
        self.open(ix, cx);
        self.place_link_draft(ix, prompt);
        cx.notify();
    }

    /// Puts the prompt in the conversation's composer, caret at the end. Nothing is sent.
    fn place_link_draft(&mut self, ix: usize, prompt: String) {
        self.drafts.insert(ix, restored_draft(&prompt));
        self.touch_drafts();
    }

    /// A link with something to say (hidden characters, a missing project, a provider that did
    /// not apply) stops to say it, in the confirmation popup: the warning about hidden characters
    /// must not pass unseen.
    fn explain_link(&mut self, notes: Vec<String>, cx: &mut Context<Self>) {
        if notes.is_empty() {
            return;
        }
        self.confirm(
            t("Prompt adicionado a partir de um link").to_owned(),
            format!(
                "{}\n\n{}",
                notes.join(" "),
                t("Leia antes de enviar. Nada foi enviado.")
            ),
            t("Entendi"),
            |_| {},
            cx,
        );
    }

    /// The palette's "Copiar link do prompt": a link to the open composer's draft.
    pub(super) fn copy_prompt_link(&mut self) {
        let ix = self.active_conversation();
        let conversation = &self.conversations[ix];
        let draft = self.drafts.get(&ix).map_or("", |d| d.text.as_str());
        let project = self.project_of(ix).map(|p| self.projects[p].id.clone());
        let Some(args) = copy_args(
            project.as_deref(),
            draft,
            conversation.provider,
            conversation.model.as_deref(),
        ) else {
            self.problem =
                Some(t("Escreva um prompt no composer e depois copie o link.").to_owned());
            return;
        };
        let call = self.backend.invoke("build_prompt_link", args);
        self.waiting.insert(call, Waiting::PromptLinkCopy);
    }

    pub(super) fn prompt_link_copied(&mut self, ok: bool, value: &Value, cx: &mut Context<Self>) {
        match (ok, value.as_str()) {
            (true, Some(link)) => {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(link.to_owned()));
                crate::native_notify::banner(
                    "Sirus",
                    t(
                        "Link do prompt copiado. Quem abrir recebe o prompt como rascunho; nada é enviado.",
                    ),
                );
            }
            _ => self.problem = Some(error_text(value)),
        }
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn models() -> Vec<(String, String)> {
        vec![
            ("opus".into(), "Opus 5".into()),
            ("haiku".into(), "Haiku 5.5".into()),
        ]
    }

    #[test]
    fn only_sirus_links_are_taken() {
        let urls = vec!["https://example.com".into(), "SIRUS://new?prompt=a".into()];
        assert_eq!(link_in(&urls).as_deref(), Some("SIRUS://new?prompt=a"));
        assert_eq!(link_in(&["file:///x".to_owned(), "sir".to_owned()]), None);
    }

    #[test]
    fn the_hosts_outcome_is_read_either_way() {
        let open = json!({ "kind": "open", "prompt": "Fix it", "hiddenCharacters": 2,
            "projectId": "p1", "missingProject": null, "provider": "claude", "model": "opus" });
        assert_eq!(
            outcome_of(&open),
            Some(Outcome::Open(Open {
                prompt: "Fix it".into(),
                hidden: 2,
                project_id: Some("p1".into()),
                missing: None,
                provider: Some("claude".into()),
                model: Some("opus".into()),
            }))
        );
        assert_eq!(
            outcome_of(&json!({ "kind": "invalid", "reason": "tooLong" })),
            Some(Outcome::Invalid("tooLong".into()))
        );
        assert_eq!(outcome_of(&json!({ "kind": "open" })), None);
        assert_eq!(outcome_of(&json!({ "kind": "other" })), None);
    }

    #[test]
    fn every_refusal_has_its_own_words() {
        for reason in [
            "malformed",
            "tooLong",
            "unknownParameter",
            "repeatedParameter",
            "noPrompt",
            "hiddenOnly",
            "project",
            "repo",
            "provider",
            "model",
            "pullRequest",
        ] {
            assert_ne!(
                reason_text(reason),
                reason_text("something else"),
                "{reason}"
            );
        }
    }

    #[test]
    fn a_provider_applies_only_when_enabled_and_its_model_is_known() {
        let on = ["claude", "codex"];
        assert_eq!(
            pick_provider(None, None, &on, &[]),
            (None, None, ProviderNote::Silent)
        );
        assert_eq!(
            pick_provider(Some("cursor"), Some("x"), &on, &[]),
            (None, None, ProviderNote::Unavailable)
        );
        assert_eq!(
            pick_provider(Some("claude"), None, &on, &models()),
            (Some("claude"), None, ProviderNote::Applied)
        );
        // By id or by display name, answered with the id.
        assert_eq!(
            pick_provider(Some("claude"), Some("OPUS"), &on, &models()),
            (Some("claude"), Some("opus".into()), ProviderNote::Applied)
        );
        assert_eq!(
            pick_provider(Some("claude"), Some("haiku 5.5"), &on, &models()),
            (Some("claude"), Some("haiku".into()), ProviderNote::Applied)
        );
        assert_eq!(
            pick_provider(Some("claude"), Some("gone"), &on, &models()),
            (Some("claude"), None, ProviderNote::ModelUnknown)
        );
        // Models not loaded yet cannot refute it.
        assert_eq!(
            pick_provider(Some("claude"), Some("gone"), &on, &[]),
            (Some("claude"), Some("gone".into()), ProviderNote::Applied)
        );
    }

    #[test]
    fn a_plain_link_has_no_notes_and_a_suspicious_one_says_so() {
        let mut open = Open {
            prompt: "x".into(),
            hidden: 0,
            project_id: Some("p".into()),
            missing: None,
            provider: None,
            model: None,
        };
        assert!(notes(&open, ProviderNote::Silent).is_empty());
        open.hidden = 3;
        open.missing = Some("acme/app".into());
        let said = notes(&open, ProviderNote::Unavailable);
        assert_eq!(said.len(), 3);
        assert!(said[0].contains('3'));
        assert!(said[1].contains("acme/app"));
    }

    #[test]
    fn copying_needs_a_draft_and_trims_it() {
        assert_eq!(copy_args(Some("p"), " \n ", "claude", None), None);
        let args = copy_args(Some("p"), "  hi  ", "claude", Some("opus")).unwrap();
        assert_eq!(args["prompt"], "hi");
        assert_eq!(args["projectId"], "p");
        assert_eq!(args["provider"], "claude");
        assert_eq!(args["model"], "opus");
        assert_eq!(
            copy_args(None, "hi", "codex", None).unwrap()["projectId"],
            Value::Null
        );
    }
}
