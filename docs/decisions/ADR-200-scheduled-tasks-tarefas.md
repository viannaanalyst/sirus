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
- **Permissions.** A run starts with the task's approval, `Ask` by default: nobody is there to answer, so anything that needs approval waits in the conversation instead of running unattended.
- **Repeat rules** now also include `monthly` (`repeat.day`, 1 to 31; a shorter month uses its last day, `monthly_next`). A task also carries `approval` (`ask`, `auto`, `full`), the composer's access modes; a missing value is `ask`, so an unattended run still waits for approval unless the owner chose otherwise.
- **The window (revised).** "Tarefas" is a page in its own tab, like Pull requests (`TermState.tasks_tab` / `tasks_active` in `shell_terminal.rs`; ✕ or ⌘W closes it, the sidebar row opens or focuses it; it is not pinnable and has no tab menu). It opens on a **month** calendar whose header shows "Novembro de 2026", previous and next, "Hoje", the Dia | Semana | Mês segmented control (sliding accent pill) and "+ Nova tarefa". Weeks start on Sunday in Portuguese and Monday in English; a month shows only the weeks it needs. Runs are chips coloured by provider with ✓ or ✗ for the past, paused tasks struck through, and "+N" past three a day (it opens that day). Week and Day are the hour grids.
- **Keyboard** (when no modal is open): ← and → step a month, week or day; T is today; D, S and M pick Dia, Semana and Mês.
- **The modal** ("Nova tarefa" / "Editar tarefa", centred over a dimmed backdrop, Esc or ✕ closes): prompt, project, provider and model, effort (the composer's levels for the chosen model), access, presets plus date and time fields with arrow steppers, repeat (Não, Todo dia, Dias úteis, Semanal with weekday chips, Mensal), a live summary sentence, Cancelar and Salvar. Clicking a day, or an empty hour in Semana and Dia, opens it prefilled; clicking a run opens a popover (details, switch, Editar, Executar agora, Abrir última conversa, Excluir). Editing sends `save_task` with the task's id.
- **Date maths are pure and tested** (`month_grid`, `view_days`, `step_anchor`, `weekday_order`, `slot_default`, `form_parts`).

## Consequences

- The scheduler runs only while Sirus runs (the host is started with the app); a run missed while it was closed happens at the next launch.
- A run's conversation is not linked back to the task except through `lastRun.sessionId`; "Abrir última conversa" fails with a message if that conversation was deleted.
- ADR-172 stays accurate for everything else it removed.
