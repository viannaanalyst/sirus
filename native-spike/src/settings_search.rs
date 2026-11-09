//! The search box of Settings: an index of every page, group and row, matching that ignores case
//! and accents in Portuguese and English, and the jump to a row with its flash.
//!
//! The index is built from the data the pages draw (`settings_pages::index_groups`, the connectors'
//! catalog) and, for Aparência, whose rows the shell draws by hand, from a recorder those rows
//! report to while the index is built (`collect`, `note_row`, `note_group`). Nothing here lists a
//! setting by name, so a new setting is found as soon as its page draws it. The two places that
//! add to the data are marked `SHELL-BUILT` below.

use gpui::{Bounds, ParentElement, Pixels, Styled, canvas};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::i18n::t;

/// What an entry stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Page,
    Group,
    Row,
}

/// One entry in a single language, before the two are joined.
pub struct Raw {
    pub kind: Kind,
    pub page: &'static str,
    pub page_label: String,
    pub section: String,
    pub group: String,
    pub title: String,
    pub about: String,
    /// The title of the row to scroll to and flash, as the page draws it.
    pub target: Option<String>,
}

/// A text in both languages, with the folded form of both for matching.
#[derive(Clone, Default)]
pub struct Text {
    pt: String,
    en: String,
    folded: String,
}

impl Text {
    fn new(pt: &str, en: &str) -> Self {
        Self {
            pt: pt.to_owned(),
            en: en.to_owned(),
            folded: format!("{}\n{}", fold(pt), fold(en)),
        }
    }

    /// In the app's language.
    pub fn shown(&self) -> &str {
        if crate::i18n::is_en() {
            &self.en
        } else {
            &self.pt
        }
    }

    fn has(&self, word: &str) -> bool {
        self.folded.contains(word)
    }
}

#[derive(Clone)]
pub struct Entry {
    pub kind: Kind,
    pub page: &'static str,
    pub section: Text,
    pub page_label: Text,
    pub group: Text,
    pub title: Text,
    pub about: Text,
    pub target: Option<Text>,
}

impl Entry {
    /// "Página › Grupo" under a result; a page shows its section, a group its page.
    pub fn subtitle(&self) -> String {
        match self.kind {
            Kind::Page => self.section.shown().to_owned(),
            Kind::Group => self.page_label.shown().to_owned(),
            Kind::Row if self.group.shown().is_empty() => self.page_label.shown().to_owned(),
            Kind::Row => format!("{} › {}", self.page_label.shown(), self.group.shown()),
        }
    }

    pub fn target(&self) -> Option<String> {
        self.target.as_ref().map(|t| t.shown().to_owned())
    }
}

/// Lowercase without accents, one character for each character in, so positions carry over.
pub fn fold(text: &str) -> String {
    text.chars().map(fold_char).collect()
}

fn fold_char(c: char) -> char {
    let c = c.to_lowercase().next().unwrap_or(c);
    match c {
        'á' | 'à' | 'â' | 'ã' | 'ä' | 'å' => 'a',
        'é' | 'è' | 'ê' | 'ë' => 'e',
        'í' | 'ì' | 'î' | 'ï' => 'i',
        'ó' | 'ò' | 'ô' | 'õ' | 'ö' => 'o',
        'ú' | 'ù' | 'û' | 'ü' => 'u',
        'ç' => 'c',
        'ñ' => 'n',
        other => other,
    }
}

fn words(query: &str) -> Vec<String> {
    fold(query).split_whitespace().map(str::to_owned).collect()
}

/// How well `entry` answers `words` (all of them must be found): each word counts 3 when it is in
/// the title, 2 in the description and 1 in the group, page or section; the entry scores the sum.
fn score(entry: &Entry, words: &[String]) -> Option<u32> {
    let mut total = 0;
    for word in words {
        let best = if entry.title.has(word) {
            3
        } else if entry.about.has(word) {
            2
        } else if entry.group.has(word) || entry.page_label.has(word) || entry.section.has(word) {
            1
        } else {
            return None;
        };
        total += best;
    }
    (!words.is_empty()).then_some(total)
}

/// The entries that answer `query`, best first and in page order among equals.
pub fn search<'a>(index: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let words = words(query);
    let mut hits: Vec<(u32, usize, &Entry)> = index
        .iter()
        .enumerate()
        .filter_map(|(at, entry)| score(entry, &words).map(|s| (s, at, entry)))
        .collect();
    hits.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    hits.into_iter().map(|(_, _, entry)| entry).collect()
}

/// `text` cut into pieces, each with whether it is what `query` matched (the first place each word
/// is found), for the accent-colored highlight.
pub fn spans(text: &str, query: &str) -> Vec<(String, bool)> {
    let folded: Vec<char> = fold(text).chars().collect();
    let chars: Vec<char> = text.chars().collect();
    if folded.len() != chars.len() {
        return vec![(text.to_owned(), false)];
    }
    let mut marked = vec![false; chars.len()];
    for word in words(query) {
        let needle: Vec<char> = word.chars().collect();
        if needle.is_empty() || needle.len() > folded.len() {
            continue;
        }
        if let Some(at) = folded.windows(needle.len()).position(|w| w == needle) {
            marked[at..at + needle.len()].fill(true);
        }
    }
    let mut out: Vec<(String, bool)> = Vec::new();
    for (c, on) in chars.into_iter().zip(marked) {
        match out.last_mut() {
            Some((piece, was)) if *was == on => piece.push(c),
            _ => out.push((c.to_string(), on)),
        }
    }
    out
}

// The index.

/// The pages' groups as (title, rows of (title, description)).
pub type Groups = Vec<(String, Vec<(String, String)>)>;

/// One choice of a control the page draws by hand (a segment, a color, a tile): found by its own
/// label under `group`, and a jump to it lands on the row or picker titled `target`.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub group: String,
    pub label: String,
    pub target: String,
}

/// What a hand-drawn page reports: its groups and the choices of its controls.
#[derive(Default)]
pub struct Built {
    pub groups: Groups,
    pub choices: Vec<Choice>,
}

/// Every page of Settings as entries in the current language: the page, each group and each row.
/// `shell_built` returns the groups of the pages the shell draws by hand (SHELL-BUILT: Aparência).
pub fn raw_entries(
    settings: &Value,
    data: &HashMap<&'static str, Value>,
    shell_built: &mut dyn FnMut(&str) -> Built,
) -> Vec<Raw> {
    let mut out = Vec::new();
    for (section, items) in crate::shell::SETTINGS_NAV {
        let section = t(section).to_owned();
        for (_, name) in items {
            let name: &'static str = name;
            let label = t(crate::shell::nav_label(name)).to_owned();
            let (mut about, mut groups) = crate::settings_pages::index_groups(name, settings, data);
            // SHELL-BUILT: rows the shell draws itself come from the recorder, ahead of the rest.
            let built = shell_built(name);
            let mut choices = built.choices;
            let mut hand_built = built.groups;
            if !hand_built.is_empty() {
                hand_built.extend(groups);
                groups = hand_built;
                about = t("Personalize materiais e movimento.").to_owned();
            }
            // SHELL-BUILT: the connectors' catalog is its own view; its rows are decorated by name.
            if name == "Conectores" {
                groups.extend(crate::settings_connectors::index_groups());
            }
            let base = |kind, title: &str, about: &str, group: &str, target: Option<String>| Raw {
                kind,
                page: name,
                page_label: label.clone(),
                section: section.clone(),
                group: group.to_owned(),
                title: title.to_owned(),
                about: about.to_owned(),
                target,
            };
            out.push(base(Kind::Page, &label, &about, &section, None));
            for (group, rows) in groups {
                let mine: Vec<Choice> = choices
                    .iter()
                    .filter(|c| !group.is_empty() && c.group == group)
                    .cloned()
                    .collect();
                choices.retain(|c| group.is_empty() || c.group != group);
                let first = rows
                    .iter()
                    .find(|(title, _)| !title.is_empty())
                    .map(|(title, _)| title.clone())
                    .or_else(|| mine.first().map(|c| c.target.clone()));
                if !group.is_empty() {
                    out.push(base(Kind::Group, &group, "", "", first));
                }
                for (title, about) in rows {
                    if title.is_empty() && about.is_empty() {
                        continue;
                    }
                    // A row with only a description (a note) is found and shown by that text.
                    let (title, about) = if title.is_empty() {
                        (about, String::new())
                    } else {
                        (title, about)
                    };
                    let target = Some(title.clone());
                    out.push(base(Kind::Row, &title, &about, &group, target));
                }
                for choice in mine {
                    let target = Some(choice.target);
                    out.push(base(Kind::Row, &choice.label, "", &group, target));
                }
            }
        }
    }
    out
}

/// The entries in both languages: the Portuguese and English builds of the pages, joined row by
/// row (the two have the same shape).
pub fn join(pt: Vec<Raw>, en: Vec<Raw>) -> Vec<Entry> {
    let same = pt.len() == en.len();
    pt.into_iter()
        .enumerate()
        .map(|(at, p)| {
            let e = if same { &en[at] } else { &p };
            let pair = |a: &str, b: &str| Text::new(a, b);
            Entry {
                kind: p.kind,
                page: p.page,
                section: pair(&p.section, &e.section),
                page_label: pair(&p.page_label, &e.page_label),
                group: pair(&p.group, &e.group),
                title: pair(&p.title, &e.title),
                about: pair(&p.about, &e.about),
                target: match (&p.target, &e.target) {
                    (Some(a), Some(b)) => Some(pair(a, b)),
                    (Some(a), None) => Some(pair(a, a)),
                    _ => None,
                },
            }
        })
        .collect()
}

/// The whole index, in both languages.
pub fn build_index(
    settings: &Value,
    data: &HashMap<&'static str, Value>,
    shell_built: &mut dyn FnMut(&str) -> Built,
) -> Vec<Entry> {
    let pt = crate::i18n::with_locale(false, || raw_entries(settings, data, &mut *shell_built));
    let en = crate::i18n::with_locale(true, || raw_entries(settings, data, &mut *shell_built));
    join(pt, en)
}

// The recorder for the rows the shell draws by hand.

#[derive(Default)]
struct Recorder {
    pending: Vec<(String, String)>,
    groups: Groups,
    choices: Vec<Choice>,
}

thread_local! {
    static RECORDER: RefCell<Option<Recorder>> = const { RefCell::new(None) };
}

/// Runs `draw` with the recorder on and returns the groups its `note_group` calls closed and the
/// choices its `note_choice` calls reported.
pub fn collect(draw: impl FnOnce()) -> Built {
    RECORDER.with(|r| *r.borrow_mut() = Some(Recorder::default()));
    draw();
    RECORDER
        .with(|r| r.borrow_mut().take())
        .map(|r| Built {
            groups: r.groups,
            choices: r.choices,
        })
        .unwrap_or_default()
}

/// A choice of a control the shell is drawing (a segment, a swatch, a tile): `label` under
/// `group`, jumping to the row or picker whose `decorate` key is `target`.
pub fn note_choice(group: &str, label: &str, target: &str) {
    RECORDER.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            rec.choices.push(Choice {
                group: group.to_owned(),
                label: label.to_owned(),
                target: target.to_owned(),
            });
        }
    });
}

/// A row the shell is drawing: its title and description.
pub fn note_row(title: &str, about: &str) {
    RECORDER.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            rec.pending.push((title.to_owned(), about.to_owned()));
        }
    });
}

/// A group the shell is drawing: takes the rows noted since the last group.
pub fn note_group(title: &str) {
    RECORDER.with(|r| {
        if let Some(rec) = r.borrow_mut().as_mut() {
            let rows = std::mem::take(&mut rec.pending);
            rec.groups.push((title.to_owned(), rows));
        }
    });
}

// The jump: scroll to a row and flash it.

/// How long the accent wash stays at full strength, and how long it takes to fade after.
const FLASH_HOLD: f32 = 0.5;
const FLASH_FADE: f32 = 0.7;
/// A row that never shows up (its data did not load) stops being waited for after this.
const GIVE_UP: Duration = Duration::from_secs(6);

struct Jump {
    title: String,
    started: Instant,
    /// When the row was scrolled into view; the flash counts from there.
    landed: Option<Instant>,
    bounds: Option<Bounds<Pixels>>,
}

thread_local! {
    static JUMP: RefCell<Option<Jump>> = const { RefCell::new(None) };
}

/// Aims at the row titled `title` on the page being opened.
pub fn aim(title: Option<String>, now: Instant) {
    JUMP.with(|j| {
        *j.borrow_mut() = title.map(|title| Jump {
            title,
            started: now,
            landed: None,
            bounds: None,
        })
    });
}

/// Waiting for the row to be drawn so it can be scrolled to; true for a while after `aim`.
pub fn waiting(now: Instant) -> bool {
    JUMP.with(|j| {
        j.borrow()
            .as_ref()
            .is_some_and(|j| j.landed.is_none() && now.duration_since(j.started) < GIVE_UP)
    })
}

/// Where the aimed row was last drawn, once.
pub fn take_bounds() -> Option<Bounds<Pixels>> {
    JUMP.with(|j| j.borrow_mut().as_mut().and_then(|j| j.bounds.take()))
}

/// The row is in view: the flash starts now.
pub fn land(now: Instant) {
    JUMP.with(|j| {
        if let Some(j) = j.borrow_mut().as_mut() {
            j.landed = Some(now);
        }
    });
}

/// The wash's strength from 1 to 0, or `None` before the flash and after it.
fn flash(now: Instant) -> Option<f32> {
    JUMP.with(|j| {
        let landed = j.borrow().as_ref()?.landed?;
        let since = now.duration_since(landed).as_secs_f32();
        let total = FLASH_HOLD + FLASH_FADE;
        (since < total).then(|| {
            if since < FLASH_HOLD {
                1.
            } else {
                1. - (since - FLASH_HOLD) / FLASH_FADE
            }
        })
    })
}

/// Whether anything is still animating: the wait for the row or the flash itself.
pub fn animating(now: Instant) -> bool {
    waiting(now) || flash(now).is_some()
}

/// Marks the row titled `title` if it is the aimed one: reports where it is drawn and washes it in
/// the accent while it flashes.
pub fn decorate<E: Styled + ParentElement + Sized>(row: E, title: &str) -> E {
    let aimed =
        !title.is_empty() && JUMP.with(|j| j.borrow().as_ref().is_some_and(|j| j.title == title));
    if !aimed {
        return row;
    }
    let wash = flash(Instant::now()).map(|strength| crate::theme::accent_wash(0.18 * strength));
    let row = row.relative();
    let row = match wash {
        Some(color) => row.bg(color),
        None => row,
    };
    row.child(
        canvas(
            |bounds, _, _| {
                JUMP.with(|j| {
                    if let Some(j) = j.borrow_mut().as_mut() {
                        j.bounds = Some(bounds);
                    }
                })
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_full(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn raw(kind: Kind, group: &str, title: &str, about: &str) -> Raw {
        Raw {
            kind,
            page: "Geral",
            page_label: "Geral".into(),
            section: "Pessoal".into(),
            group: group.into(),
            title: title.into(),
            about: about.into(),
            target: None,
        }
    }

    fn tiny() -> Vec<Entry> {
        let pt = vec![
            raw(Kind::Row, "Alertas", "Notificação sonora", "Toca um som"),
            raw(
                Kind::Row,
                "Alertas",
                "Som ao terminar",
                "Avisa com notificação",
            ),
            raw(Kind::Row, "Notificação visual", "Banner", "Mostra no canto"),
        ];
        let en = vec![
            raw(Kind::Row, "Alerts", "Sound notification", "Plays a sound"),
            raw(
                Kind::Row,
                "Alerts",
                "Sound when done",
                "Alerts with a notification",
            ),
            raw(
                Kind::Row,
                "Visual notification",
                "Banner",
                "Shows in the corner",
            ),
        ];
        join(pt, en)
    }

    #[test]
    fn folding_drops_case_and_accents() {
        assert_eq!(fold("Notificação"), "notificacao");
        assert_eq!(fold("ÁÉÍÓÚ çñ"), "aeiou cn");
        assert_eq!(fold("Avançado").chars().count(), "Avançado".chars().count());
    }

    #[test]
    fn accents_and_case_do_not_matter() {
        let index = tiny();
        assert_eq!(search(&index, "NOTIFICACAO").len(), 3);
        assert_eq!(search(&index, "notificação").len(), 3);
        assert!(search(&index, "inexistente").is_empty());
        assert!(search(&index, "   ").is_empty());
    }

    #[test]
    fn every_word_must_be_found() {
        let index = tiny();
        let hits = search(&index, "som terminar");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.pt, "Som ao terminar");
        assert!(search(&index, "som banner").is_empty());
    }

    #[test]
    fn title_beats_description_beats_group() {
        let index = tiny();
        let hits = search(&index, "notificacao");
        // Title ("Notificação sonora"), then description ("Avisa com notificação"), then group.
        assert_eq!(hits[0].title.pt, "Notificação sonora");
        assert_eq!(hits[1].title.pt, "Som ao terminar");
        assert_eq!(hits[2].title.pt, "Banner");
    }

    #[test]
    fn english_text_is_searched_too() {
        let index = tiny();
        assert_eq!(search(&index, "notification").len(), 3);
        assert_eq!(search(&index, "corner")[0].title.pt, "Banner");
    }

    #[test]
    fn the_match_is_highlighted_through_accents() {
        let pieces = spans("Notificação sonora", "notificacao");
        assert_eq!(pieces[0], ("Notificação".to_owned(), true));
        assert_eq!(pieces[1], (" sonora".to_owned(), false));
        let none = spans("Banner", "zzz");
        assert_eq!(none, vec![("Banner".to_owned(), false)]);
    }

    #[test]
    fn the_subtitle_names_the_page_and_the_group() {
        let index = tiny();
        assert_eq!(index[0].subtitle(), "Geral › Alertas");
    }

    #[test]
    fn the_recorder_groups_the_rows_noted_before_each_group() {
        note_row("ignored", "outside a collection");
        let groups = collect(|| {
            note_row("A", "a");
            note_row("B", "b");
            note_group("G1");
            note_group("G2");
        })
        .groups;
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, "G1");
        assert_eq!(groups[0].1.len(), 2);
        assert!(groups[1].1.is_empty());
    }

    /// Every page of the nav is in the index, with something besides the page itself, in both
    /// languages; the data-driven pages come from the same builders the pages draw.
    #[test]
    fn every_page_of_the_nav_is_searchable() {
        let settings = json!({});
        let data = HashMap::new();
        let index = build_index(&settings, &data, &mut |_| Built::default());
        for (_, items) in crate::shell::SETTINGS_NAV {
            for (_, name) in items {
                let page: Vec<&Entry> = index.iter().filter(|e| e.page == *name).collect();
                assert!(
                    page.iter().any(|e| e.kind == Kind::Page),
                    "{name} has no page entry"
                );
                assert!(
                    page.iter().any(|e| e.kind != Kind::Page),
                    "{name} has no rows in the index: add its data to the index"
                );
            }
        }
        // Spot checks across the kinds of page.
        for query in [
            "atalhos",
            "provedores",
            "notification",
            "supabase",
            "reduzir movimento",
            "worktrees",
        ] {
            assert!(!search(&index, query).is_empty(), "{query}");
        }
    }

    #[test]
    fn every_keyboard_command_is_searchable() {
        let index = build_index(&json!({}), &HashMap::new(), &mut |_| Built::default());
        let atalhos: Vec<&Entry> = index.iter().filter(|e| e.page == "Atalhos").collect();
        assert!(atalhos.len() > crate::settings_pages::keybinding_count());
    }

    /// The index as the shell builds it for Aparência: the recorder fed by the real list of
    /// choices, so the labels searched are the ones the page draws.
    fn appearance_index() -> Vec<Entry> {
        build_index(&json!({}), &HashMap::new(), &mut |page| {
            if page == crate::shell::APPEARANCE {
                collect(|| {
                    // The page's groups, as it draws them; the choices are filed under these.
                    for group in ["Tema e cores", "Materiais", "Interface", "Ícone do app"] {
                        note_group(t(group));
                    }
                    crate::shell::note_appearance_choices();
                })
            } else {
                Built::default()
            }
        })
    }

    #[test]
    fn a_theme_mode_is_found_and_jumps_to_the_picker() {
        let index = appearance_index();
        let hits = search(&index, "escuro");
        let hit = hits
            .iter()
            .find(|e| e.title.pt == "Escuro" && e.group.pt == "Tema e cores")
            .expect("the theme option is indexed");
        assert_eq!(hit.page, crate::shell::APPEARANCE);
        assert_eq!(hit.target().as_deref(), Some("Tema"));
        assert!(
            search(&index, "dark")
                .iter()
                .any(|e| e.title.pt == "Escuro" && e.group.pt == "Tema e cores")
        );
        // The group itself jumps to its picker too.
        let group = index
            .iter()
            .find(|e| e.kind == Kind::Group && e.title.pt == "Tema e cores")
            .unwrap();
        assert_eq!(group.target().as_deref(), Some("Tema"));
    }

    #[test]
    fn an_app_icon_is_found_with_or_without_the_accent() {
        let index = appearance_index();
        for query in ["mármore", "marmore", "marble"] {
            let hit = search(&index, query)
                .into_iter()
                .find(|e| e.title.pt == "Mármore")
                .unwrap_or_else(|| panic!("{query}"));
            assert!(hit.target().is_some());
            assert_eq!(hit.group.pt, "Ícone do app");
        }
        for icon in crate::app_icon::AppIcon::ALL {
            let label = icon.label();
            assert!(
                index
                    .iter()
                    .any(|e| e.title.pt == label && e.target().is_some()),
                "{label}"
            );
        }
    }

    #[test]
    fn every_choice_jumps_to_something_the_page_decorates() {
        let index = appearance_index();
        let choices = collect(crate::shell::note_appearance_choices).choices;
        assert!(choices.len() > 20);
        for choice in choices {
            assert!(
                index.iter().any(|e| e.title.pt == choice.label
                    && e.target().as_deref() == Some(choice.target.as_str())),
                "{} is not indexed",
                choice.label
            );
        }
    }

    #[test]
    fn every_connector_entry_has_a_target_the_row_answers_to() {
        let index = build_index(&json!({}), &HashMap::new(), &mut |_| Built::default());
        let page: Vec<&Entry> = index
            .iter()
            .filter(|e| e.page == "Conectores" && e.kind != Kind::Page)
            .collect();
        assert!(page.len() > 10);
        for entry in &page {
            assert!(entry.target().is_some(), "{}", entry.title.pt);
        }
        // A connector's row is decorated with its catalog key, which is what the entry targets.
        for key in crate::settings_connectors::catalog_keys() {
            assert!(
                page.iter()
                    .any(|e| e.kind == Kind::Row && e.target().as_deref() == Some(key)),
                "{key}"
            );
        }
    }
}
