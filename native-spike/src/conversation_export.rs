//! Saving a conversation as Markdown from the conversation's context menu. The text is what the
//! composer's `/export` writes (`conversationMarkdown` in lib/composer-commands.ts), and the host
//! shows the save dialog (`export_conversation`), so the file choice is Sirus Code's own.
//!
//! This module is a child of `shell` (`#[path]`), so it reaches the window's private state.

use super::*;

/// The Markdown copy of a conversation: its title, the provider and model with the date, then each
/// message under its speaker. System messages and empty ones are left out.
pub(crate) fn markdown(
    title: &str,
    provider: &str,
    model: Option<&str>,
    created_at: &str,
    messages: &[Value],
    you: &str,
    agent: &str,
) -> String {
    let date: String = created_at.chars().take(10).collect();
    let mut head = provider.to_owned();
    if let Some(model) = model.filter(|model| !model.is_empty()) {
        head.push_str(" · ");
        head.push_str(model);
    }
    head.push_str(" · ");
    head.push_str(&date);
    let mut lines = vec![format!("# {title}"), String::new(), head];
    for message in messages {
        let role = message["role"].as_str().unwrap_or("");
        let content = message["content"].as_str().unwrap_or("").trim();
        if role == "system" || content.is_empty() {
            continue;
        }
        let speaker = if role == "user" { you } else { agent };
        lines.push(String::new());
        lines.push(format!("## {speaker}"));
        lines.push(String::new());
        lines.push(content.to_owned());
    }
    format!("{}\n", lines.join("\n"))
}

/// The `transcript_action` call that reads a whole conversation (no window, no lite copy).
pub(crate) fn transcript_args(session_id: &str) -> Value {
    json!({ "action": { "type": "load", "sessionId": session_id } })
}

/// The `export_conversation` call's arguments.
pub(crate) fn export_args(session_id: &str, markdown: &str) -> Value {
    json!({ "sessionId": session_id, "markdown": markdown })
}

impl Shell {
    /// Reads the whole conversation, then hands its Markdown to the host, which asks where to save
    /// it. A cancelled dialog saves nothing and shows nothing; a refusal shows in the footer.
    pub(super) fn export_conversation(&mut self, ix: usize, cx: &mut Context<Self>) {
        let conversation = &self.conversations[ix];
        if conversation.session_id.is_empty() {
            return;
        }
        let session = conversation.session_id.clone();
        let title = conversation.title.clone();
        let provider = conversation.provider;
        let model = conversation.model.clone();
        let created = conversation.created_at.clone();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let transcript = super::message_actions::settle(
                backend.request("transcript_action", transcript_args(&session)),
            )
            .await;
            let outcome = match transcript {
                Ok(value) => {
                    let messages = value["messages"].as_array().cloned().unwrap_or_default();
                    let markdown = markdown(
                        &title,
                        provider,
                        model.as_deref(),
                        &created,
                        &messages,
                        t("Você"),
                        t("Agente"),
                    );
                    super::message_actions::settle(
                        backend.request("export_conversation", export_args(&session, &markdown)),
                    )
                    .await
                    .map(|_| ())
                }
                Err(problem) => Err(problem),
            };
            let _ = this.update(cx, |this, cx| {
                if let Err(problem) = outcome {
                    this.problem = Some(problem);
                }
                cx.notify();
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_markdown_has_the_title_the_head_and_each_speaker() {
        let messages = [
            json!({ "role": "user", "content": "  Oi  " }),
            json!({ "role": "system", "content": "ignored" }),
            json!({ "role": "agent", "content": "" }),
            json!({ "role": "agent", "content": "Olá." }),
        ];
        let text = markdown(
            "Plano",
            "codex",
            Some("gpt-5"),
            "2026-11-05T10:00:00Z",
            &messages,
            "You",
            "Agent",
        );
        assert_eq!(
            text,
            "# Plano\n\ncodex · gpt-5 · 2026-11-05\n\n## You\n\nOi\n\n## Agent\n\nOlá.\n"
        );
    }

    #[test]
    fn a_model_less_conversation_has_no_model_part() {
        let text = markdown("T", "claude", None, "2026-01-02", &[], "Você", "Agente");
        assert_eq!(text, "# T\n\nclaude · 2026-01-02\n");
        assert_eq!(
            markdown("T", "claude", Some(""), "2026-01-02", &[], "Você", "Agente"),
            text
        );
    }

    #[test]
    fn the_argument_shapes_match_the_host() {
        assert_eq!(
            transcript_args("s"),
            json!({ "action": { "type": "load", "sessionId": "s" } })
        );
        assert_eq!(
            export_args("s", "# x\n"),
            json!({ "sessionId": "s", "markdown": "# x\n" })
        );
    }
}
