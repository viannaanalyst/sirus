# ADR-057: Context usage and compaction

**Status:** Accepted

## Context

Long conversations fill the model's context window, and answers degrade without warning. The owner asked for a context indicator and `/compact`.

## Decision

**Reading.** `Session.contextUsage { used, window }` stores the latest reading the provider reports. Nothing is inferred.

| Provider | `used` | `window` |
| --- | --- | --- |
| Codex | `thread/tokenUsage/updated` → `tokenUsage.last.totalTokens` (the running total grows across compactions and is ignored) | `modelContextWindow` |
| Claude Code | Main-agent `assistant` events → `input + cache_creation + cache_read + output` tokens | Successful `result`: the largest `modelUsage[*].contextWindow` |

- **Missing window:** a reading without one keeps the last window known.
- **Model change:** changing the model clears the reading.
- **Other providers:** they report nothing and show no indicator.

**Indicator.** The composer shows a ring once both numbers are known; it turns amber at 75% and red at 90%. Its popover shows the percentage, used/window tokens and Compact now.

**Compaction.**

- **Trigger:** Compact now, or typing `/compact`, sends the standalone `/compact`.
- **Codex:** with an existing thread, `/compact` calls `thread/compact/start` and follows the compaction turn from `turn/started` to `turn/completed`.
- **Claude Code:** `/compact` is sent as its own stream-json user message **without** `client_composed` (see below). The CLI runs the compaction and reports `system/status` (`compacting`), then `system/compact_boundary` (its `post_tokens` is the rebuilt context, read as the new reading), then a result with `local_command: "compact"`. The usage on that result is ignored, because it describes the summarizer call. A `status` with `compact_result: "failed"` fails the turn with the CLI's reason.
- **Other providers:** refused before anything is sent, with a message (`context.cannotCompact`). Typing `/compact` in their composer never sends the text.
- **Refusals:** no native conversation yet (`context.nothingToCompact`), a turn still running (`context.wait`), a `/compact` carrying attachments or team/debug mode, and a fork's first turn before it has a native thread. The host checks the same rules (`transcript::compaction_refusal`, the Codex turn start).
- **Literal text never reaches the model:** `/compact` is never prefixed, wrapped (goal, handoff, side chat, skills) or steered. `steer_turn` refuses it, and the composer queues any message sent during a compaction.
- **In the transcript:** while the compaction runs, the reply reads "Compacting context…"; afterwards an empty reply reads "Context compacted".

## Revision 2026-10-19: `/compact` arrived as text

**Root cause.** Claude turns carry `client_composed: true`, so the CLI delivers their text as written, with no slash-command dispatch (the CLI's own schema describes it as: "no slash-command dispatch"; the 2.1.301–2.1.303 binaries all skip `/command` dispatch for it). `/compact` was therefore sent through that same path and the model answered it as text. Checked against the installed CLI (2.1.303, Haiku, a throwaway directory): with `client_composed` the reply was "I can't run `/compact`"; without it the CLI compacted (`compact_boundary`, `pre_tokens` 3558 → `post_tokens` 638). The reference apps agree: Another client's `compactThread` sends a plain `/compact` turn, and its adapter reads `compact_boundary` and `compact_result`.

**Codex** needs no protocol change: `thread/compact/start` on the thread starts a `contextCompaction` turn (checked against codex-cli 0.163.0: `turn/started`, `item/started` `contextCompaction`, `thread/tokenUsage/updated` with the rebuilt `last` total, `turn/completed`).

**Fix.** The CLI command is recognised on the host (`codex::is_compact`, trimmed and exact), sent without `client_composed`, and never wrapped. The store action `compactSession` (used by the context popover and by typing `/compact`) refuses unsupported cases before sending.

## Consequences

- **Positive:** the person sees when a conversation is getting heavy and can compact it without starting over.
- **Negative:** readings exist only for Codex and Claude, and only after a turn.
- **Security:** no new IPC or capability. `/compact` is an ordinary explicit Send. Codex compaction uses one fixed protocol method, bound to the session's own thread.

## Alternatives considered

- **Estimating tokens locally.** Rejected: the UI must show real provider data.
