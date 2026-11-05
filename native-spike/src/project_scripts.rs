//! Project scripts (ADR-078, ADR-151) in the native app: the Setup and On finish scripts a project
//! keeps, its loopback preview address, the run notice above the composer and the arguments sent to
//! the host's `project_scripts_action`. Every decision lives here and is tested; the drawing is in
//! `project_scripts_view.rs`. Scripts run only in the host, never here.

use serde_json::{Value, json};

/// The host's limit for one script, in bytes (`project_scripts::MAX_SCRIPT_BYTES`).
pub const MAX_SCRIPT_BYTES: usize = 8 * 1024;
/// The host's limit for the preview address, in characters (`PREVIEW_URL_MAX_LENGTH`).
pub const MAX_PREVIEW_CHARS: usize = 512;

/// The two scripts. `key` is the host's name for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Setup,
    Finish,
}

impl Kind {
    pub fn key(self) -> &'static str {
        match self {
            Kind::Setup => "setup",
            Kind::Finish => "finish",
        }
    }

    pub fn parse(text: &str) -> Option<Kind> {
        match text {
            "setup" => Some(Kind::Setup),
            "finish" => Some(Kind::Finish),
            _ => None,
        }
    }
}

/// The text fields of the project's settings popup, in the order Tab visits them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    Setup,
    Finish,
    Preview,
}

impl Field {
    /// Setup and On finish take several lines; the preview address takes one.
    pub fn multiline(self) -> bool {
        !matches!(self, Field::Preview)
    }

    /// The field Tab moves to, or `None` after the last one.
    pub fn next(self) -> Option<Field> {
        match self {
            Field::Setup => Some(Field::Finish),
            Field::Finish => Some(Field::Preview),
            Field::Preview => None,
        }
    }
}

/// What the host keeps for a project (`ProjectScripts`). A blank script means none.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Saved {
    pub setup: String,
    pub finish: String,
    pub preview: String,
    pub open_preview: bool,
}

impl Saved {
    /// The `scripts` object of a project as the host sends it.
    pub fn of(scripts: &Value) -> Self {
        let text = |key: &str| scripts[key].as_str().unwrap_or("").to_owned();
        Self {
            setup: text("setup"),
            finish: text("onFinish"),
            preview: text("previewUrl"),
            open_preview: scripts["openPreview"].as_bool() == Some(true),
        }
    }

    /// The saved body of one script.
    pub fn body(&self, kind: Kind) -> &str {
        match kind {
            Kind::Setup => &self.setup,
            Kind::Finish => &self.finish,
        }
    }

    /// True when the script has something other than blanks.
    pub fn has(&self, kind: Kind) -> bool {
        !self.body(kind).trim().is_empty()
    }
}

/// The settings popup's text while it is open; sent with `save_args` or dropped.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Draft {
    pub setup: String,
    pub finish: String,
    pub preview: String,
    pub open_preview: bool,
}

impl Draft {
    pub fn of(saved: &Saved) -> Self {
        Self {
            setup: saved.setup.clone(),
            finish: saved.finish.clone(),
            preview: saved.preview.clone(),
            open_preview: saved.open_preview,
        }
    }

    pub fn text(&self, field: Field) -> &str {
        match field {
            Field::Setup => &self.setup,
            Field::Finish => &self.finish,
            Field::Preview => &self.preview,
        }
    }

    pub fn field_mut(&mut self, field: Field) -> &mut String {
        match field {
            Field::Setup => &mut self.setup,
            Field::Finish => &mut self.finish,
            Field::Preview => &mut self.preview,
        }
    }

    /// The preview toggle works only with an address to open.
    pub fn open_preview_allowed(&self) -> bool {
        !self.preview.trim().is_empty()
    }

    /// Each script within the host's limit and the preview address valid. Save stays off otherwise.
    pub fn valid(&self) -> bool {
        self.setup.len() <= MAX_SCRIPT_BYTES
            && self.finish.len() <= MAX_SCRIPT_BYTES
            && preview_valid(&self.preview)
    }

    /// True when saving would change what the host keeps. Surrounding spaces do not count.
    pub fn changed(&self, saved: &Saved) -> bool {
        self.setup.trim() != saved.setup.trim()
            || self.finish.trim() != saved.finish.trim()
            || self.preview.trim() != saved.preview.trim()
            || self.open_preview != saved.open_preview
    }

    /// The arguments of `project_scripts_action` that save this draft. Blank means null.
    pub fn save_args(&self, project_id: &str) -> Value {
        json!({ "action": {
            "type": "save",
            "projectId": project_id,
            "setup": nullable(&self.setup),
            "onFinish": nullable(&self.finish),
            "previewUrl": nullable(&self.preview),
            "openPreview": self.open_preview && self.open_preview_allowed(),
        } })
    }
}

fn nullable(text: &str) -> Value {
    match text.trim() {
        "" => Value::Null,
        trimmed => Value::String(trimmed.to_owned()),
    }
}

/// A blank address, or an http(s) URL on localhost, 127.0.0.1 or [::1] with a port. Mirrors the
/// front end's `isPreviewUrlValid`; the host checks it again.
pub fn preview_valid(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() {
        return true;
    }
    if text.chars().count() > MAX_PREVIEW_CHARS
        || text.chars().any(|c| c.is_whitespace() || (c as u32) < 32)
    {
        return false;
    }
    let Some((scheme, rest)) = text.split_once("://") else {
        return false;
    };
    if !scheme.eq_ignore_ascii_case("http") && !scheme.eq_ignore_ascii_case("https") {
        return false;
    }
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@') {
        return false;
    }
    let (host, port) = if let Some(inside) = authority.strip_prefix('[') {
        let Some(close) = inside.find(']') else {
            return false;
        };
        let host = &authority[..close + 2];
        let Some(port) = authority[close + 2..].strip_prefix(':') else {
            return false;
        };
        (host, port)
    } else {
        match authority.rsplit_once(':') {
            Some(parts) => parts,
            None => return false,
        }
    };
    let host_ok = matches!(
        host.to_ascii_lowercase().as_str(),
        "localhost" | "127.0.0.1" | "[::1]"
    );
    host_ok
        && !port.is_empty()
        && port.bytes().all(|b| b.is_ascii_digit())
        && port.parse::<u16>().is_ok_and(|port| port > 0)
}

/// The settings popup's editing state: the project, the draft and the field with the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub project: usize,
    pub draft: Draft,
    pub focus: Option<Field>,
}

impl Edit {
    pub fn new(project: usize, saved: &Saved) -> Self {
        Self {
            project,
            draft: Draft::of(saved),
            focus: None,
        }
    }

    /// Enter in a multi-line field adds a line; in the address it does nothing. Returns whether
    /// the key was taken.
    pub fn enter(&mut self) -> bool {
        match self.focus {
            Some(field) if field.multiline() => {
                self.draft.field_mut(field).push('\n');
                true
            }
            Some(_) => true,
            None => false,
        }
    }

    /// Backspace removes the last character of the focused field.
    pub fn backspace(&mut self) -> bool {
        match self.focus {
            Some(field) => {
                self.draft.field_mut(field).pop();
                true
            }
            None => false,
        }
    }

    /// Tab moves to the next field and leaves the last one.
    pub fn tab(&mut self) -> bool {
        match self.focus {
            Some(field) => {
                self.focus = field.next();
                true
            }
            None => false,
        }
    }

    /// Escape leaves the focused field and keeps the popup open.
    pub fn escape(&mut self) -> bool {
        self.focus.take().is_some()
    }
}

/// One script run of a session (`ScriptRun`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Running,
    Succeeded,
    Failed,
    TimedOut,
    Cancelled,
}

impl Status {
    pub fn parse(text: &str) -> Option<Status> {
        match text {
            "running" => Some(Status::Running),
            "succeeded" => Some(Status::Succeeded),
            "failed" => Some(Status::Failed),
            "timedOut" => Some(Status::TimedOut),
            "cancelled" => Some(Status::Cancelled),
            _ => None,
        }
    }
}

/// The latest run of one script in a session's worktree, with the output's bounded tail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub kind: Kind,
    pub status: Status,
    pub started_at: String,
    pub exit_code: Option<i64>,
    pub output: String,
}

/// The runs in a session's `scripts` object. Unknown entries are skipped.
pub fn runs_of(scripts: &Value) -> Vec<Run> {
    scripts["runs"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|run| {
            Some(Run {
                kind: Kind::parse(run["kind"].as_str()?)?,
                status: Status::parse(run["status"].as_str()?)?,
                started_at: run["startedAt"].as_str().unwrap_or("").to_owned(),
                exit_code: run["exitCode"].as_i64(),
                output: run["output"].as_str().unwrap_or("").to_owned(),
            })
        })
        .collect()
}

/// The run the notice shows: a running one first, else the most recently started (the first of
/// equal start times).
pub fn visible(runs: &[Run]) -> Option<&Run> {
    if let Some(running) = runs.iter().find(|run| run.status == Status::Running) {
        return Some(running);
    }
    let mut latest: Option<&Run> = None;
    for run in runs {
        if latest.is_none_or(|best| run.started_at > best.started_at) {
            latest = Some(run);
        }
    }
    latest
}

/// The notice's icon tone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Working,
    Done,
    Stopped,
    Failed,
}

/// What the notice above the composer offers for one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Notice {
    pub tone: Tone,
    pub stop: bool,
    pub run_again: bool,
    /// Run again waits for the conversation to go idle.
    pub run_again_enabled: bool,
    pub dismiss: bool,
    /// The output can be shown once the run has ended.
    pub output: bool,
    /// Shown for a failed run that reported one.
    pub exit_code: Option<i64>,
    /// A failed or timed-out script leaves the conversation going on.
    pub continues: bool,
}

/// The notice for `run`. `working` is whether the conversation is starting, running or waiting.
pub fn notice(run: &Run, working: bool) -> Notice {
    let running = run.status == Status::Running;
    let failed = matches!(run.status, Status::Failed | Status::TimedOut);
    Notice {
        tone: match run.status {
            Status::Running => Tone::Working,
            Status::Succeeded => Tone::Done,
            Status::Cancelled => Tone::Stopped,
            Status::Failed | Status::TimedOut => Tone::Failed,
        },
        stop: running,
        run_again: !running && (failed || run.status == Status::Cancelled),
        run_again_enabled: !working,
        dismiss: !running,
        output: !running,
        exit_code: if run.status == Status::Failed {
            run.exit_code
        } else {
            None
        },
        continues: failed,
    }
}

/// A script may run only in an isolated worktree of a conversation that is not a side chat, while
/// the conversation is idle and no script is already running (the host refuses the rest).
pub fn can_run(isolated: bool, side_chat: bool, working: bool, script_running: bool) -> bool {
    isolated && !side_chat && !working && !script_running
}

/// Arguments of a `run` action.
pub fn run_args(session_id: &str, kind: Kind) -> Value {
    json!({ "action": { "type": "run", "sessionId": session_id, "kind": kind.key() } })
}

/// Arguments of a `cancel` action.
pub fn cancel_args(session_id: &str) -> Value {
    json!({ "action": { "type": "cancel", "sessionId": session_id } })
}

/// Arguments of a `dismiss` action.
pub fn dismiss_args(session_id: &str, kind: Kind) -> Value {
    json!({ "action": { "type": "dismiss", "sessionId": session_id, "kind": kind.key() } })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(kind: Kind, status: Status, started: &str) -> Run {
        Run {
            kind,
            status,
            started_at: started.into(),
            exit_code: None,
            output: String::new(),
        }
    }

    #[test]
    fn loopback_addresses_only() {
        for good in [
            "",
            "   ",
            "http://localhost:5173",
            "https://127.0.0.1:8443/app?x=1",
            "http://[::1]:3000/",
            "HTTP://LOCALHOST:8080",
            "  http://localhost:3000\n",
        ] {
            assert!(preview_valid(good), "{good:?} should be valid");
        }
        for bad in [
            "localhost:5173",
            "ftp://localhost:21",
            "http://example.com:3000",
            "http://localhost",
            "http://localhost:",
            "http://localhost:0",
            "http://localhost:70000",
            "http://user:pw@localhost:3000",
            "http://localhost:3000@evil.com",
            "http://localhost:30 00",
            "http://[::1]3000",
            "http://:3000",
        ] {
            assert!(!preview_valid(bad), "{bad:?} should be invalid");
        }
        let long = format!("http://localhost:3000/{}", "a".repeat(MAX_PREVIEW_CHARS));
        assert!(!preview_valid(&long));
    }

    #[test]
    fn draft_validity_and_limits() {
        let mut draft = Draft::default();
        assert!(draft.valid());
        draft.setup = "x".repeat(MAX_SCRIPT_BYTES);
        assert!(draft.valid());
        draft.setup.push('x');
        assert!(!draft.valid());
        draft.setup.clear();
        draft.preview = "http://example.com".into();
        assert!(!draft.valid());
        assert!(draft.open_preview_allowed());
        draft.preview = "  ".into();
        assert!(!draft.open_preview_allowed());
    }

    #[test]
    fn changed_ignores_surrounding_spaces() {
        let saved = Saved {
            setup: "npm install".into(),
            ..Saved::default()
        };
        let mut draft = Draft::of(&saved);
        assert!(!draft.changed(&saved));
        draft.setup = "  npm install\n".into();
        assert!(!draft.changed(&saved));
        draft.setup = "npm ci".into();
        assert!(draft.changed(&saved));
        let mut toggled = Draft::of(&saved);
        toggled.open_preview = true;
        assert!(toggled.changed(&saved));
    }

    #[test]
    fn save_args_trim_and_null_blanks() {
        let draft = Draft {
            setup: "  npm install \n".into(),
            finish: "   ".into(),
            preview: "".into(),
            open_preview: true,
        };
        let args = draft.save_args("p1");
        assert_eq!(args["action"]["type"], "save");
        assert_eq!(args["action"]["projectId"], "p1");
        assert_eq!(args["action"]["setup"], "npm install");
        assert!(args["action"]["onFinish"].is_null());
        assert!(args["action"]["previewUrl"].is_null());
        // Without an address the preview cannot open, whatever the toggle says.
        assert_eq!(args["action"]["openPreview"], false);

        let with_preview = Draft {
            preview: "http://localhost:5173".into(),
            open_preview: true,
            ..Draft::default()
        };
        assert_eq!(with_preview.save_args("p1")["action"]["openPreview"], true);
    }

    #[test]
    fn saved_reads_the_host_object() {
        let saved = Saved::of(&json!({
            "setup": "npm i", "onFinish": null, "previewUrl": "http://localhost:3000", "openPreview": true
        }));
        assert_eq!(saved.setup, "npm i");
        assert!(saved.has(Kind::Setup));
        assert!(!saved.has(Kind::Finish));
        assert!(saved.open_preview);
        assert_eq!(Saved::of(&Value::Null), Saved::default());
    }

    #[test]
    fn editing_keys_go_to_the_focused_field() {
        let mut edit = Edit::new(2, &Saved::default());
        assert!(!edit.enter());
        assert!(!edit.backspace());
        assert!(!edit.tab());
        assert!(!edit.escape());

        edit.focus = Some(Field::Setup);
        edit.draft.setup = "a".into();
        assert!(edit.enter());
        edit.draft.setup.push('b');
        assert_eq!(edit.draft.setup, "a\nb");
        assert!(edit.backspace());
        assert_eq!(edit.draft.setup, "a\n");

        assert!(edit.tab());
        assert_eq!(edit.focus, Some(Field::Finish));
        assert!(edit.tab());
        assert_eq!(edit.focus, Some(Field::Preview));
        assert!(edit.enter());
        assert_eq!(edit.draft.preview, "");
        assert!(edit.tab());
        assert_eq!(edit.focus, None);
        edit.focus = Some(Field::Finish);
        assert!(edit.escape());
        assert_eq!(edit.focus, None);
    }

    #[test]
    fn visible_run_prefers_running_then_latest() {
        assert_eq!(visible(&[]), None);
        let runs = vec![
            run(Kind::Setup, Status::Succeeded, "2026-01-01T10:00:00Z"),
            run(Kind::Finish, Status::Running, "2026-01-01T09:00:00Z"),
            run(Kind::Setup, Status::Failed, "2026-01-01T11:00:00Z"),
        ];
        assert_eq!(visible(&runs).unwrap().status, Status::Running);
        let done = &runs[..1]
            .iter()
            .chain(&runs[2..])
            .cloned()
            .collect::<Vec<_>>();
        assert_eq!(visible(done).unwrap().status, Status::Failed);
        let tie = vec![
            run(Kind::Setup, Status::Succeeded, "t"),
            run(Kind::Finish, Status::Cancelled, "t"),
        ];
        assert_eq!(visible(&tie).unwrap().kind, Kind::Setup);
    }

    #[test]
    fn runs_skip_unknown_entries() {
        let runs = runs_of(&json!({ "runs": [
            { "kind": "setup", "status": "failed", "startedAt": "a", "exitCode": 2, "output": "boom" },
            { "kind": "other", "status": "failed", "startedAt": "b" },
            { "kind": "finish", "status": "nope", "startedAt": "c" },
            { "kind": "finish", "status": "timedOut", "startedAt": "d" }
        ]}));
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].exit_code, Some(2));
        assert_eq!(runs[0].output, "boom");
        assert_eq!(runs[1].status, Status::TimedOut);
        assert!(runs_of(&Value::Null).is_empty());
    }

    #[test]
    fn notice_decisions_per_status() {
        let running = notice(&run(Kind::Setup, Status::Running, "a"), false);
        assert!(running.stop && !running.dismiss && !running.output && !running.run_again);
        assert_eq!(running.tone, Tone::Working);

        let mut failed_run = run(Kind::Finish, Status::Failed, "a");
        failed_run.exit_code = Some(3);
        let failed = notice(&failed_run, true);
        assert!(failed.run_again && !failed.run_again_enabled && failed.dismiss);
        assert_eq!(failed.exit_code, Some(3));
        assert!(failed.continues);
        assert_eq!(failed.tone, Tone::Failed);

        let timed = notice(&run(Kind::Setup, Status::TimedOut, "a"), false);
        assert!(timed.run_again && timed.run_again_enabled && timed.exit_code.is_none());
        assert!(timed.continues);

        let stopped = notice(&run(Kind::Setup, Status::Cancelled, "a"), false);
        assert!(stopped.run_again && !stopped.continues);
        assert_eq!(stopped.tone, Tone::Stopped);

        let done = notice(&run(Kind::Finish, Status::Succeeded, "a"), false);
        assert!(!done.run_again && done.output && done.dismiss && !done.continues);
        assert_eq!(done.tone, Tone::Done);
    }

    #[test]
    fn run_gate_and_working_states() {
        assert!(can_run(true, false, false, false));
        assert!(!can_run(false, false, false, false));
        assert!(!can_run(true, true, false, false));
        assert!(!can_run(true, false, true, false));
        assert!(!can_run(true, false, false, true));
    }

    #[test]
    fn action_arguments_match_the_host() {
        assert_eq!(
            run_args("s", Kind::Finish),
            json!({ "action": { "type": "run", "sessionId": "s", "kind": "finish" } })
        );
        assert_eq!(
            cancel_args("s"),
            json!({ "action": { "type": "cancel", "sessionId": "s" } })
        );
        assert_eq!(
            dismiss_args("s", Kind::Setup),
            json!({ "action": { "type": "dismiss", "sessionId": "s", "kind": "setup" } })
        );
    }
}
