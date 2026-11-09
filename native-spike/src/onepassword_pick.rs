//! "1Password" on a secret card: lists the person's items, then the chosen item's fields, and the
//! host reads the picked field and answers the request with it (onepassword.rs). Only titles and
//! field names reach the window.

use super::*;

pub(crate) enum Stage {
    Loading,
    Items(Vec<(String, String, String)>),
    Fields(String, Vec<(String, String)>),
    Failed(String),
}

pub(crate) struct OpPick {
    pub request: String,
    pub stage: Stage,
}

fn reply_error(error: &Value) -> String {
    crate::i18n::from_host(
        error
            .as_str()
            .or(error["message"].as_str())
            .unwrap_or("1Password refused the request."),
    )
}

/// The items worth showing first: those whose title shares a word with what is asked.
pub(crate) fn ranked(
    mut items: Vec<(String, String, String)>,
    asked: &str,
) -> Vec<(String, String, String)> {
    let words: Vec<String> = asked
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .map(str::to_lowercase)
        .collect();
    let score = |title: &str| {
        let title = title.to_lowercase();
        words.iter().filter(|w| title.contains(w.as_str())).count()
    };
    items.sort_by_key(|(_, title, _)| std::cmp::Reverse(score(title)));
    items
}

impl Shell {
    pub(super) fn start_op_pick(&mut self, request: String, asked: String, cx: &mut Context<Self>) {
        self.op_pick = Some(OpPick {
            request: request.clone(),
            stage: Stage::Loading,
        });
        let rx = self.backend.request(
            "onepassword_action",
            json!({ "action": { "type": "search", "query": "" } }),
        );
        cx.spawn(async move |this, cx| {
            let reply = rx.recv().await;
            let _ = this.update(cx, |this, cx| {
                let Some(pick) = this.op_pick.as_mut().filter(|p| p.request == request) else {
                    return;
                };
                pick.stage = match reply {
                    Ok(Ok(list)) => {
                        let items: Vec<(String, String, String)> = list
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|i| {
                                Some((
                                    i["id"].as_str()?.to_owned(),
                                    i["title"].as_str()?.to_owned(),
                                    i["vault"].as_str().unwrap_or("").to_owned(),
                                ))
                            })
                            .collect();
                        if items.is_empty() {
                            Stage::Failed(t("Nenhum item no 1Password.").to_owned())
                        } else {
                            Stage::Items(ranked(items, &asked))
                        }
                    }
                    Ok(Err(error)) => Stage::Failed(reply_error(&error)),
                    Err(_) => Stage::Failed(t("O motor do Sirus não respondeu.").to_owned()),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn op_fields(&mut self, id: String, title: String, cx: &mut Context<Self>) {
        let Some(pick) = self.op_pick.as_mut() else {
            return;
        };
        pick.stage = Stage::Loading;
        let request = pick.request.clone();
        let rx = self.backend.request(
            "onepassword_action",
            json!({ "action": { "type": "fields", "id": id } }),
        );
        cx.spawn(async move |this, cx| {
            let reply = rx.recv().await;
            let _ = this.update(cx, |this, cx| {
                let Some(pick) = this.op_pick.as_mut().filter(|p| p.request == request) else {
                    return;
                };
                pick.stage = match reply {
                    Ok(Ok(list)) => Stage::Fields(
                        title,
                        list.as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|f| {
                                Some((
                                    f["label"].as_str()?.to_owned(),
                                    f["reference"].as_str()?.to_owned(),
                                ))
                            })
                            .collect(),
                    ),
                    Ok(Err(error)) => Stage::Failed(reply_error(&error)),
                    Err(_) => Stage::Failed(t("O motor do Sirus não respondeu.").to_owned()),
                };
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn op_answer(&mut self, reference: String, cx: &mut Context<Self>) {
        let Some(pick) = self.op_pick.as_mut() else {
            return;
        };
        pick.stage = Stage::Loading;
        let request = pick.request.clone();
        let rx = self.backend.request(
            "onepassword_action",
            json!({ "action": { "type": "answer", "requestId": request, "reference": reference } }),
        );
        cx.spawn(async move |this, cx| {
            let reply = rx.recv().await;
            let _ = this.update(cx, |this, cx| {
                match reply {
                    Ok(Ok(_)) => {
                        this.op_pick = None;
                        this.secrets
                            .retain(|r| r["id"].as_str() != Some(request.as_str()));
                    }
                    Ok(Err(error)) => {
                        if let Some(pick) = this.op_pick.as_mut() {
                            pick.stage = Stage::Failed(reply_error(&error));
                        }
                    }
                    Err(_) => this.op_pick = None,
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// The list over the secret card while 1Password is being picked from.
    pub(super) fn op_pick_view(&self, request: &str, cx: &mut Context<Self>) -> Option<AnyElement> {
        let pick = self.op_pick.as_ref().filter(|p| p.request == request)?;
        let line = |text: String, alpha: f32| {
            div()
                .px(px(8.))
                .py(px(6.))
                .text_size(px(12.5))
                .text_color(content(alpha))
                .child(text)
        };
        let row = |id: (&'static str, usize), title: String, about: String| {
            div()
                .id(id)
                .px(px(8.))
                .py(px(5.))
                .rounded(px(crate::theme::RADIUS_MD))
                .flex()
                .items_center()
                .gap(px(8.))
                .cursor_pointer()
                .hover(|s| s.bg(content(0.08)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .text_color(content(0.92))
                        .child(title),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .text_size(px(11.5))
                        .text_color(content(0.45))
                        .child(about),
                )
        };
        let body: Vec<AnyElement> =
            match &pick.stage {
                Stage::Loading => {
                    vec![line(t("Abrindo o 1Password…").to_owned(), 0.6).into_any_element()]
                }
                Stage::Failed(problem) => vec![line(problem.clone(), 0.75).into_any_element()],
                Stage::Items(items) => items
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(at, (id, title, vault))| {
                        let chosen = title.clone();
                        row(("op-item", at), title, vault)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.op_fields(id.clone(), chosen.clone(), cx)
                            }))
                            .into_any_element()
                    })
                    .collect(),
                Stage::Fields(title, fields) => {
                    let mut rows = vec![line(title.clone(), 0.5).into_any_element()];
                    rows.extend(fields.iter().cloned().enumerate().map(
                        |(at, (label, reference))| {
                            row(("op-field", at), label, String::new())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.op_answer(reference.clone(), cx)
                                }))
                                .into_any_element()
                        },
                    ));
                    rows
                }
            };
        Some(
            div()
                .mb(px(6.))
                .p(px(4.))
                .rounded(px(crate::theme::RADIUS_LG))
                .border_1()
                .border_color(crate::theme::border())
                .bg(field_fill())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .px(px(8.))
                        .h(px(26.))
                        .text_size(px(11.5))
                        .text_color(content(0.5))
                        .child(div().flex_1().child("1Password"))
                        .child(
                            div()
                                .id("op-close")
                                .cursor_pointer()
                                .hover(|s| s.text_color(content(0.9)))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.op_pick = None;
                                    cx.notify();
                                }))
                                .child(t("Fechar")),
                        ),
                )
                .child(
                    div()
                        .id("op-list")
                        .max_h(px(220.))
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .children(body),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn items_sharing_a_word_with_the_request_come_first() {
        let items = vec![
            ("1".into(), "Banco".into(), String::new()),
            ("2".into(), "GitHub token".into(), String::new()),
        ];
        assert_eq!(ranked(items, "GITHUB_TOKEN")[0].0, "2");
    }
}
