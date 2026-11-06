# ADR-051: Scheduled automations start agent turns

**Status:** Accepted. *Amended 2026-10-07:* the Automations page was removed; the engine remains for Astro habits (ADR-069). Superseded by ADR-172 (Tasks and scheduled tasks removed).

## Context

Until now every agent turn started from an explicit Send. Goals never continue on their own (ADR-041), queued requests advance only after an explicit submit (ADR-042), and restored drafts never send. The rule was a conservative default: an unattended turn can edit files, run commands and use quota while nobody is watching. That raises the stakes of prompt injection from project content.

The owner asked for Automations: agent runs on a schedule while the app is open.


The owner removed the "explicit Send only" rule for this feature.

## Decision

An **automation** has a name, a prompt (≤64 KiB), a saved project, a provider and optional model, an approval profile, an optional planning (read-only) flag, a workspace (new isolated worktree by default, or the project checkout), a schedule and an enabled flag. Automations persist in `AppData.automations` (at most 50). Each one keeps a bounded run history (`AppData.automation_runs`, 50 per automation).

Schedules are a closed enum:

- `manual`, `once` (future instant);
- `hourly` (minute), `daily` / `weekdays` (HH:MM), `weekly` (weekday + HH:MM), in local time;
- `interval` (15 min to 7 days).

There is no free-form cron.

**Running.**

- **Timer:** one native timer, `automations::start`, sleeps until the earliest next run (re-reading the clock at most hourly) or until a definition changes. It runs only while Sirus is open. This is a timer to a known instant, not a polling loop.
- **Each run:** a due run creates a normal session with `create_session_locked` and admits its first turn through the same `send_prompt` path as the Send button, so every native validation and approval rule applies. The run uses the automation's typed approval profile.
  - **Ask (the default):** tool approvals wait for the person, and the run stays paused until they answer.
  - **Full:** requires `acknowledgeFullAccess` on every save and cannot combine with planning.
- **Visibility:** run sessions appear in the sidebar like any session and trigger the usual notifications.

**Safety rules.**

- **Overlap:** a run is skipped if the previous run of the same automation is still active.
- **Repeated failures:** after 3 consecutive failed runs the automation pauses itself and shows why.
- **Missed runs:** runs missed while the app was closed start once if they are under 60 minutes late. Older ones are recorded as missed and rescheduled, so opening the app never starts a burst of runs.
- **Run now:** starts one run on demand, even when the automation is paused.
- **Project removal:** removing a project removes its automations. Deleting an automation keeps its past sessions.

**IPC security review.**

- **Command:** one closed command, `automation_action` (`list`, `upsert`, `delete`, `setEnabled`, `runNow`), with `deny_unknown_fields`.
- **Validation:**
  - names are printable and ≤200 characters;
  - prompts are 1–64 KiB without NUL;
  - the project must exist and the provider must be enabled;
  - model IDs are a bounded charset that cannot look like a flag;
  - schedules come from the closed enum.
- **No new surface:** renderer input never names a path, argv, flag or command. No new process kind, capability or credential access is added. Process spawn stays in the existing provider adapters.
- **Event:** one payload-free event, `automations-changed`, tells the renderer to refresh. The renderer also adopts native-created sessions it learns about from `session-updated`.

*Amended 2026-10-07:* **Automations page removed; the engine remains for Astro habits.** Once Astros existed (ADR-069), habits covered scheduled work and the standalone page no longer made sense, so the owner removed it: its rail item, ⌘K entry and main view are gone, and saved rail settings drop the `automations` ID on load, like the retired `drafts` ID. The scheduler, `automation_action`, run history, failure pause and run notifications are unchanged, and every automation is now a habit (`astroId`). Standalone automations from before are never deleted: on load they are paused (`enabled: false`, no next run) and kept in `AppData.automations`, and `automation_action` refuses to create one, resume one or Run now one ("Scheduled work now lives in Astro habits."). They stay out of the Inbox, which lists failing habits instead.

*Amended 2026-10-08:* **Habit runs have a one-hour working limit.** Time waiting on an approval does not count; overdue runs are stopped and recorded as failed, counting toward the failure pause. Details in ADR-069.

## Consequences

- **Positive:**
  - Recurring work (triage, dependency checks, summaries) runs on its own, in isolated worktrees, under the same approval rules as manual turns.
- **Negative:**
  - Agents can now act without a fresh click.
  - With Auto or Full, a run can edit files and use quota while nobody is watching.
  - The project checkout option shares the person's working copy.
- **Accepted trade-off:** runs only happen while the app is open. A schedule is not a guarantee: sleep, quitting or misses older than 60 minutes skip runs.

## Alternatives considered

- **Keeping explicit-Send only and offering reminders.** Rejected by the owner.
- **A background daemon or launch agent so runs happen with the app closed.** Rejected for now: it adds a long-lived process and a new trust boundary.
- **Free-form cron.** Deferred. The closed schedules cover the common cases and validate trivially.
- **Agent-created automations.** Deferred. Only the person creates automations.
