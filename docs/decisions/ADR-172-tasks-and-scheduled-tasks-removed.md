# ADR-172: Tasks and scheduled tasks removed

**Status:** Accepted (2026-10-28). Supersedes ADR-051, ADR-052, ADR-110, ADR-142 and ADR-161.

## Context

Sirus grew a personal to-do list (Tasks, ADR-052, with agent tools in ADR-110) and, on top
of it, prompts that run by themselves: on a clock (scheduled tasks, ADR-142) or when another
service posts to a private URL (webhook tasks, ADR-161). The owner decided the product is about
sessions, agents, worktrees and reviewable changes, and that these features pull it away from
that. They also carried a wide surface: a timer started with the app, a public webhook endpoint
on the remote-access server, and eight agent tools.

## Decision

Remove the whole feature set.

- The Tasks page, the Scheduled view, the "scheduled" line in the composer, the rail item
  (`tasks`), the command palette entry and every related string and style are gone.
- The back end loses the tasks module, the scheduled-tasks engine and its timer (desktop and
  headless), the webhook module, the `task_action` and `scheduled_task_action` IPC commands,
  the `tasks-changed` and `scheduled-tasks-changed` events, and `AppData.tasks` and
  `AppData.scheduledTasks`.
- The remote-access server no longer serves `POST /hooks/<task>/<secret>`.
- The Sirus MCP server no longer lists `sirus_create_task`, `sirus_list_tasks`,
  `sirus_update_task`, `sirus_schedule_task`, `sirus_list_scheduled_tasks`,
  `sirus_update_scheduled_task`, `sirus_delete_scheduled_task` or `sirus_run_scheduled_task_now`.
  The session tools stay.
- `state.db` migration 7 drops the `tasks` and `scheduled_tasks` tables. Older JSON or fallback
  data that still has `tasks` or `scheduledTasks` loads without error: unknown fields are
  ignored, and a saved rail order that names `tasks` drops it.

ADR-051, ADR-052, ADR-110, ADR-142 and ADR-161 stay as history, marked as superseded.

## Consequences

- Existing tasks and scheduled tasks are deleted by the migration. Nothing runs by itself any
  more, and existing webhook URLs answer 404.
- A person who relied on schedules needs an outside scheduler (cron, CI) that starts a session
  another way.
- The app has no timer that starts agent turns on its own, and the remote-access server no
  longer has a route that is reached with a URL secret instead of a paired device.
- The rail has four customizable items: Home, Inbox, Archived and Pull requests.
