//! Guest links in the window (ADR-210): the card that makes a read-only link to a conversation
//! (the share icon in the title bar) or to a snapshot of the page in the embedded browser (the
//! browser's "⋯" menu), and shows the link once it exists. The host keeps the links and serves
//! them (`share_link_action`, `src-tauri/src/share_links.rs`); this is only the card and what it
//! sends and reads. `body` draws the inside, and the shell and the browser panel each put it in
//! their own popover and feed `Event`s back.

use crate::i18n::{t, tf};
use crate::theme::{self, content};
use chrono::Local;
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, Window, div, prelude::*, px, rgb};
use serde_json::{Value, json};
use std::rc::Rc;

/// What the person can pick for how long a link lives: the host's name, and the label.
pub const VALIDITIES: [(&str, &str); 3] =
    [("hour", "1 hora"), ("day", "24 horas"), ("week", "7 dias")];

/// The most a captured page may weigh (the host takes a little more).
pub const MAX_PREVIEW: usize = 15 * 1024 * 1024;

#[derive(Clone, PartialEq, Debug)]
pub enum Target {
    /// A conversation, by session id.
    Conversation(String),
    /// The page open in the embedded browser.
    Preview,
}

/// A link just made.
#[derive(Clone, PartialEq, Debug)]
pub struct Done {
    pub id: String,
    pub url: String,
    pub expires_at: String,
    pub views: u64,
}

#[derive(Clone, Debug)]
pub struct Share {
    pub target: Target,
    pub validity: &'static str,
    /// Conversations only: leave command output and diffs out.
    pub hide_output: bool,
    /// Conversations only: show attachments and images.
    pub show_attachments: bool,
    pub working: bool,
    pub error: Option<String>,
    pub done: Option<Done>,
}

impl Share {
    pub fn new(target: Target) -> Self {
        Self {
            target,
            validity: "day",
            hide_output: true,
            show_attachments: false,
            working: false,
            error: None,
            done: None,
        }
    }

    /// The `share_link_action` arguments for a conversation.
    pub fn conversation_args(&self) -> Option<Value> {
        let Target::Conversation(session) = &self.target else {
            return None;
        };
        Some(json!({ "action": {
            "type": "conversation",
            "sessionId": session,
            "validity": self.validity,
            "hideOutput": self.hide_output,
            "showAttachments": self.show_attachments,
        }}))
    }

    /// The `share_link_action` arguments for a captured page.
    pub fn preview_args(&self, title: &str, html: &str) -> Value {
        json!({ "action": {
            "type": "preview",
            "title": title,
            "html": html,
            "validity": self.validity,
        }})
    }
}

/// The link a `share_link_action` answer says was just made.
pub fn parse_created(value: &Value) -> Option<Done> {
    let created = &value["created"];
    Some(Done {
        id: created["id"].as_str()?.to_owned(),
        url: created["url"].as_str()?.to_owned(),
        expires_at: created["expiresAt"].as_str()?.to_owned(),
        views: created["views"].as_u64().unwrap_or(0),
    })
}

/// When a link expires, in the person's time: "12/11/2026 08:42".
pub fn when(stamp: &str) -> String {
    chrono::DateTime::parse_from_rfc3339(stamp).map_or_else(
        |_| stamp.to_owned(),
        |at| {
            at.with_timezone(&Local)
                .format(t("%d/%m/%Y %H:%M"))
                .to_string()
        },
    )
}

/// "1 visualização", "3 visualizações".
pub fn views_text(views: u64) -> String {
    if views == 1 {
        t("1 visualização").to_owned()
    } else {
        tf("{} visualizações", &[&views])
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    Validity(&'static str),
    Hide,
    Attachments,
    Cancel,
    Create,
    Copy,
    Revoke,
    Close,
}

pub type Handler<T> = Rc<dyn Fn(&mut T, Event, &mut Window, &mut Context<T>)>;

fn button<T: 'static>(
    id: &'static str,
    label: &'static str,
    kind: Kind,
    enabled: bool,
    event: Event,
    cx: &mut Context<T>,
    on: &Handler<T>,
) -> AnyElement {
    let (fill, ink, edge) = match kind {
        Kind::Primary => {
            let (fill, ink) = theme::primary_action(enabled);
            (Some(fill), ink, gpui::transparent_black())
        }
        Kind::Secondary => (None, content(0.8), theme::border()),
        Kind::Danger => (
            None,
            rgb(theme::DIFF_DEL).into(),
            gpui::Hsla {
                a: 0.35,
                ..rgb(theme::DIFF_DEL).into()
            },
        ),
    };
    let on = on.clone();
    div()
        .id(id)
        .h(px(28.))
        .px(px(12.))
        .rounded(px(theme::RADIUS_SM))
        .border_1()
        .border_color(edge)
        .flex()
        .items_center()
        .text_size(px(12.5))
        .text_color(ink)
        .when(kind == Kind::Primary, |d| {
            d.font_weight(FontWeight::SEMIBOLD)
        })
        .when_some(fill, |d, fill| d.bg(fill))
        .when(enabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(content(0.07)))
                .on_click(cx.listener(move |this, _, window, cx| on(this, event, window, cx)))
        })
        .child(t(label))
        .into_any_element()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Primary,
    Secondary,
    Danger,
}

fn check_row<T: 'static>(
    id: &'static str,
    label: &'static str,
    on: bool,
    event: Event,
    cx: &mut Context<T>,
    handler: &Handler<T>,
) -> AnyElement {
    let handler = handler.clone();
    div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(8.))
        .py(px(3.))
        .text_size(px(12.))
        .text_color(content(0.65))
        .cursor_pointer()
        .on_click(cx.listener(move |this, _, window, cx| handler(this, event, window, cx)))
        .child(
            div()
                .size(px(14.))
                .flex_shrink_0()
                .rounded(px(4.))
                .border_1()
                .border_color(if on { theme::accent() } else { content(0.3) })
                .flex()
                .items_center()
                .justify_center()
                .when(on, |d| {
                    d.bg(theme::accent())
                        .child(theme::icon("check", 10.).text_color(theme::on_accent()))
                }),
        )
        .child(t(label))
        .into_any_element()
}

fn validity_row<T: 'static>(
    key: &'static str,
    share: &Share,
    cx: &mut Context<T>,
    handler: &Handler<T>,
) -> AnyElement {
    let chosen = VALIDITIES
        .iter()
        .position(|(name, _)| *name == share.validity)
        .unwrap_or(1);
    let items: Vec<AnyElement> = VALIDITIES
        .iter()
        .enumerate()
        .map(|(index, (name, label))| {
            let (name, label, handler) = (*name, *label, handler.clone());
            div()
                .id((key, index))
                .relative()
                .flex_1()
                .child(crate::segment_pill::probe(key, index))
                .h(px(26.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(theme::RADIUS_SM))
                .text_size(px(12.))
                .text_color(if index == chosen {
                    theme::accent()
                } else {
                    content(0.55)
                })
                .when(index != chosen, |d| d.hover(|s| s.text_color(content(0.9))))
                .cursor_pointer()
                .on_click(cx.listener(move |this, _, window, cx| {
                    handler(this, Event::Validity(name), window, cx)
                }))
                .child(t(label))
                .into_any_element()
        })
        .collect();
    div()
        .relative()
        .flex()
        .p(px(2.))
        .rounded(px(theme::RADIUS_MD))
        .border_1()
        .border_color(theme::border())
        .child(crate::segment_pill::pill(
            key,
            chosen,
            theme::accent_wash(0.18),
            theme::RADIUS_SM,
        ))
        .children(items)
        .into_any_element()
}

/// The card's content. `key` names the validity control's sliding thumb (one per place that
/// shows the card).
pub fn body<T: 'static>(
    share: &Share,
    key: &'static str,
    cx: &mut Context<T>,
    on: Handler<T>,
) -> Div {
    let conversation = matches!(share.target, Target::Conversation(_));
    let heading = |title: &'static str, text: &'static str| {
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(content(0.95))
                    .child(t(title)),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child(t(text)),
            )
    };
    let column = div().flex().flex_col().gap(px(10.));
    if let Some(done) = &share.done {
        let url = SharedString::from(done.url.clone());
        let handler = on.clone();
        return column
            .child(heading(
                "Link criado",
                "Copiado para a área de transferência.",
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(9.))
                    .h(px(32.))
                    .rounded(px(theme::RADIUS_MD))
                    .border_1()
                    .border_color(theme::border())
                    .bg(theme::field_fill())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_family("Menlo")
                            .text_size(px(11.5))
                            .text_color(content(0.9))
                            .child(url),
                    )
                    .child(
                        div()
                            .id("share-copy")
                            .flex_shrink_0()
                            .px(px(4.))
                            .text_size(px(12.))
                            .text_color(theme::accent())
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                handler(this, Event::Copy, window, cx)
                            }))
                            .child(t("Copiar")),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .text_size(px(11.5))
                    .text_color(content(0.4))
                    .child(div().size(px(6.)).rounded_full().bg(rgb(theme::DIFF_ADD)))
                    .child(tf("Ativo · vence {}", &[&when(&done.expires_at)]))
                    .child(
                        div()
                            .ml_auto()
                            .child(format!("· {}", views_text(done.views))),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .pt(px(2.))
                    .child(button(
                        "share-revoke",
                        "Desligar link",
                        Kind::Danger,
                        true,
                        Event::Revoke,
                        cx,
                        &on,
                    ))
                    .child(div().flex_1())
                    .child(button(
                        "share-done",
                        "Pronto",
                        Kind::Primary,
                        true,
                        Event::Close,
                        cx,
                        &on,
                    )),
            );
    }
    let head = if conversation {
        heading(
            "Compartilhar conversa",
            "Quem tiver o link vê a conversa, sem poder escrever.",
        )
    } else {
        heading(
            "Compartilhar prévia",
            "Quem tiver o link vê esta página como ela está agora, sem poder mexer nela.",
        )
    };
    let mut card = column.child(head).child(
        div()
            .flex()
            .flex_col()
            .gap(px(5.))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(content(0.55))
                    .child(t("Vale por")),
            )
            .child(validity_row(key, share, cx, &on)),
    );
    if conversation {
        card = card
            .child(check_row(
                "share-hide",
                "Esconder saída dos comandos e diffs",
                share.hide_output,
                Event::Hide,
                cx,
                &on,
            ))
            .child(check_row(
                "share-attachments",
                "Mostrar anexos e imagens",
                share.show_attachments,
                Event::Attachments,
                cx,
                &on,
            ));
    }
    if share.working {
        card = card.child(div().text_size(px(12.)).text_color(content(0.55)).child(t(
            if conversation {
                "Criando o link…"
            } else {
                "Capturando a página…"
            },
        )));
    }
    if let Some(error) = &share.error {
        card = card.child(
            div()
                .text_size(px(12.))
                .text_color(rgb(theme::DIFF_DEL))
                .child(SharedString::from(error.clone())),
        );
    }
    card.child(
        div()
            .flex()
            .items_center()
            .justify_end()
            .gap(px(6.))
            .child(button(
                "share-cancel",
                "Cancelar",
                Kind::Secondary,
                true,
                Event::Cancel,
                cx,
                &on,
            ))
            .child(button(
                "share-create",
                "Criar link",
                Kind::Primary,
                !share.working,
                Event::Create,
                cx,
                &on,
            )),
    )
}

/// What the capture script answered: the page's title and HTML, or why not.
pub fn parse_capture(raw: &str) -> Result<(String, String), String> {
    let value: Value = serde_json::from_str(raw)
        .map_err(|_| t("Não foi possível capturar a página.").to_owned())?;
    if let Some(error) = value["error"].as_str() {
        return Err(if error == "large" {
            t("A página é grande demais para compartilhar.").to_owned()
        } else {
            t("Não foi possível capturar a página.").to_owned()
        });
    }
    let html = value["html"].as_str().filter(|html| !html.is_empty());
    let Some(html) = html else {
        return Err(t("Não foi possível capturar a página.").to_owned());
    };
    if html.len() > MAX_PREVIEW {
        return Err(t("A página é grande demais para compartilhar.").to_owned());
    }
    Ok((
        value["title"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned(),
        html.to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_link_asks_for_what_the_card_shows() {
        let mut share = Share::new(Target::Conversation("s-1".into()));
        let args = share.conversation_args().unwrap();
        assert_eq!(args["action"]["type"], "conversation");
        assert_eq!(args["action"]["sessionId"], "s-1");
        assert_eq!(args["action"]["validity"], "day");
        assert_eq!(args["action"]["hideOutput"], true);
        assert_eq!(args["action"]["showAttachments"], false);
        share.validity = "week";
        share.hide_output = false;
        share.show_attachments = true;
        let args = share.conversation_args().unwrap();
        assert_eq!(args["action"]["validity"], "week");
        assert_eq!(args["action"]["hideOutput"], false);
        assert_eq!(args["action"]["showAttachments"], true);
        assert!(Share::new(Target::Preview).conversation_args().is_none());
    }

    #[test]
    fn a_preview_link_carries_the_page() {
        let share = Share::new(Target::Preview);
        let args = share.preview_args("My page", "<p>x</p>");
        assert_eq!(args["action"]["type"], "preview");
        assert_eq!(args["action"]["title"], "My page");
        assert_eq!(args["action"]["html"], "<p>x</p>");
        assert_eq!(args["action"]["validity"], "day");
    }

    #[test]
    fn the_made_link_is_read_from_the_answer() {
        let answer = json!({ "links": [], "created": {
            "id": "i", "url": "https://h/s/tok", "expiresAt": "2026-11-12T10:00:00Z", "views": 0
        }});
        let done = parse_created(&answer).unwrap();
        assert_eq!(done.url, "https://h/s/tok");
        assert_eq!(done.id, "i");
        assert!(parse_created(&json!({ "links": [] })).is_none());
    }

    #[test]
    fn views_read_in_the_singular_and_plural() {
        crate::i18n::with_locale(false, || {
            assert_eq!(views_text(1), "1 visualização");
            assert_eq!(views_text(3), "3 visualizações");
        });
    }

    #[test]
    fn a_capture_is_a_title_and_a_page_or_a_reason() {
        crate::i18n::with_locale(false, || {
            let ok = parse_capture(r#"{"title":" T ","html":"<p>x</p>"}"#).unwrap();
            assert_eq!(ok, ("T".to_owned(), "<p>x</p>".to_owned()));
            assert_eq!(
                parse_capture(r#"{"error":"large"}"#).unwrap_err(),
                "A página é grande demais para compartilhar."
            );
            assert!(parse_capture(r#"{"error":"boom"}"#).is_err());
            assert!(parse_capture("nope").is_err());
            assert!(parse_capture(r#"{"title":"x","html":""}"#).is_err());
        });
    }
}
