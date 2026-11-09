//! What the top bar's Git split button offers (ADR-198). Pure choices only: the quick action for
//! a Git state, why a menu item is off, what the footer says, and the label while it runs.
//! git_button.rs draws it and runs the work.
use crate::i18n::{t, tf};

/// What the working folder looks like to Git.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GitFacts {
    /// Commits the branch has that its upstream lacks.
    pub ahead: u64,
    /// Commits the upstream has that the branch lacks.
    pub behind: u64,
    /// Staged, unstaged or untracked changes in the working tree.
    pub dirty: bool,
}

/// The left half of the button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Quick {
    Pull,
    Push,
    CommitPush,
    Sync,
    /// Nothing to do: a cloud with no label, which opens the menu.
    UpToDate,
}

impl Quick {
    pub fn icon(self) -> &'static str {
        match self {
            Quick::Pull => "cloud-download",
            Quick::Push => "cloud-upload",
            Quick::CommitPush => "commit",
            Quick::Sync => "cloud-sync",
            Quick::UpToDate => "cloud",
        }
    }

    pub fn label(self) -> Option<&'static str> {
        match self {
            Quick::Pull => Some(t("Puxar")),
            Quick::Push => Some(t("Enviar")),
            Quick::CommitPush => Some(t("Commit e enviar")),
            Quick::Sync => Some(t("Sincronizar")),
            Quick::UpToDate => None,
        }
    }
}

/// Both sides have commits: sync. Something to pull comes before anything else (a push would be
/// refused, and a pull keeps local changes). Then local changes: commit and push. Then commits to
/// push. Otherwise there is nothing to do.
pub fn quick_action(facts: GitFacts) -> Quick {
    match (facts.ahead > 0, facts.behind > 0) {
        (true, true) => Quick::Sync,
        (false, true) => Quick::Pull,
        _ if facts.dirty => Quick::CommitPush,
        (true, false) => Quick::Push,
        _ => Quick::UpToDate,
    }
}

/// The menu's rows, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Commit,
    CommitPush,
    Pull,
    Push,
    PullRequest,
}

pub const ITEMS: [Item; 5] = [
    Item::Commit,
    Item::CommitPush,
    Item::Pull,
    Item::Push,
    Item::PullRequest,
];

impl Item {
    pub fn icon(self) -> &'static str {
        match self {
            Item::Commit => "commit",
            Item::CommitPush | Item::Push => "cloud-upload",
            Item::Pull => "cloud-download",
            Item::PullRequest => "git-pull-request",
        }
    }

    /// "Commit" is the same word in both languages, so it never goes through `t`.
    pub fn label(self) -> &'static str {
        match self {
            Item::Commit => "Commit",
            Item::CommitPush => t("Commit e enviar"),
            Item::Pull => t("Puxar"),
            Item::Push => t("Enviar"),
            Item::PullRequest => t("Criar PR"),
        }
    }
}

/// Why a row is off, shown as its tooltip; `None` when it can be used.
pub fn disabled_reason(item: Item, facts: GitFacts) -> Option<&'static str> {
    match item {
        Item::Commit if !facts.dirty => Some(t("Nada para commitar.")),
        Item::CommitPush if !facts.dirty => Some(t("Nada para commitar.")),
        // A push after the commit would be refused while the branch is behind.
        Item::CommitPush if facts.behind > 0 => Some(t("Puxe antes de enviar.")),
        Item::Pull if facts.behind == 0 => Some(t("Já está em dia.")),
        Item::Push if facts.ahead == 0 => Some(t("Nada para enviar.")),
        Item::Push if facts.behind > 0 => Some(t("Puxe antes de enviar.")),
        _ => None,
    }
}

/// What the button is doing; it is off meanwhile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Running {
    Pulling,
    Pushing,
    Syncing,
    Committing,
}

impl Running {
    pub fn label(self) -> &'static str {
        match self {
            Running::Pulling => t("Puxando…"),
            Running::Pushing => t("Enviando…"),
            Running::Syncing => t("Sincronizando…"),
            Running::Committing => t("Salvando…"),
        }
    }
}

/// The menu's last line: an amber warning when behind, else when GitHub was last asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Footer {
    Behind,
    Checked(String),
}

pub fn footer(facts: GitFacts, checked_at: Option<u64>, now: u64) -> Footer {
    if facts.behind > 0 {
        return Footer::Behind;
    }
    Footer::Checked(match checked_at {
        None => t("Ainda não verificado no GitHub").to_owned(),
        Some(at) => checked_text(now.saturating_sub(at) / 1000),
    })
}

/// "Verificado no GitHub há 3 min" for `seconds` since the check.
pub fn checked_text(seconds: u64) -> String {
    match seconds {
        0..=59 => t("Verificado no GitHub há menos de 1 min").to_owned(),
        60..=3599 => tf("Verificado no GitHub há {} min", &[&(seconds / 60)]),
        3600..=86399 => tf("Verificado no GitHub há {} h", &[&(seconds / 3600)]),
        _ => tf("Verificado no GitHub há {} dias", &[&(seconds / 86400)]),
    }
}

/// Files of a conflict as one line: the first few, then how many more.
pub fn conflict_line(files: &[String]) -> String {
    const SHOWN: usize = 4;
    if files.is_empty() {
        return t("O Git não conseguiu juntar as alterações.").to_owned();
    }
    let mut list = files
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if files.len() > SHOWN {
        list.push_str(&format!(" +{}", files.len() - SHOWN));
    }
    tf("Conflito em: {}", &[&list])
}

/// A rough width of the button for the title bar's room, in px.
pub fn width_hint(label: Option<&str>) -> f32 {
    let label = label.map_or(0., |label| 6. + label.chars().count() as f32 * 6.6);
    7. + 16. + label + 7. + 20. + 4.
}

#[cfg(test)]
mod tests {
    use super::*;

    fn facts(ahead: u64, behind: u64, dirty: bool) -> GitFacts {
        GitFacts {
            ahead,
            behind,
            dirty,
        }
    }

    #[test]
    fn the_quick_action_follows_the_git_state() {
        assert_eq!(quick_action(facts(0, 2, false)), Quick::Pull);
        assert_eq!(quick_action(facts(3, 0, false)), Quick::Push);
        assert_eq!(quick_action(facts(0, 0, true)), Quick::CommitPush);
        assert_eq!(quick_action(facts(2, 0, true)), Quick::CommitPush);
        assert_eq!(quick_action(facts(1, 1, false)), Quick::Sync);
        assert_eq!(quick_action(facts(0, 0, false)), Quick::UpToDate);
    }

    #[test]
    fn something_to_pull_wins_over_local_changes() {
        assert_eq!(quick_action(facts(0, 1, true)), Quick::Pull);
        assert_eq!(quick_action(facts(2, 1, true)), Quick::Sync);
    }

    #[test]
    fn only_the_up_to_date_cloud_has_no_label() {
        assert_eq!(Quick::UpToDate.label(), None);
        assert_eq!(Quick::Pull.label(), Some("Puxar"));
        assert_eq!(Quick::Push.label(), Some("Enviar"));
        assert_eq!(Quick::CommitPush.label(), Some("Commit e enviar"));
        assert_eq!(Quick::Sync.label(), Some("Sincronizar"));
    }

    #[test]
    fn menu_rows_say_why_they_are_off() {
        let clean = facts(0, 0, false);
        assert_eq!(
            disabled_reason(Item::Commit, clean),
            Some("Nada para commitar.")
        );
        assert_eq!(
            disabled_reason(Item::CommitPush, clean),
            Some("Nada para commitar.")
        );
        assert_eq!(disabled_reason(Item::Pull, clean), Some("Já está em dia."));
        assert_eq!(
            disabled_reason(Item::Push, clean),
            Some("Nada para enviar.")
        );
        assert_eq!(disabled_reason(Item::PullRequest, clean), None);

        let behind = facts(0, 2, false);
        assert_eq!(disabled_reason(Item::Pull, behind), None);
        assert_eq!(
            disabled_reason(Item::Push, behind),
            Some("Nada para enviar.")
        );

        let both = facts(1, 1, false);
        assert_eq!(
            disabled_reason(Item::Push, both),
            Some("Puxe antes de enviar.")
        );

        let dirty = facts(0, 0, true);
        assert_eq!(disabled_reason(Item::Commit, dirty), None);
        assert_eq!(disabled_reason(Item::CommitPush, dirty), None);
        let dirty_behind = facts(0, 1, true);
        assert_eq!(disabled_reason(Item::Commit, dirty_behind), None);
        assert_eq!(
            disabled_reason(Item::CommitPush, dirty_behind),
            Some("Puxe antes de enviar.")
        );

        let ahead = facts(2, 0, false);
        assert_eq!(disabled_reason(Item::Push, ahead), None);
        assert_eq!(disabled_reason(Item::Pull, ahead), Some("Já está em dia."));
    }

    #[test]
    fn the_footer_warns_when_behind_and_otherwise_says_when_it_checked() {
        let now = 1_000_000_000;
        assert_eq!(footer(facts(0, 1, false), Some(now), now), Footer::Behind);
        assert_eq!(footer(facts(1, 2, false), None, now), Footer::Behind);
        assert_eq!(
            footer(facts(0, 0, false), Some(now - 30_000), now),
            Footer::Checked("Verificado no GitHub há menos de 1 min".into())
        );
        assert_eq!(
            footer(facts(0, 0, false), Some(now - 60_000), now),
            Footer::Checked("Verificado no GitHub há 1 min".into())
        );
        assert_eq!(
            footer(facts(0, 0, false), Some(now - 3 * 3_600_000), now),
            Footer::Checked("Verificado no GitHub há 3 h".into())
        );
        assert_eq!(
            footer(facts(0, 0, false), None, now),
            Footer::Checked("Ainda não verificado no GitHub".into())
        );
        // A clock that went back reads as just now.
        assert_eq!(
            footer(facts(0, 0, false), Some(now + 5_000), now),
            Footer::Checked("Verificado no GitHub há menos de 1 min".into())
        );
        assert_eq!(checked_text(2 * 86_400), "Verificado no GitHub há 2 dias");
    }

    #[test]
    fn conflicts_list_a_few_files_and_count_the_rest() {
        let files: Vec<String> = ["a", "b", "c", "d", "e", "f"]
            .iter()
            .map(|f| f.to_string())
            .collect();
        assert_eq!(conflict_line(&files[..2]), "Conflito em: a, b");
        assert_eq!(conflict_line(&files), "Conflito em: a, b, c, d +2");
        assert!(!conflict_line(&[]).is_empty());
    }

    #[test]
    fn the_running_label_names_the_work() {
        assert_eq!(Running::Pulling.label(), "Puxando…");
        assert_eq!(Running::Pushing.label(), "Enviando…");
        assert_eq!(Running::Syncing.label(), "Sincronizando…");
        assert_eq!(Running::Committing.label(), "Salvando…");
    }

    #[test]
    fn the_icon_alone_is_narrower_than_a_labelled_button() {
        assert!(width_hint(None) < width_hint(Some("Puxar")));
        assert!(width_hint(Some("Puxar")) < width_hint(Some("Commit e enviar")));
    }
}
