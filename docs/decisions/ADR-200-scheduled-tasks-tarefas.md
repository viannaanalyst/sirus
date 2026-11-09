# ADR-200: Scheduled tasks (Tarefas)

- Status: accepted
- Date: 2026-11-09

## Context

ADR-172 removed the earlier scheduled tasks together with the to-do list and the webhook entry point, because they pulled the product away from sessions, agents and reviewable changes. The owner now wants a smaller version of one idea only: a prompt that starts a conversation by itself, once or on a repeat rule, shown as a calendar. No webhook, no agent tools, no to-do list.

## Decision

- **A task is a prompt plus a rule.** `{ id, title, prompt, projectId, provider, model, effort?, schedule, enabled, lastRun?, nextRun }`, with `schedule` either `{ once: "YYYY-MM-DDTHH:MM" }` or `{ repeat: { kind: daily | weekdays | weekly, time: "HH:MM", weekdays: [0..6] } }` (0 is Monday). Times are the host's local wall-clock time, kept as naive strings. The host also keeps the last 60 runs of each task (`history`) so the calendar can show ✓ and ✗ for the past.
- **Storage follows ADR-114.** `AppData.scheduledPrompts` is one row per task in a new `scheduled_prompts` table (schema version 8), in the same save transaction as projects and review guides. The name is new on purpose: `scheduled_tasks` of ADR-142 was dropped by version 7 and held another shape. Tasks stay out of `load_state` and out of streaming checkpoints; the window reads them with `list_tasks`.
- **Commands** (`scheduled_prompts.rs`, in `commands.list`): `list_tasks`, `save_task`, `delete_task`, `set_task_enabled`, `run_task_now`. Event: `tasks-updated`, sent when a task is saved, deleted, switched or has run.
  - `list_tasks { from?, to? }` answers `{ now, tasks, occurrences }`: the tasks, the host's local "now", and the runs inside the range (`done` or `failed` from the history, `planned` from the rule, at most 120 days). The window does not repeat the schedule maths.
  - `save_task` validates the prompt, the project, the time and the weekdays, refuses a one-time task in the past, and computes `nextRun`. An edit keeps the task's runs.
- **The schedule maths are pure functions** with unit tests: `next_run` (strictly after a moment), `occurrences` (a range) and `claim_due` (what is due, and each task's next run moved past now).
- **The scheduler** wakes every minute (first pass five seconds after start-up), claims the enabled tasks whose `nextRun` has come, and runs each through `launch::launch`, the path delegation uses: a normal, visible conversation in the task's project, with its provider, model and effort, in the project checkout (no new worktree). It records `lastRun` and the history, and persists.
- **Missed runs run once.** The first pass after launch claims every due task, and claiming moves `nextRun` to the first run after now, so a task that was missed for three days runs a single time and never repeats to catch up. A task already starting is skipped.
- **Permissions.** A run starts with approval `Ask`: nobody is there to answer, so anything that needs approval waits in the conversation instead of running unattended.
- **The window.** A "Tarefas" row under Pull requests in the sidebar (optional, like the others) with a badge of the tasks that will still run. It opens a page over the window (`tasks_page.rs`): a calendar with Dia, Semana (default) and Mês, a pink "now" line, runs coloured by provider, ✓ or ✗ for past runs, paused tasks dimmed and struck through, a popover per run (rule in words, project, model, switch, "Executar agora", "Abrir última conversa", "Excluir"), and a "Nova tarefa" sheet with the prompt, project, provider and model (the composer's picker data), presets or a custom date and time, the repeat rule with weekday chips, and a live summary sentence.

## Consequences

- The scheduler runs only while Sirus runs (the host is started with the app); a run missed while it was closed happens at the next launch.
- A run's conversation is not linked back to the task except through `lastRun.sessionId`; "Abrir última conversa" fails with a message if that conversation was deleted.
- ADR-172 stays accurate for everything else it removed.
