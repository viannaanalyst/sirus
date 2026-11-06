# ADR-063: Usage-limit notice and resume at reset

**Status:** Accepted

## Context

When a provider's account limit is spent, the turn failed with a generic error and queued requests stayed paused. The owner asked to see when the limit resets and to continue by itself.

## Decision

**Detection.** It is native and uses only provider reports.

- **Codex:** `codexErrorInfo: "usageLimitExceeded"` on the `error` notification or the failed turn. The reset is the latest `resetsAt` among `account/rateLimits/updated` windows at or over 100%.
- **Claude:** a `rate_limit_event` with `status: "rejected"` that is not paid by extra usage, or a failed result whose text says the limit was reached. Its `resetsAt` is the reset.

The turn fails with code `usage_limit`, and `Session.usageLimit = { resetsAt }` is persisted. The next send clears it.

**Notice.** Above the composer it shows "Usage limit reached · Resets at 15:16 · in 4h 42m". When the stream gave no reset, the renderer asks the existing usage probe (`provider_usage`) for the spent window. The countdown moves by minutes.

**Resume.** Resuming is explicit. "Resume at reset" arms an automatic continue for that notice (memory-only). One renderer timer waits for the earliest armed reset plus 30 seconds, rechecking at least every 5 minutes because timers stall while the Mac sleeps. It then sends "Continue from where you left off." with the session's last admitted execution. A queue that the limit paused follows once that turn succeeds. After the reset, "Resume" sends the same turn by hand, and the notice can be dismissed.

## Consequences

- **Positive:** a long task survives a usage limit without the person watching the clock.
- **Negative:** the automatic continue is the second exception, after automations, to "turns start from an explicit Send". (Since the 2026-10-18 amendment one setting arms every notice; it is off by default and each notice can still be cancelled.)

## Alternatives considered

- **Arming by default.** Rejected: it would start turns without the person's action.
- **Native timers.** Rejected: the renderer already owns the queue, and the resume needs no new IPC.

## Amendment (2026-10-18): automatic by default

The owner asked for the resume to be automatic. A new setting, `usageAutoResume` (Settings → General → Application; off by default, like the reference app the owner compared), arms every usage-limit notice whose reset is known and later than the moment the turn stopped; a reset that had already passed when the limit was reported never arms itself, so a stale report cannot loop. The notice shows "Resuming at reset" with Cancel; cancelling records that reset for the session (`usageResumeCancelled`, kept in local storage so it survives a restart) and a later limit with a new reset arms again. With the setting off, "Resume at reset" still arms one notice by hand. The timer now rechecks every minute and again when the window regains focus or becomes visible, so a resume missed while the Mac slept or the app was in the background fires within moments of coming back; a reset that passed while the app was closed fires on the next launch. What is sent and the queue that follows are unchanged.

## Amendment (2026-10-19): a full context window is not a usage limit

Claude ends a turn whose prompt no longer fits the model's window with `terminal_reason: "blocking_limit"` and "Prompt is too long" (or the API's 400 `invalid_request_error`, "prompt is too long: N tokens > M maximum"), and Codex with `codexErrorInfo: "contextWindowExceeded"`. No reset fixes that, so such a turn is never classified as a usage limit, even after a refusing `rate_limit_event`: it sets no `Session.usageLimit`, arms no resume, and fails with code `context_window` and "The conversation no longer fits the model's context window. Run /compact or start a new conversation." (`claude::failed_result`, `codex::is_context_window_full`). A `blocking_limit` without limit evidence stays a plain failure.

## Amendment (2026-10-19): continue on another account

Each Claude and Codex conversation stays pinned to the account that started it (ADR-015), so a usage limit on one account stranded it even when the person had another account with quota left. When the provider has another account that is signed in with quota left, the notice now offers **Continue on another account**. Its popover lists the provider's other accounts with their plan and the remaining share of the most constrained window (cached `provider_usage` reads, the same as the accounts panel). Signed-out, unread and spent accounts are listed but cannot be picked (`accountChoices`, `src/lib/provider-accounts.ts`). Picking one shows progress on that row and any error inline.

Native `move_session_account { request: { sessionId, accountId } }` (`account_move.rs`; closed request) refuses a running session, an account of another provider or one that is not registered, an account that is signing in, and an account whose last usage read did not confirm a signed-in identity. It stops the session's kept process and then copies the native transcript from the current account's folder into the target's. Named accounts use their private profile; the default account uses `CLAUDE_CONFIG_DIR` or `CODEX_HOME`, else `~/.claude` or `~/.codex`.

- **Claude:** `projects/<encoded cwd>/<id>.jsonl`, found in the folder named after the working directory or else in any project folder, because the CLI shortens long names. The sidecar `projects/<encoded cwd>/<id>/` (subagent transcripts, large tool results) and `file-history/<id>/` go with it. All of it lands under the same folder names.
- **Codex:** the rollout `sessions/YYYY/MM/DD/rollout-…-<id>.jsonl`, or an archived one. The rollouts of agents it spawned (read-only `thread_spawn_edges` in the source's newest `state_<n>.sqlite`) and any `history_base` rollout go with it. All of them land in `sessions/YYYY/MM/DD/` of the target, dated from the file name. The target's index is not touched: on Codex 0.163 `thread/resume` finds a copied rollout and indexes it by itself, both in a fresh home and in one whose `state_5.sqlite` already exists.

Copies go through a flushed temporary file beside the destination. The transcript or root rollout is copied last, so a partial copy is never taken for a whole one, and a copy from an earlier move is replaced. Links are not followed, new folders are private (0700), and the copy is bounded (256 rollouts; 4096 sidecar files, six folders deep). On success the session's `providerAccountId`, its `accountBindings` entry and `nativeThread.providerAccountId` move to the target, so the next turn resumes the same native session there. If the copy is impossible (the transcript is missing, the folders cannot be resolved, or an I/O error occurs), the session drops its native thread and arms the in-session handoff recap that a provider switch uses (`arm_in_session_handoff`), so the next turn starts fresh on the new account with a summary. A session without a native session only changes account. The reply says which of these happened (`carried`, `summary`, `switched`).

The renderer (`continueOnAccount`) then disarms any armed resume and sends "Continue from where you left off." through `resumeAfterUsageLimit`, the normal `send_prompt` path, which clears the usage limit. A queue that the limit paused follows as it does after Resume. The usage limit stays on the session until that send, so a failed send still leaves Resume in the notice.

**Consequences.** A transcript now exists in both profiles. Later turns write only to the target's copy, and moving back replaces the old copy with the newer one. Each account's provider still sees the whole conversation it continues. Codex rollouts record the account that created them (`creator_account_id`), and whether a provider later treats that differently is outside Sirus's control. If it ever refuses, the conversation can still move with the handoff recap.

**Alternatives considered.** Hard or symbolic links would let two accounts append to one file, so they were rejected. Writing rows into the target's Codex SQLite index was not needed because Codex rebuilds them, and it would couple Sirus to a private schema.
