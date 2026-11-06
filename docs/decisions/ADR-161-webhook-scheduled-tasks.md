# ADR-161: Scheduled tasks that run on a webhook

**Status:** Accepted (2026-10-22). Extends ADR-142 and ADR-155. Superseded by ADR-172 (Tasks and scheduled tasks removed).

## Context

The owner wants another service to start agent work: a new Sentry issue, a GitHub event or a failed CI job should open a conversation that investigates and fixes it. Scheduled tasks (ADR-142) already run a prompt by themselves in a new or bound conversation; they only ran on the clock. Another open-source client added an "On webhook" schedule with filters and dedupe.

## Decision

- **A schedule kind.** `{ "kind": "webhook", "filters": [...], "dedupeMinutes": 60 }` beside `once`, `daily`, `weekly` and `interval`. It never runs on the clock (`next_run` is `None`), and Run now refuses it. Agents cannot create one (`sirus_schedule_task` refuses `webhook`): the URL is a credential the person handles.
- **The URL.** `POST /hooks/<task id>/<secret>` on the remote-access server (`remote.rs`, the same port and `guard`; requests without an `Origin` pass, as Cloudflare's loopback deliveries do). The secret is 32 random bytes (two v4 UUIDs), URL-safe base64, kept in the task (`webhookSecret`) and never in the renderer's snapshot. It is shown once, in the action's reply (`ScheduledTasksSnapshot.webhook`: the path and the URLs on the public route from `remote-public.json`, the Tailscale HTTPS name, and `http://127.0.0.1:<port>`), when a webhook task is made or its URL rotated (`rotateWebhook`; the old one stops working). Leaving the webhook schedule drops the secret.
- **Answers.** `scheduled_webhooks.rs` decides each delivery: 404 for an unknown task, a wrong secret (compared in constant time), a paused task or one that is not a webhook task, alike; 413 above 256 KB (axum's body limit on the route); 429 above 30 deliveries a minute per task; 202 otherwise, at once, with the run started in the background.
- **Filters.** Up to eight, all required: a value at `headers.<name>` or `body.<json path>` equals a string, or is present. A filtered delivery answers 202 and runs nothing.
- **The prompt.** `{{body}}`, `{{body.<path>}}` and `{{headers.<name>}}` in the task's prompt are replaced by the request's values; without placeholders the body (pretty JSON when it parses) is appended. Every value sits between `<webhook-data-ID>` tags with a fresh random ID, `@`, `$`, a word-initial `/` and frame tags get a word joiner, and a closing note says the framed text is untrusted external data. Values are cut to 8,000 characters, an appended body to 20,000, and the prompt to 64 KiB.
- **Dedupe and coalescing.** A delivery whose prompt (rendered with raw values) matches one taken within `dedupeMinutes` (default 60, 0 off, at most 7 days) runs nothing. One that arrives while the task's previous run still works (its new conversation is active, or its bound conversation busy) waits as the task's single follow-up, replaced by any later one, and the scheduler starts it once the task is free. Rate, dedupe and follow-ups live in memory.

## Consequences

- Sentry, GitHub or CI can start a fix in its own worktree, with the task's provider, model and approval mode.
- Anyone holding the URL can start a run with text of their choosing; the framing helps the agent tell data from instructions but does not guarantee it. No signature check (HMAC) yet.
- The URL answers only while remote access is on. A public sender needs a public route (a Cloudflare Tunnel named in `remote-public.json`) whose Access policy lets `/hooks/*` through, since a webhook sender cannot pass an Access login; Sirus does not change that configuration.
- A restart forgets the dedupe window, the rate count and a waiting follow-up.
