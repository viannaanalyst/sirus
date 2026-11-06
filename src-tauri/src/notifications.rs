//! Native-owned session alerts. Renderer actions cannot supply message text,
//! session IDs, sound paths or lifecycle events.
use crate::runtime::{AppHandle, Emitter, Manager, State};
use crate::{
    commands::AppState,
    error::{Error, Result},
    models::{MessageRole, PendingRequestKind, Session, SessionStatus},
};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

#[cfg(all(target_os = "macos", feature = "desktop"))]
#[path = "notifications_macos.rs"]
mod platform;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Sound {
    Glass,
    Ping,
    Pop,
    Submarine,
    Tink,
    Hero,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase", deny_unknown_fields)]
pub struct Preferences {
    pub toasts: bool,
    pub system: bool,
    pub sounds: bool,
    pub foreground: bool,
    pub permissions: bool,
    pub questions: bool,
    pub completion: bool,
    /// Someone else opened a pull request or asked for the person's review (ADR-050).
    pub pull_requests: bool,
    pub permission_sound: Sound,
    pub question_sound: Sound,
    pub completion_sound: Sound,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            toasts: true,
            system: true,
            sounds: true,
            foreground: false,
            permissions: true,
            questions: true,
            completion: true,
            pull_requests: true,
            permission_sound: Sound::Ping,
            question_sound: Sound::Pop,
            completion_sound: Sound::Glass,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Permission {
    Prompt,
    Granted,
    Denied,
    Unsupported,
}
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub enum Action {
    Status {},
    Request {},
    Test {},
    Settings {},
    Preview {
        sound: Sound,
    },
    /// Pull requests the renderer found new since the person last looked (ADR-050). Native
    /// keeps only those its own last open pull request list confirms and writes the text.
    #[serde(rename = "pullRequests")]
    PullRequests {
        items: Vec<PullRequestRef>,
    },
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PullRequestRef {
    repository: String,
    number: u32,
    reason: crate::github_inbox::AlertReason,
}
/// Most pull requests one alert request may name.
const PULL_ALERT_REQUEST_LIMIT: usize = 50;
/// At most this many pull requests get their own alert at once; the rest share one summary.
const PULL_ALERT_BURST: usize = 3;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Permission,
    Question,
    Completion,
    /// A turn ended in an error (ADR-086); follows the completion switch and sound.
    Failure,
    /// A snoozed conversation came back (ADR-103). The person asked for it, so no event
    /// switch hides it; the channels (toasts, system, sounds, foreground) still apply.
    Reminder,
    /// A pull request someone else opened or asked the person to review (ADR-050); follows
    /// the pull requests switch and plays the completion sound.
    #[serde(rename = "pullRequest")]
    PullRequest,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    pub created_at: i64,
    pub id: String,
    pub session_id: String,
    pub kind: Kind,
    pub title: String,
    pub body: String,
    /// The pull request a click opens (`owner/repo#n`) when `kind` is `pullRequest`; a summary
    /// has none and opens the Pull requests page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull: Option<String>,
    /// What the in-app card shows under the title: the command or question awaiting an
    /// answer, the start of the final answer or the error.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Default)]
pub struct NotificationState {
    seen: Mutex<VecDeque<String>>,
    previewed: Mutex<Option<Instant>>,
    tested: Mutex<Option<Instant>>,
    requesting: std::sync::atomic::AtomicBool,
}
fn notification_error(message: &str) -> Error {
    Error::new("notification", message)
}
fn admit_time(slot: &Mutex<Option<Instant>>, interval: Duration) -> Result<()> {
    let mut slot = slot.lock();
    if slot.is_some_and(|previous| previous.elapsed() < interval) {
        return Err(notification_error("Please wait before testing again."));
    }
    *slot = Some(Instant::now());
    Ok(())
}
fn remember(seen: &mut VecDeque<String>, key: &str) -> bool {
    if seen.iter().any(|existing| existing == key) {
        return false;
    }
    if seen.len() == 256 {
        seen.pop_front();
    }
    seen.push_back(key.into());
    true
}
fn candidates(session: &Session) -> Vec<(String, Kind)> {
    if session.status == SessionStatus::Failed {
        return session
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .map(|message| {
                vec![(
                    format!("{}:failed:{}", session.id, message.id),
                    Kind::Failure,
                )]
            })
            .unwrap_or_default();
    }
    if session.status == SessionStatus::Completed && session.last_error.is_none() {
        return session
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::User)
            .map(|message| {
                vec![(
                    format!("{}:completed:{}", session.id, message.id),
                    Kind::Completion,
                )]
            })
            .unwrap_or_default();
    }
    if session.status != SessionStatus::Waiting {
        return vec![];
    }
    session
        .pending_requests
        .iter()
        .map(|request| {
            (
                format!(
                    "{}:{}:{}:{}",
                    session.id, request.generation, request.turn_id, request.request_id
                ),
                if matches!(request.kind, PendingRequestKind::UserInput { .. }) {
                    Kind::Question
                } else {
                    Kind::Permission
                },
            )
        })
        .collect()
}
fn choice(prefs: &Preferences, kind: Kind) -> (bool, Sound) {
    match kind {
        Kind::Permission => (prefs.permissions, prefs.permission_sound),
        Kind::Question => (prefs.questions, prefs.question_sound),
        Kind::Completion | Kind::Failure => (prefs.completion, prefs.completion_sound),
        Kind::Reminder => (true, prefs.completion_sound),
        Kind::PullRequest => (prefs.pull_requests, prefs.completion_sound),
    }
}
fn copy(kind: Kind, portuguese: bool) -> &'static str {
    match (kind, portuguese) {
        (Kind::Permission, true) => "Permissão necessária",
        (Kind::Question, true) => "Resposta necessária",
        (Kind::Completion, true) => "Tarefa concluída",
        (Kind::Permission, false) => "Permission needed",
        (Kind::Question, false) => "Answer needed",
        (Kind::Completion, false) => "Task completed",
        (Kind::Failure, true) => "A tarefa falhou",
        (Kind::Failure, false) => "Task failed",
        (Kind::Reminder, true) => "Lembrete",
        (Kind::Reminder, false) => "Reminder",
        (Kind::PullRequest, true) => "Novo pull request",
        (Kind::PullRequest, false) => "New pull request",
    }
}
/// The phone alert's status line and the plural phrase its grouped form uses.
fn phone_status(kind: Kind, portuguese: bool) -> (&'static str, &'static str) {
    match (kind, portuguese) {
        (Kind::Completion, true) => ("✓ Concluída", "conversas concluídas"),
        (Kind::Completion, false) => ("✓ Finished", "conversations finished"),
        (Kind::Permission, true) => ("⚠︎ Precisa de aprovação", "conversas pedem aprovação"),
        (Kind::Permission, false) => ("⚠︎ Approval needed", "conversations need approval"),
        (Kind::Question, true) => ("? Pergunta para você", "perguntas para você"),
        (Kind::Question, false) => ("? Question for you", "questions for you"),
        (Kind::Failure, true) => ("✕ Falhou", "conversas falharam"),
        (Kind::Failure, false) => ("✕ Failed", "conversations failed"),
        (Kind::Reminder, true) => ("⏰ De volta", "conversas de volta"),
        (Kind::Reminder, false) => ("⏰ Back from snooze", "conversations back"),
        (Kind::PullRequest, true) => ("Pull request", "novos pull requests"),
        (Kind::PullRequest, false) => ("Pull request", "new pull requests"),
    }
}
fn kind_name(kind: Kind) -> &'static str {
    match kind {
        Kind::Permission => "permission",
        Kind::Question => "question",
        Kind::Completion => "completion",
        Kind::Failure => "failure",
        Kind::Reminder => "reminder",
        Kind::PullRequest => "pullRequest",
    }
}
/// A phone alert: the conversation as the title, then what happened and
/// where, then its detail after a dash. iOS joins the lines of a web alert, so nothing relies
/// on a line break.
fn phone_alert(
    kind: Kind,
    portuguese: bool,
    project: &str,
    session: &str,
    detail: Option<&str>,
) -> (String, String) {
    let (project, session) = (bounded_label(project), bounded_label(session));
    let titled = !session.trim().is_empty() && !session.trim().eq_ignore_ascii_case(project.trim());
    let title = if titled { session } else { project.clone() };
    let mut body = phone_status(kind, portuguese).0.to_owned();
    if titled && !project.trim().is_empty() {
        body.push_str(" · ");
        body.push_str(&project);
    }
    if let Some(detail) = detail.map(str::trim).filter(|detail| !detail.is_empty()) {
        body.push_str(" — ");
        body.push_str(detail);
    }
    (title, body)
}
/// What a phone alert adds under the title (ADR-086): the command or files awaiting
/// approval, the question asked, the start of the final answer or the error.
fn detail(session: &Session, key: &str, kind: Kind, portuguese: bool) -> Option<String> {
    let line = |text: &str| -> Option<String> {
        let text = text
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty() && !line.starts_with("```"))?
            .trim_start_matches(['#', '>', '-', '*', ' '])
            .replace(['*', '`'], "");
        (!text.is_empty()).then(|| {
            text.chars()
                .filter(|ch| !ch.is_control())
                .take(140)
                .collect()
        })
    };
    match kind {
        Kind::Permission | Kind::Question => {
            let request_id = key.rsplit(':').next()?;
            let request = session
                .pending_requests
                .iter()
                .find(|request| request.request_id == request_id)?;
            match &request.kind {
                PendingRequestKind::Command { command, .. } => {
                    line(command).map(|command| format!("$ {command}"))
                }
                PendingRequestKind::FileChange { changes, .. } => Some(if portuguese {
                    format!("Alterar {} arquivo(s)", changes.len())
                } else {
                    format!("Change {} file(s)", changes.len())
                }),
                PendingRequestKind::UserInput { questions } => questions
                    .first()
                    .and_then(|question| line(&question.question)),
                PendingRequestKind::Tool { name, .. } => line(name),
            }
        }
        Kind::Completion => session
            .messages
            .iter()
            .rev()
            .find(|message| message.role == MessageRole::Agent)
            .and_then(|message| line(&message.content)),
        Kind::Failure => session.last_error.as_deref().and_then(line),
        Kind::Reminder | Kind::PullRequest => None,
    }
}

fn bounded_label(value: &str) -> String {
    value
        .chars()
        .filter(|ch| !ch.is_control())
        .take(96)
        .collect()
}

/// Called only at fresh native request admission and settled completion, never
/// on bootstrap, metadata edits or renderer snapshots.
/// A native-originated status alert that is not tied to a pending request
/// (CI auto-fix green/paused). It honours the person's channel preferences.
/// "Project · Session", or the name once when the session is titled like its project.
fn notice_body(project: &str, session: &str) -> String {
    let (project, session) = (bounded_label(project), bounded_label(session));
    if session.trim().is_empty() || session.trim().eq_ignore_ascii_case(project.trim()) {
        project
    } else {
        format!("{project} · {session}")
    }
}

pub fn announce(app: &AppHandle, state: &Arc<AppState>, title: String, body: String) {
    let (prefs, foreground) = {
        let data = state.data.lock();
        let foreground = app
            .get_webview_window("main")
            .is_some_and(|window| window.is_focused().unwrap_or(true));
        (data.settings.notifications.clone(), foreground)
    };
    let notice = Notice {
        pull: None,
        detail: None,
        created_at: chrono::Utc::now().timestamp_millis(),
        id: uuid::Uuid::new_v4().to_string(),
        session_id: String::new(),
        kind: Kind::Completion,
        title: bounded_label(&title),
        body: bounded_label(&body),
    };
    crate::remote::notify(&notice.title, &notice.body, "", None, None);
    if prefs.toasts {
        let _ = app.emit("notification-activity", &notice);
    }
    #[cfg(all(target_os = "macos", feature = "desktop"))]
    if prefs.system && (!foreground || prefs.foreground) {
        let app = app.clone();
        crate::runtime::async_runtime::spawn(async move {
            if let Err(error) = platform::show(&app, &notice).await {
                tracing::debug!(%error, "system status alert unavailable");
            }
        });
    }
    #[cfg(not(all(target_os = "macos", feature = "desktop")))]
    {
        let _ = foreground;
        let sound = prefs.completion_sound;
        window_alert(app, &notice, &prefs, sound);
    }
}

/// The identity of one snooze reminder (ADR-103): a session returns once per wake.
fn reminder_key(session: &Session) -> Option<String> {
    let at = session.snooze_reminder_at.as_deref()?;
    session
        .snoozed_until
        .is_none()
        .then(|| format!("{}:reminder:{at}", session.id))
}

/// "Reminder" with the conversation's title, when a snoozed session comes back (ADR-103).
/// Called by the native snooze timer, also while the window is hidden; a click opens the
/// session like any activity alert.
pub fn remind(app: &AppHandle, state: &Arc<AppState>, session_id: &str) {
    let app = app.clone();
    let dispatcher = app.clone();
    let state = state.clone();
    let id = session_id.to_string();
    let _ = dispatcher.run_on_main_thread(move || {
        let data = state.data.lock();
        if state.closing.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let Some(session) = data.sessions.iter().find(|session| session.id == id) else {
            return;
        };
        let Some(project) = data
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
        else {
            return;
        };
        let Some(key) = reminder_key(session) else {
            return;
        };
        let prefs = data.settings.notifications.clone();
        let portuguese = data.settings.locale == "pt-BR";
        let notice = Notice {
            pull: None,
            detail: None,
            created_at: chrono::Utc::now().timestamp_millis(),
            id: key,
            session_id: id.clone(),
            kind: Kind::Reminder,
            title: copy(Kind::Reminder, portuguese).into(),
            body: notice_body(&project.name, &session.title),
        };
        let (project_name, session_title) = (project.name.clone(), session.title.clone());
        drop(data);
        if !remember(
            &mut app.state::<NotificationState>().seen.lock(),
            &notice.id,
        ) {
            return;
        }
        let foreground = app
            .get_webview_window("main")
            .is_some_and(|window| window.is_focused().unwrap_or(true));
        let eligible = !foreground || prefs.foreground;
        let (title, body) = phone_alert(
            Kind::Reminder,
            portuguese,
            &project_name,
            &session_title,
            None,
        );
        crate::remote::notify(
            &title,
            &body,
            &notice.session_id,
            None,
            Some((
                kind_name(Kind::Reminder),
                phone_status(Kind::Reminder, portuguese).1,
            )),
        );
        if prefs.toasts {
            let _ = app.emit("notification-activity", &notice);
        }
        #[cfg(all(target_os = "macos", feature = "desktop"))]
        {
            if prefs.sounds && eligible {
                let _ = platform::play(choice(&prefs, Kind::Reminder).1);
            }
            if prefs.system && eligible {
                let app = app.clone();
                crate::runtime::async_runtime::spawn(async move {
                    if let Err(error) = platform::show(&app, &notice).await {
                        tracing::debug!(%error, "system reminder unavailable");
                    }
                });
            }
        }
        #[cfg(not(all(target_os = "macos", feature = "desktop")))]
        if eligible {
            window_alert(&app, &notice, &prefs, choice(&prefs, Kind::Reminder).1);
        }
    });
}

pub fn publish(app: &AppHandle, state: &Arc<AppState>, snapshot: &Session) {
    let expected = candidates(snapshot);
    if expected.is_empty() {
        return;
    }
    let app = app.clone();
    let dispatcher = app.clone();
    let state = state.clone();
    let id = snapshot.id.clone();
    let _ = dispatcher.run_on_main_thread(move || {
        let data = state.data.lock();
        if state.closing.load(std::sync::atomic::Ordering::Acquire) {
            return;
        }
        let Some(session) = data.sessions.iter().find(|session| session.id == id) else {
            return;
        };
        let Some(project) = data
            .projects
            .iter()
            .find(|project| project.id == session.project_id)
        else {
            return;
        };
        let current = candidates(session);
        let prefs = data.settings.notifications.clone();
        let body = notice_body(&project.name, &session.title);
        let (project_name, session_title) = (project.name.clone(), session.title.clone());
        let portuguese = data.settings.locale == "pt-BR";
        let foreground = app
            .get_webview_window("main")
            .is_some_and(|window| window.is_focused().unwrap_or(true));
        let eligible = !foreground || prefs.foreground;
        let notices: Vec<_> = expected
            .into_iter()
            .filter(|candidate| current.contains(candidate))
            .collect();
        // The phone's app icon shows how many conversations need the person (ADR-124).
        let waiting = data
            .sessions
            .iter()
            .filter(|session| {
                !session.is_auxiliary()
                    && (session.status == SessionStatus::Waiting
                        || !session.pending_requests.is_empty())
            })
            .count();
        let details: std::collections::HashMap<String, Option<String>> = notices
            .iter()
            .map(|(key, kind)| (key.clone(), detail(session, key, *kind, portuguese)))
            .collect();
        drop(data);
        for (key, kind) in notices {
            // Consume even disabled events: enabling later cannot replay them.
            if !remember(&mut app.state::<NotificationState>().seen.lock(), &key) {
                continue;
            }
            let (enabled, sound) = choice(&prefs, kind);
            if !enabled {
                continue;
            }
            let notice = Notice {
                pull: None,
                detail: details.get(&key).cloned().flatten(),
                created_at: chrono::Utc::now().timestamp_millis(),
                id: key,
                session_id: id.clone(),
                kind,
                title: copy(kind, portuguese).into(),
                body: body.clone(),
            };
            // Paired phones get the alert whether or not the Mac window has focus (ADR-082):
            // the conversation, then what happened and where, then its detail (ADR-086).
            let (phone_title, phone_body) = phone_alert(
                kind,
                portuguese,
                &project_name,
                &session_title,
                details.get(&notice.id).and_then(Option::as_deref),
            );
            crate::remote::notify(
                &phone_title,
                &phone_body,
                &notice.session_id,
                Some(waiting),
                Some((kind_name(kind), phone_status(kind, portuguese).1)),
            );
            if prefs.toasts {
                let _ = app.emit("notification-activity", &notice);
            }
            #[cfg(all(target_os = "macos", feature = "desktop"))]
            {
                if prefs.sounds && eligible {
                    let _ = platform::play(sound);
                }
                if prefs.system && eligible {
                    let app = app.clone();
                    let notice = notice.clone();
                    crate::runtime::async_runtime::spawn(async move {
                        if let Err(error) = platform::show(&app, &notice).await {
                            tracing::debug!(%error, "system activity alert unavailable");
                        }
                    });
                }
            }
            #[cfg(not(all(target_os = "macos", feature = "desktop")))]
            if eligible {
                window_alert(&app, &notice, &prefs, sound);
            }
        }
    });
}

/// "ana opened a pull request in owner/repo", with "#12 Title" under it.
fn pull_text(alert: &crate::github_inbox::PullAlert, portuguese: bool) -> (String, String) {
    use crate::github_inbox::AlertReason;
    let (author, repository) = (
        bounded_label(&alert.author),
        bounded_label(&alert.repository),
    );
    let title = match (alert.reason, portuguese) {
        (AlertReason::Opened, false) => format!("{author} opened a pull request in {repository}"),
        (AlertReason::Opened, true) => format!("{author} abriu um pull request em {repository}"),
        (AlertReason::ReviewRequested, false) => {
            format!("{author} asked for your review in {repository}")
        }
        (AlertReason::ReviewRequested, true) => {
            format!("{author} pediu sua revisão em {repository}")
        }
    };
    (
        title,
        bounded_label(&format!("#{} {}", alert.number, alert.title)),
    )
}

/// The summary after the first `PULL_ALERT_BURST` alerts: "3 more new pull requests" with
/// their numbers under it.
fn pull_summary(rest: &[crate::github_inbox::PullAlert], portuguese: bool) -> (String, String) {
    let count = rest.len();
    let title = match (count, portuguese) {
        (1, false) => "1 more new pull request".to_owned(),
        (_, false) => format!("{count} more new pull requests"),
        (1, true) => "Mais 1 novo pull request".to_owned(),
        (_, true) => format!("Mais {count} novos pull requests"),
    };
    let body = rest
        .iter()
        .map(|alert| alert.key.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    (title, bounded_label(&body))
}

/// The alerts shown on their own and those folded into the summary.
fn burst<T>(alerts: &[T]) -> (&[T], &[T]) {
    alerts.split_at(alerts.len().min(PULL_ALERT_BURST))
}

/// The memory key of one pull request alert: an item is announced once per reason.
fn pull_alert_key(alert: &crate::github_inbox::PullAlert) -> String {
    let reason = match alert.reason {
        crate::github_inbox::AlertReason::Opened => "opened",
        crate::github_inbox::AlertReason::ReviewRequested => "review",
    };
    format!("pull:{reason}:{}", alert.key.to_ascii_lowercase())
}

/// New pull requests (ADR-050 amendment, 2026-10-18): only items the last open pull request
/// list confirms, each at most once per run (the renderer remembers them across restarts in
/// `githubNotified`); the first `PULL_ALERT_BURST` alert on their own, the rest in one summary.
fn announce_pulls(
    app: &AppHandle,
    state: &Arc<AppState>,
    items: Vec<PullRequestRef>,
) -> Result<()> {
    if items.len() > PULL_ALERT_REQUEST_LIMIT {
        return Err(notification_error("Too many pull requests."));
    }
    let requests: Vec<_> = items
        .into_iter()
        .map(|item| (item.repository, item.number, item.reason))
        .collect();
    let alerts = crate::github_inbox::pull_alerts(&requests);
    let (prefs, portuguese) = {
        let data = state.data.lock();
        (
            data.settings.notifications.clone(),
            data.settings.locale == "pt-BR",
        )
    };
    // Consume even with the switch off: turning it on later does not replay them.
    let fresh: Vec<_> = {
        let notification_state = app.state::<NotificationState>();
        let mut seen = notification_state.seen.lock();
        alerts
            .into_iter()
            .filter(|alert| remember(&mut seen, &pull_alert_key(alert)))
            .collect()
    };
    if fresh.is_empty() || !prefs.pull_requests {
        return Ok(());
    }
    let foreground = app
        .get_webview_window("main")
        .is_some_and(|window| window.is_focused().unwrap_or(true));
    let eligible = !foreground || prefs.foreground;
    let (single, rest) = burst(&fresh);
    let mut notices: Vec<Notice> = single
        .iter()
        .map(|alert| {
            let (title, body) = pull_text(alert, portuguese);
            Notice {
                created_at: chrono::Utc::now().timestamp_millis(),
                id: pull_alert_key(alert),
                session_id: String::new(),
                kind: Kind::PullRequest,
                title,
                body,
                pull: Some(alert.key.clone()),
                detail: None,
            }
        })
        .collect();
    if !rest.is_empty() {
        let (title, body) = pull_summary(rest, portuguese);
        notices.push(Notice {
            created_at: chrono::Utc::now().timestamp_millis(),
            id: format!("pull:summary:{}", uuid::Uuid::new_v4()),
            session_id: String::new(),
            kind: Kind::PullRequest,
            title,
            body,
            pull: None,
            detail: None,
        });
    }
    for notice in notices {
        // Paired phones get it too (ADR-082); a tap opens the app.
        crate::remote::notify(&notice.title, &notice.body, "", None, None);
        if prefs.toasts {
            let _ = app.emit("notification-activity", &notice);
        }
        #[cfg(not(all(target_os = "macos", feature = "desktop")))]
        if eligible {
            window_alert(app, &notice, &prefs, choice(&prefs, Kind::PullRequest).1);
        }
        #[cfg(all(target_os = "macos", feature = "desktop"))]
        if prefs.system && eligible {
            let app = app.clone();
            crate::runtime::async_runtime::spawn(async move {
                if let Err(error) = platform::show(&app, &notice).await {
                    tracing::debug!(%error, "system pull request alert unavailable");
                }
            });
        }
    }
    #[cfg(all(target_os = "macos", feature = "desktop"))]
    if prefs.sounds && eligible {
        let sound = choice(&prefs, Kind::PullRequest).1;
        let _ = app.run_on_main_thread(move || {
            let _ = platform::play(sound);
        });
    }
    #[cfg(not(all(target_os = "macos", feature = "desktop")))]
    let _ = eligible;
    Ok(())
}

pub fn install(app: &AppHandle) {
    app.manage(NotificationState::default());
    #[cfg(all(target_os = "macos", feature = "desktop"))]
    platform::install(app);
}

/// Final admission happens on the main thread after the OS permission lookup.
/// Hold native ownership while submitting; a resolved request or removal cannot
/// race this gate. Explicit tests have no session and do not impersonate work.
/// The headless host has no notification center of its own: the window that drives it plays the
/// sound and shows the banner (`native_notify.rs`), by these rules.
#[cfg(not(all(target_os = "macos", feature = "desktop")))]
fn window_alert(app: &AppHandle, notice: &Notice, prefs: &Preferences, sound: Sound) {
    let _ = app.emit(
        "notification-native",
        serde_json::json!({
            "title": notice.title,
            "body": notice.body,
            "sound": prefs.sounds.then_some(sound),
            "system": prefs.system,
            "foreground": prefs.foreground,
        }),
    );
}

#[cfg(all(target_os = "macos", feature = "desktop"))]
pub(super) fn deliver_if_current(app: &AppHandle, notice: &Notice, deliver: impl FnOnce()) -> bool {
    let state = app.state::<Arc<AppState>>();
    let data = state.data.lock();
    if state.ensure_running().is_err() {
        return false;
    }
    let foreground = app
        .get_webview_window("main")
        .is_some_and(|window| window.is_focused().unwrap_or(true));
    if !current_notice(&data, notice, foreground) {
        return false;
    }
    deliver();
    true
}

#[cfg(any(all(target_os = "macos", feature = "desktop"), test))]
fn current_notice(data: &crate::models::AppData, notice: &Notice, foreground: bool) -> bool {
    let prefs = &data.settings.notifications;
    if notice.kind == Kind::PullRequest {
        return prefs.system && prefs.pull_requests && (!foreground || prefs.foreground);
    }
    if notice.session_id.is_empty() {
        return true;
    }
    let Some(session) = data
        .sessions
        .iter()
        .find(|session| session.id == notice.session_id)
    else {
        return false;
    };
    data.projects
        .iter()
        .any(|project| project.id == session.project_id)
        && if notice.kind == Kind::Reminder {
            reminder_key(session).as_deref() == Some(notice.id.as_str())
        } else {
            candidates(session).contains(&(notice.id.clone(), notice.kind))
        }
        && prefs.system
        && choice(prefs, notice.kind).0
        && (!foreground || prefs.foreground)
}

#[cfg_attr(feature = "desktop", tauri::command)]
pub async fn notification_action(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    action: Action,
) -> Result<Permission> {
    state.ensure_running()?;
    match action {
        Action::Preview { sound } => {
            admit_time(
                &app.state::<NotificationState>().previewed,
                Duration::from_millis(300),
            )?;
            #[cfg(all(target_os = "macos", feature = "desktop"))]
            {
                let (tx, rx) = tokio::sync::oneshot::channel();
                app.run_on_main_thread(move || {
                    let _ = tx.send(platform::play(sound));
                })
                .map_err(|_| notification_error("Sound preview unavailable."))?;
                rx.await
                    .map_err(|_| notification_error("Sound preview unavailable."))??;
            }
            #[cfg(not(all(target_os = "macos", feature = "desktop")))]
            {
                let _ = sound;
                return Err(notification_error(
                    "Notification sounds are unavailable on this host.",
                ));
            }
        }
        Action::Request {} => {
            let notification_state = app.state::<NotificationState>();
            if notification_state
                .requesting
                .swap(true, std::sync::atomic::Ordering::AcqRel)
            {
                return Err(notification_error(
                    "Notification permission request is already open.",
                ));
            }
            #[cfg(all(target_os = "macos", feature = "desktop"))]
            platform::request().await;
            notification_state
                .requesting
                .store(false, std::sync::atomic::Ordering::Release);
        }
        Action::Test {} => {
            admit_time(
                &app.state::<NotificationState>().tested,
                Duration::from_secs(3),
            )?;
            let settings = state.data.lock().settings.clone();
            let portuguese = settings.locale == "pt-BR";
            let notice = Notice {
                pull: None,
                detail: None,
                created_at: chrono::Utc::now().timestamp_millis(),
                id: uuid::Uuid::new_v4().to_string(),
                session_id: String::new(),
                kind: Kind::Completion,
                title: "Sirus Code".into(),
                body: if portuguese {
                    "Teste de notificação. Seus avisos de atividade aparecerão aqui."
                } else {
                    "Notification test. Your activity alerts will appear here."
                }
                .into(),
            };
            #[cfg(all(target_os = "macos", feature = "desktop"))]
            {
                platform::show(&app, &notice).await?;
                if settings.notifications.sounds {
                    let sound = settings.notifications.completion_sound;
                    app.run_on_main_thread(move || {
                        let _ = platform::play(sound);
                    })
                    .map_err(|_| notification_error("Sound preview unavailable."))?;
                }
            }
            #[cfg(not(all(target_os = "macos", feature = "desktop")))]
            {
                let _ = notice;
                return Err(notification_error(
                    "System notifications are unavailable on this host.",
                ));
            }
        }
        Action::Status {} => {}
        Action::PullRequests { items } => announce_pulls(&app, state.inner(), items)?,
        Action::Settings {} => {
            #[cfg(all(target_os = "macos", feature = "desktop"))]
            app.run_on_main_thread(platform::open_settings)
                .map_err(|_| notification_error("Notification service unavailable."))?;
        }
    }
    #[cfg(all(target_os = "macos", feature = "desktop"))]
    {
        platform::permission().await
    }
    #[cfg(not(all(target_os = "macos", feature = "desktop")))]
    {
        Ok(Permission::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phone_alerts_lead_with_the_conversation_and_never_need_a_line_break() {
        let (title, body) = phone_alert(
            Kind::Completion,
            true,
            "newlex",
            "Novo Astro",
            Some("De nada! 👍"),
        );
        assert_eq!(title, "Novo Astro");
        assert_eq!(body, "✓ Concluída · newlex — De nada! 👍");
        assert!(!body.contains('\n'));
        // A conversation titled like its project shows the name once.
        let (title, body) = phone_alert(Kind::Permission, false, "newlex", "newlex", Some("  "));
        assert_eq!(
            (title.as_str(), body.as_str()),
            ("newlex", "⚠︎ Approval needed")
        );
        let (title, body) = phone_alert(Kind::Failure, false, "api", "", None);
        assert_eq!((title.as_str(), body.as_str()), ("api", "✕ Failed"));
        assert_eq!(phone_status(Kind::Question, true).1, "perguntas para você");
    }

    #[test]
    fn a_session_named_like_its_project_shows_the_name_once() {
        assert_eq!(notice_body("dp-inchurch", "dp-inchurch"), "dp-inchurch");
        assert_eq!(
            notice_body("dp-inchurch", "Ajustes do rateio"),
            "dp-inchurch · Ajustes do rateio"
        );
    }
    fn session() -> Session {
        serde_json::from_value(serde_json::json!({
            "id": "session", "title": "Work", "projectId":"project", "agent":"codex", "status":"running",
            "createdAt":"2026-10-02T00:00:00Z", "lastActivityAt":"2026-10-02T00:00:00Z",
            "worktree":{"path":"/workspace", "branch":"main", "isolated":false}, "lastError":null,
            "messages":[{"id":"user1", "sessionId":"session", "role":"user", "content":"private prompt", "createdAt":"2026-10-02T00:00:00Z", "streaming":false}]
        })).unwrap()
    }
    #[test]
    fn only_settled_turns_notify_and_each_user_turn_is_distinct() {
        let mut session = session();
        for status in [
            SessionStatus::Running,
            SessionStatus::Starting,
            SessionStatus::Stopped,
            SessionStatus::Idle,
        ] {
            session.status = status;
            assert!(candidates(&session).is_empty());
        }
        session.status = SessionStatus::Completed;
        let first = candidates(&session);
        assert_eq!(first.len(), 1);
        assert_eq!(first[0].1, Kind::Completion);
        session.messages[0].id = "user2".into();
        assert_ne!(first, candidates(&session));
        session.last_error = Some("Failure".into());
        assert!(candidates(&session).is_empty());
        // A failed turn alerts once, as a failure (ADR-086).
        session.status = SessionStatus::Failed;
        let failed = candidates(&session);
        assert_eq!(failed.len(), 1);
        assert_eq!(failed[0].1, Kind::Failure);
        assert!(failed[0].0.contains(":failed:"));
    }
    #[test]
    fn phone_alerts_say_what_they_are_about() {
        let mut session = session();
        session.status = SessionStatus::Waiting;
        session
            .pending_requests
            .push(crate::models::PendingRequest {
                request_id: "r1".into(),
                generation: "g".into(),
                turn_id: "t".into(),
                item_id: "i".into(),
                kind: PendingRequestKind::Command {
                    command: "vercel deploy --prod\n--yes".into(),
                    cwd: None,
                    reason: None,
                },
            });
        assert_eq!(
            detail(&session, "s:g:t:r1", Kind::Permission, true).as_deref(),
            Some("$ vercel deploy --prod")
        );
        session.last_error = Some("Usage limit reached".into());
        assert_eq!(
            detail(&session, "", Kind::Failure, true).as_deref(),
            Some("Usage limit reached")
        );
        assert_eq!(
            detail(&session, "s:g:t:missing", Kind::Permission, true),
            None
        );
    }
    #[test]
    fn pending_identity_includes_generation_and_classifies_real_questions() {
        let mut session = session();
        session.status = SessionStatus::Waiting;
        let mut request = crate::models::PendingRequest {
            request_id: "request".into(),
            generation: "generation1".into(),
            turn_id: "turn".into(),
            item_id: "item".into(),
            kind: PendingRequestKind::UserInput { questions: vec![] },
        };
        session.pending_requests.push(request.clone());
        let first = candidates(&session);
        assert_eq!(first[0].1, Kind::Question);
        request.generation = "generation2".into();
        request.kind = PendingRequestKind::Command {
            command: "private command".into(),
            cwd: None,
            reason: None,
        };
        session.pending_requests = vec![request];
        assert_ne!(first[0].0, candidates(&session)[0].0);
        assert_eq!(candidates(&session)[0].1, Kind::Permission);
        session.pending_requests.clear();
        assert!(candidates(&session).is_empty());
        assert!(!first[0].0.contains("private prompt"));
    }
    #[test]
    fn final_delivery_gate_rejects_resolved_removed_disabled_and_foreground_events() {
        let mut data = crate::models::AppData::default();
        data.projects.push(crate::models::Project {
            id: "project".into(),
            name: "Project".into(),
            path: "/workspace".into(),
            added_at: "time".into(),
            last_opened_at: "time".into(),
            look: Default::default(),
            appearance: None,
            scripts: Default::default(),
        });
        let mut session = session();
        session.status = SessionStatus::Completed;
        let (id, kind) = candidates(&session).remove(0);
        data.sessions.push(session);
        let notice = Notice {
            pull: None,
            detail: None,
            id,
            kind,
            session_id: "session".into(),
            title: "Task completed".into(),
            body: "Project · Work".into(),
            created_at: 0,
        };
        assert!(current_notice(&data, &notice, false));
        assert!(!current_notice(&data, &notice, true));
        data.settings.notifications.foreground = true;
        assert!(current_notice(&data, &notice, true));
        data.settings.notifications.system = false;
        assert!(!current_notice(&data, &notice, false));
        data.settings.notifications.system = true;
        data.settings.notifications.completion = false;
        assert!(!current_notice(&data, &notice, false));
        data.settings.notifications.completion = true;
        data.sessions[0].status = SessionStatus::Stopped;
        assert!(!current_notice(&data, &notice, false));
        data.sessions[0].status = SessionStatus::Completed;
        data.sessions[0].messages[0].id = "new-user-turn".into();
        assert!(!current_notice(&data, &notice, false));
        data.sessions[0].messages[0].id = "user1".into();
        data.projects.clear();
        assert!(!current_notice(&data, &notice, false));
        data.sessions.clear();
        assert!(!current_notice(&data, &notice, false));
    }
    #[test]
    fn a_reminder_is_current_until_the_session_is_snoozed_again_or_opened() {
        let mut data = crate::models::AppData::default();
        data.projects.push(crate::models::Project {
            id: "project".into(),
            name: "Project".into(),
            path: "/workspace".into(),
            added_at: "time".into(),
            last_opened_at: "time".into(),
            look: Default::default(),
            appearance: None,
            scripts: Default::default(),
        });
        let mut session = session();
        session.status = SessionStatus::Completed;
        assert_eq!(reminder_key(&session), None);
        session.snooze_reminder_at = Some("2026-10-09T09:00:00Z".into());
        let id = reminder_key(&session).unwrap();
        data.sessions.push(session);
        let notice = Notice {
            pull: None,
            detail: None,
            id,
            kind: Kind::Reminder,
            session_id: "session".into(),
            title: "Lembrete".into(),
            body: "Project · Work".into(),
            created_at: 0,
        };
        // Reminders follow the channels, not the completion switch.
        data.settings.notifications.completion = false;
        assert!(current_notice(&data, &notice, false));
        assert!(!current_notice(&data, &notice, true));
        data.sessions[0].snoozed_until = Some("2026-10-10T09:00:00Z".into());
        assert!(!current_notice(&data, &notice, false));
        data.sessions[0].snoozed_until = None;
        data.sessions[0].snooze_reminder_at = None;
        assert!(!current_notice(&data, &notice, false));
        assert_eq!(copy(Kind::Reminder, true), "Lembrete");
    }
    #[test]
    fn closed_preferences_migrate_and_round_trip() {
        let prefs: Preferences = serde_json::from_str("{}").unwrap();
        assert_eq!(prefs, Preferences::default());
        for sound in ["glass", "ping", "pop", "submarine", "tink", "hero"] {
            let prefs: Preferences = serde_json::from_value(
                serde_json::json!({"permissionSound":sound,"permissions":false}),
            )
            .unwrap();
            let stored = serde_json::to_value(&prefs).unwrap();
            assert_eq!(stored["permissionSound"], sound);
            assert_eq!(stored["permissions"], false);
        }
        assert!(serde_json::from_value::<Preferences>(
            serde_json::json!({"completionSound":"/etc/passwd"})
        )
        .is_err());
        assert!(serde_json::from_value::<Action>(
            serde_json::json!({"type":"test","title":"spoofed"})
        )
        .is_err());
    }
    #[test]
    fn deduplication_is_bounded_and_distinguishes_turns() {
        let mut seen = VecDeque::new();
        assert!(remember(&mut seen, "session:turn1"));
        assert!(!remember(&mut seen, "session:turn1"));
        assert!(remember(&mut seen, "session:turn2"));
        for index in 0..400 {
            remember(&mut seen, &format!("event:{index}"));
        }
        assert_eq!(seen.len(), 256);
    }
    #[test]
    fn event_switches_and_sounds_are_independent() {
        let prefs = Preferences {
            permissions: false,
            question_sound: Sound::Hero,
            ..Default::default()
        };
        assert_eq!(choice(&prefs, Kind::Permission), (false, Sound::Ping));
        assert_eq!(choice(&prefs, Kind::Question), (true, Sound::Hero));
    }

    fn pull_alert(
        number: u32,
        reason: crate::github_inbox::AlertReason,
    ) -> crate::github_inbox::PullAlert {
        crate::github_inbox::PullAlert {
            key: format!("owner/repo#{number}"),
            repository: "owner/repo".into(),
            number,
            author: "ana".into(),
            title: "Fix the login".into(),
            reason,
        }
    }

    #[test]
    fn pull_request_alerts_are_localized_capped_and_follow_their_switch() {
        use crate::github_inbox::AlertReason::{Opened, ReviewRequested};
        assert_eq!(
            pull_text(&pull_alert(12, Opened), false),
            (
                "ana opened a pull request in owner/repo".into(),
                "#12 Fix the login".into()
            )
        );
        assert_eq!(
            pull_text(&pull_alert(12, Opened), true).0,
            "ana abriu um pull request em owner/repo"
        );
        assert_eq!(
            pull_text(&pull_alert(12, ReviewRequested), false).0,
            "ana asked for your review in owner/repo"
        );
        assert_eq!(
            pull_text(&pull_alert(12, ReviewRequested), true).0,
            "ana pediu sua revisão em owner/repo"
        );
        // One key per item and reason: a later review request is a new alert.
        assert_ne!(
            pull_alert_key(&pull_alert(1, Opened)),
            pull_alert_key(&pull_alert(1, ReviewRequested))
        );
        let alerts: Vec<_> = (1..=5).map(|n| pull_alert(n, Opened)).collect();
        let (single, rest) = burst(&alerts);
        assert_eq!((single.len(), rest.len()), (3, 2));
        assert_eq!(
            pull_summary(rest, false),
            (
                "2 more new pull requests".into(),
                "owner/repo#4, owner/repo#5".into()
            )
        );
        assert_eq!(pull_summary(&rest[..1], true).0, "Mais 1 novo pull request");
        let (single, rest) = burst(&alerts[..2]);
        assert_eq!((single.len(), rest.len()), (2, 0));
        // The system banner gate follows the switch, the system channel and focus.
        let mut data = crate::models::AppData::default();
        let notice = Notice {
            created_at: 0,
            id: "pull:opened:owner/repo#1".into(),
            session_id: String::new(),
            kind: Kind::PullRequest,
            title: "t".into(),
            body: "b".into(),
            pull: Some("owner/repo#1".into()),
            detail: None,
        };
        assert!(current_notice(&data, &notice, false));
        assert!(!current_notice(&data, &notice, true));
        data.settings.notifications.pull_requests = false;
        assert!(!current_notice(&data, &notice, false));
        assert!(serde_json::from_value::<Action>(serde_json::json!({
            "type": "pullRequests",
            "items": [{"repository": "owner/repo", "number": 1, "reason": "reviewRequested"}]
        }))
        .is_ok());
        assert!(serde_json::from_value::<Action>(serde_json::json!({
            "type": "pullRequests",
            "items": [{"repository": "owner/repo", "number": 1, "reason": "opened", "title": "spoofed"}]
        }))
        .is_err());
        assert_eq!(
            serde_json::to_value(&notice).unwrap()["kind"],
            "pullRequest"
        );
    }
}
