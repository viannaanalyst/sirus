//! The issue or pull request a conversation's first message names, as a chip in the composer's
//! bar: number, title and state, opening on GitHub. The host reads it with `gh` (issue_ref.rs);
//! each conversation asks once.

use super::*;

/// The first github.com issue or pull URL in the text, else its first `#123`.
pub(crate) fn reference(text: &str) -> Option<String> {
    for word in text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | '<' | '>')) {
        let bare = word
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        if let Some(rest) = bare.strip_prefix("github.com/") {
            let parts: Vec<&str> = rest.split('/').collect();
            if parts.len() >= 4
                && matches!(parts[2], "issues" | "pull")
                && parts[3]
                    .split(['#', '?'])
                    .next()
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            {
                return Some(word.trim_end_matches(['.', ',', ';', ':']).to_owned());
            }
        }
    }
    text.split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ',' | '.' | ';' | ':'))
        .find(|word| {
            word.strip_prefix('#').is_some_and(|n| {
                (1..=8).contains(&n.len()) && n.bytes().all(|b| b.is_ascii_digit())
            })
        })
        .map(str::to_owned)
}

impl Shell {
    pub(super) fn issue_chip(&self, ix: usize, cx: &mut Context<Self>) -> Option<AnyElement> {
        let conversation = &self.conversations[ix];
        if conversation.session_id.is_empty() || conversation.base != 0 {
            return None;
        }
        let session = conversation.session_id.clone();
        let known = self.issue_refs.borrow().get(&session).cloned();
        let item = match known {
            Some(item) => item,
            None => {
                let first = conversation.messages.iter().find(|m| m["role"] == "user")?;
                let text = first["content"].as_str()?;
                let found = reference(text);
                self.issue_refs
                    .borrow_mut()
                    .insert(session.clone(), Value::Null);
                if let Some(found) = found {
                    let rx = self.backend.request(
                        "issue_reference",
                        json!({ "sessionId": session, "reference": found }),
                    );
                    let refs = self.issue_refs.clone();
                    cx.spawn(async move |this, cx| {
                        if let Ok(Ok(item)) = rx.recv().await
                            && !item.is_null()
                        {
                            refs.borrow_mut().insert(session, item);
                            let _ = this.update(cx, |_, cx| cx.notify());
                        }
                    })
                    .detach();
                }
                return None;
            }
        };
        let number = item["number"].as_u64()?;
        let title = item["title"].as_str()?.to_owned();
        let url = item["url"].as_str()?.to_owned();
        let pull = item["kind"] == "pull";
        let (color, state) = match item["state"].as_str()? {
            "open" => (rgb(0x3fb950), t("Aberta")),
            "draft" => (rgb(0x8b949e), t("Rascunho")),
            "merged" => (rgb(0xa371f7), t("Mergeado")),
            _ => (rgb(0xf85149), t("Fechada")),
        };
        let glyph = match (pull, item["state"].as_str()) {
            (true, Some("merged")) => "git-merge",
            (true, _) => "git-pull-request",
            _ => "circle-dot",
        };
        let tip = format!(
            "{} #{number} · {state}\n{title}",
            if pull { t("Pull request") } else { t("Issue") }
        );
        Some(
            div()
                .id("issue-chip")
                .h(px(26.))
                .px(px(8.))
                .flex_shrink(1.)
                .min_w_0()
                .max_w(px(220.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(12.))
                .text_color(content(0.75))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.07)).text_color(content(0.95)))
                .tooltip(message_actions::text_tip(tip))
                .on_click(move |_, _, cx| cx.open_url(&url))
                .child(icon(glyph, 13.).text_color(color))
                .child(div().flex_shrink_0().child(format!("#{number}")))
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_color(content(0.55))
                        .child(title),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_url_wins_over_a_number() {
        assert_eq!(
            reference("veja #3 e https://github.com/o/r/pull/12/files."),
            Some("https://github.com/o/r/pull/12/files".into())
        );
        assert_eq!(
            reference("corrige a issue #45, por favor"),
            Some("#45".into())
        );
        assert_eq!(reference("cor #fff e #a1"), None);
        assert_eq!(reference("github.com/o/r/tree/main"), None);
        assert_eq!(reference("sem nada"), None);
    }
}
