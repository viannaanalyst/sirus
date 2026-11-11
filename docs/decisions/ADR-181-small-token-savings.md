# ADR-181: Small token savings

**Status:** Accepted (2026-10-28).

## Context

The reference app, another open-source client, another open-source client and Zeron each carry a few cheap token savers. Before adding any, we checked what Sirus already does: compaction before a send after the prompt cache expired (ADR-057, ADR-074, ADR-157), a bounded handoff recap (ADR-028), bounded session tools (ADR-076), tool output stored apart (ADR-147), and usage history that counts each response once (ADR-141). Only two ideas were missing; the others were already covered or already tighter.

## Decision

- **Rotation without a model call (new, off by default).** Chat behavior → "Start a fresh session when the context is nearly full" (`rotateAtContextLimit`). When a Claude or Codex conversation has a native thread of its own, is idle, and its reported context reaches 80% of a known window, the next send drops the native thread and starts a fresh provider session. `rotation.rs` builds the brief from the transcript, with no model: the last 2 exchanges word for word (the user's message cut to 2,000 characters, the reply to 3,000, keeping head and tail), then one line of at most 280 characters per earlier exchange, newest kept within 3,000 characters for those lines. Streaming, empty and system messages are left out. The brief is armed as a pending handoff with `rotated: true`, so the existing plumbing (ADR-028, ADR-101) delivers it once, gives it back if the turn fails before the provider takes the prompt, and a retry carries it again. The sent user message carries `rotation { used, window }`, and the native window draws a "Contexto renovado · nova sessão (162k de 200k)" separator above it, like the compaction marker (`handoff.rs`, added 2026-11-11: the window had not drawn it until then). Idle time and restarts never trigger it; only the reported context does. `/compact`, team turns, side chats, team workers, conversations with a pending handoff and providers other than Claude and Codex are left alone. The full transcript stays in the app.
- **Budgets (new).** `token_budget.rs` counts bytes as a pessimistic token estimate. History sent to a provider that lacks the native thread (the text-replay providers, `conversation_prompt`) is now capped at `min(16,000, window − used − current message − max(16,000, window / 4))` tokens instead of 32 KiB; without a known window the 16,000 cap applies, and attachments on the message weigh 8,192 (image) or 4,096 (file) each. A delegated task's final answer returned to its parent was already cut at 8,000 characters (tighter than the 12,000 considered); it now also says so in the text and sets `resultTruncated`. Delegation returns one result per call, so there is no batch to cap.
- **Already covered, no change.**
  - *Ask before resuming a cold, large session.* "Compact and send" (ADR-074, ADR-157) already detects exactly this case (Claude, at least 100,000 tokens or 70% of a known window, cache expired by its reported `cacheExpiresAt`), says so on the send button and compacts first; Shift-click sends as is. A modal with the same three choices would duplicate it, so none was added.
  - *Tool guidance cadence.* The browser, computer and visuals guidance (MCP `instructions`, `--append-system-prompt`) is given once when the provider process starts and sits in the cached system prompt. Nothing is repeated per turn, so there is no counter to add.
  - *Bounded cross-conversation reads.* `sirus_read_session` returns at most 20 messages (default 6) of 2,000 characters each and `sirus_list_sessions` at most 40 sessions. That is tighter than the 40-default, 500-maximum considered, so it stays.
  - *Usage accounting.* `usage_history.rs` already counts a Claude response once by message id and request id across every log file, so forks and resumes do not double count (`claude_counts_a_repeated_response_once`).

## Consequences

- **Positive:** a long Claude or Codex conversation can keep going at a fraction of the per-turn cost without a summarizing model call; a replayed history can no longer exceed the receiving model's budget.
- **Negative:** rotation is lossy by design: the fresh session knows two exchanges in full and earlier ones in a line, and loses the provider's own tool results and reasoning. That is why it is opt-in. The brief can also repeat text the provider's own compaction would have summarized better; use `/compact` when that matters.
- **Estimates:** bytes overstate English text roughly three to four times, so the history cap is conservative.

## Alternatives considered

- **Rotating on idle time or at startup.** Rejected: only a full context justifies losing the native thread.
- **Asking a model to write the brief.** Rejected: it costs a call, and a summary of a summary drifts.
- **A cold-session dialog in addition to "Compact and send".** Rejected as a duplicate (see above).

**Rotation switched on, at 250,000 tokens.** `rotateAtContextLimit` now defaults to on, and a database from before gets it once (store migration 9). The trigger is `min(250_000, 80% of the reported window)` (`rotation::threshold`), so a conversation on the 1M window still rotates at 250k instead of 800k.
