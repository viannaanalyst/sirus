//! Snooze a conversation with a reminder (ADR-103). A snoozed session keeps
//! `snoozedUntil` (RFC 3339); the renderer hides it from tabs and session lists.
//! One native timer sleeps until the earliest snooze, so it fires while the
//! window is hidden: the session comes back, is marked by `snoozeReminderAt`
//! (unread until opened) and a "Reminder" alert goes through the notifications
//! path. Reminders missed while the app was closed fire once at launch.

use std::sync::{Arc, OnceLock};
use std::time::Duration;

use crate::runtime::{AppHandle, Manager, State};
use chrono::{DateTime, Utc};
use tokio::sync::Notify;

use crate::commands::{native_task, AppState};
use crate::error::{Error, Result};
use crate::models::AppData;

/// The timer re-reads the wall clock at least this often: its sleep is monotonic and
/// does not advance while the Mac sleeps, so a reminder due during system sleep would
/// otherwise come late by up to the time slept.
const MAX_SLEEP: Duration = Duration::from_secs(10 * 60);
/// The farthest a conversation can be snoozed.
const MAX_AHEAD_DAYS: i64 = 366;

static WAKE: OnceLock<Arc<Notify>> = OnceLock::new();

/// Re-arms the timer after a snooze changed.
fn rearm() {
    if let Some(notify) = WAKE.get() {
        notify.notify_one();
    }
}

pub(crate) fn parse(at: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(at)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

fn stamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

/// The earliest moment a snoozed session must come back. An unreadable time counts as due.
pub fn earliest(data: &AppData) -> Option<DateTime<Utc>> {
    data.sessions
        .iter()
        .filter_map(|session| session.snoozed_until.as_deref())
        .map(|at| parse(at).unwrap_or(DateTime::<Utc>::MIN_UTC))
        .min()
}

/// Brings back every session whose time has come: the snooze is cleared and the
/// reminder recorded, so the session reads as unread until it is opened. Returns their ids.
pub fn wake_due(data: &mut AppData, now: DateTime<Utc>) -> Vec<String> {
    let mut woken = vec![];
    for session in &mut data.sessions {
        let Some(until) = session.snoozed_until.as_deref() else {
            continue;
        };
        if parse(until).is_some_and(|at| at > now) {
            continue;
        }
        session.snoozed_until = None;
        session.snooze_reminder_at = Some(stamp(now));
        woken.push(session.id.clone());
    }
    woken
}

/// Snoozes a session until `until`, or with `None` returns it now and clears its reminder
/// (opening a returned conversation acknowledges it this way). Active sessions refuse.
pub fn apply(
    data: &mut AppData,
    session_id: &str,
    until: Option<&str>,
    now: DateTime<Utc>,
) -> Result<()> {
    let session = data
        .sessions
        .iter_mut()
        .find(|session| session.id == session_id)
        .ok_or_else(|| Error::not_found("session not found"))?;
    let Some(until) = until else {
        session.snoozed_until = None;
        session.snooze_reminder_at = None;
        return Ok(());
    };
    if session.status.is_active() {
        return Err(Error::new(
            "invalid",
            "A conversation that is running or waiting cannot be snoozed.",
        ));
    }
    let at = parse(until)
        .filter(|at| *at > now && *at <= now + chrono::Duration::days(MAX_AHEAD_DAYS))
        .ok_or_else(|| Error::new("invalid", "Choose a time in the future, within a year."))?;
    session.snoozed_until = Some(stamp(at));
    session.snooze_reminder_at = None;
    Ok(())
}

/// Snoozes a session until `until` (RFC 3339), or returns it now with `None`.
/// Replies with the session's metadata.
pub async fn snooze_session(
    app: AppHandle,
    state: State<'_, Arc<AppState>>,
    session_id: String,
    until: Option<String>,
) -> Result<serde_json::Value> {
    let state = state.inner().clone();
    let meta = native_task(move || {
        let mut data = state.data.lock();
        state.ensure_running()?;
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        let previous = (
            session.snoozed_until.clone(),
            session.snooze_reminder_at.clone(),
        );
        apply(&mut data, &session_id, until.as_deref(), Utc::now())?;
        if let Err(error) = crate::persist::save(&state.data_path, &data) {
            if let Some(session) = data.sessions.iter_mut().find(|item| item.id == session_id) {
                (session.snoozed_until, session.snooze_reminder_at) = previous;
            }
            return Err(error);
        }
        let session = data
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .ok_or_else(|| Error::not_found("session not found"))?;
        // Other windows (a paired phone) learn of it like any session change.
        crate::transcript_view::emit(&app, session);
        crate::transcript_view::session_meta(session)
    })
    .await?;
    rearm();
    Ok(meta)
}

/// Starts the single snooze timer: it sleeps until the earliest snooze (re-armed when one
/// changes), wakes what is due, and alerts. The first pass, at launch, fires missed reminders.
pub fn start(app: AppHandle) {
    let notify = WAKE.get_or_init(|| Arc::new(Notify::new())).clone();
    crate::runtime::async_runtime::spawn(async move {
        let state = app.state::<Arc<AppState>>().inner().clone();
        loop {
            if state.ensure_running().is_err() {
                return;
            }
            let (woken, next) = {
                let mut data = state.data.lock();
                let woken = wake_due(&mut data, Utc::now());
                (woken, earliest(&data))
            };
            if !woken.is_empty() {
                if let Err(error) = state.persist() {
                    tracing::warn!(%error, "snooze wake was not saved");
                }
                {
                    let data = state.data.lock();
                    for session in data.sessions.iter().filter(|item| woken.contains(&item.id)) {
                        crate::transcript_view::emit(&app, session);
                    }
                }
                for id in &woken {
                    crate::notifications::remind(&app, &state, id);
                }
            }
            let wait = next
                .map(|at| (at - Utc::now()).to_std().unwrap_or(Duration::ZERO))
                .unwrap_or(MAX_SLEEP)
                .min(MAX_SLEEP);
            if next.is_none() {
                // Nothing is snoozed: sleep until a snooze arrives.
                notify.notified().await;
                continue;
            }
            tokio::select! {
                _ = tokio::time::sleep(wait) => {}
                _ = notify.notified() => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Session, SessionStatus};

    fn at(value: &str) -> DateTime<Utc> {
        parse(value).unwrap()
    }

    fn session(id: &str, status: SessionStatus) -> Session {
        let mut session: Session = serde_json::from_value(serde_json::json!({
            "id": id, "title": "Work", "projectId": "project", "agent": "codex", "status": "idle",
            "createdAt": "2026-10-09T00:00:00Z", "lastActivityAt": "2026-10-09T00:00:00Z",
            "worktree": {"path": "/workspace", "branch": "main", "isolated": false}, "lastError": null
        }))
        .unwrap();
        session.status = status;
        session
    }

    fn data() -> AppData {
        AppData {
            sessions: vec![
                session("a", SessionStatus::Completed),
                session("b", SessionStatus::Idle),
                session("c", SessionStatus::Stopped),
            ],
            ..AppData::default()
        }
    }

    #[test]
    fn the_timer_sleeps_until_the_earliest_snooze() {
        let mut data = data();
        assert_eq!(earliest(&data), None);
        data.sessions[0].snoozed_until = Some("2026-10-10T09:00:00.000Z".into());
        data.sessions[1].snoozed_until = Some("2026-10-09T15:30:00-03:00".into());
        assert_eq!(earliest(&data), Some(at("2026-10-09T18:30:00Z")));
        // A corrupt time is due at once rather than stuck forever.
        data.sessions[2].snoozed_until = Some("tomorrow".into());
        assert_eq!(earliest(&data), Some(DateTime::<Utc>::MIN_UTC));
    }

    #[test]
    fn waking_clears_the_snooze_and_marks_the_session_unread() {
        let mut data = data();
        data.sessions[0].snoozed_until = Some("2026-10-09T12:00:00.000Z".into());
        data.sessions[1].snoozed_until = Some("2026-10-09T13:00:00.000Z".into());
        let now = at("2026-10-09T12:00:00Z");
        assert_eq!(wake_due(&mut data, now), vec!["a".to_string()]);
        assert_eq!(data.sessions[0].snoozed_until, None);
        assert_eq!(
            data.sessions[0].snooze_reminder_at.as_deref(),
            Some("2026-10-09T12:00:00.000Z")
        );
        assert!(data.sessions[1].snoozed_until.is_some());
        assert_eq!(data.sessions[1].snooze_reminder_at, None);
        // Once woken, a session is not woken (or announced) again.
        assert!(wake_due(&mut data, now).is_empty());
        // Opening it (return now with no time) clears the reminder.
        apply(&mut data, "a", None, now).unwrap();
        assert_eq!(data.sessions[0].snooze_reminder_at, None);
    }

    #[test]
    fn reminders_missed_while_closed_fire_once_at_launch() {
        let mut data = data();
        data.sessions[0].snoozed_until = Some("2026-10-01T09:00:00.000Z".into());
        data.sessions[2].snoozed_until = Some("2026-10-05T09:00:00.000Z".into());
        data.sessions[1].snoozed_until = Some("2026-12-01T09:00:00.000Z".into());
        // Saved and loaded again, as after a quit.
        let json = serde_json::to_value(&data.sessions[0]).unwrap();
        assert_eq!(json["snoozedUntil"], "2026-10-01T09:00:00.000Z");
        let mut data: AppData = AppData {
            sessions: data
                .sessions
                .iter()
                .map(|session| {
                    serde_json::from_value(serde_json::to_value(session).unwrap()).unwrap()
                })
                .collect(),
            ..AppData::default()
        };
        let launch = at("2026-10-09T08:00:00Z");
        assert_eq!(
            wake_due(&mut data, launch),
            vec!["a".to_string(), "c".to_string()]
        );
        assert!(wake_due(&mut data, launch).is_empty());
        assert_eq!(earliest(&data), Some(at("2026-12-01T09:00:00Z")));
    }

    #[test]
    fn active_sessions_cannot_be_snoozed_and_times_must_be_ahead() {
        let mut data = data();
        let now = at("2026-10-09T12:00:00Z");
        for status in [
            SessionStatus::Starting,
            SessionStatus::Running,
            SessionStatus::Waiting,
        ] {
            data.sessions[0].status = status;
            assert!(apply(&mut data, "a", Some("2026-10-09T13:00:00Z"), now).is_err());
            assert_eq!(data.sessions[0].snoozed_until, None);
            // Returning now is always allowed.
            assert!(apply(&mut data, "a", None, now).is_ok());
        }
        assert!(apply(&mut data, "b", Some("2026-10-09T11:00:00Z"), now).is_err());
        assert!(apply(&mut data, "b", Some("2028-10-09T11:00:00Z"), now).is_err());
        assert!(apply(&mut data, "b", Some("not a time"), now).is_err());
        assert!(apply(&mut data, "missing", None, now).is_err());
        data.sessions[1].snooze_reminder_at = Some("2026-10-09T10:00:00.000Z".into());
        apply(&mut data, "b", Some("2026-10-09T15:00:00-03:00"), now).unwrap();
        assert_eq!(
            data.sessions[1].snoozed_until.as_deref(),
            Some("2026-10-09T18:00:00.000Z")
        );
        assert_eq!(data.sessions[1].snooze_reminder_at, None);
        // Older state files have neither field.
        let json = serde_json::to_value(&data.sessions[2]).unwrap();
        assert!(json.get("snoozedUntil").is_none() && json.get("snoozeReminderAt").is_none());
    }
}
