# ADR-157: Claude's cache expiry, and compacting idle conversations before it

**Status:** Accepted (2026-10-21).

## Context

"Compact and send" (ADR-057) compacts a heavy Claude conversation before a message that follows a long pause, so the request does not resend the whole uncached context. It guessed the pause from the last activity: 70 minutes. But the prompt cache lasts 1 hour on a subscription and only 5 minutes with an API key, and its lifetime starts when a request starts, not when the reply ends. Another open-source client also offers compacting an idle conversation shortly before its cache expires, so coming back costs no wait.

## Decision

- **The expiry, from Claude.** `claude.rs` notes the start of each main-agent request (`message_start`) and reads the lifetime from the usage's `cache_creation` split (`ephemeral_1h_input_tokens` → 1 hour, `ephemeral_5m_input_tokens` → 5 minutes; a request served wholly from cache keeps the last known lifetime). Each main `assistant` reading carries `ContextUsage.cacheExpiresAt` (Unix ms); readings without one keep the last known value.
- **Compact and send** uses that expiry when known and falls back to the 70-minute guess otherwise.
- **Idle compaction (beta, off by default).** Chat behavior's "Compact idle Claude conversations before the cache expires" starts `idle_compaction.rs` passes every 30 seconds (desktop and headless host). An idle Claude conversation with a native thread, at least 200,000 tokens, no pending request, not archived, snoozed, auxiliary or a team worker, compacts once within the two minutes before its expiry, through `send_prompt("/compact")`.

## Consequences

- Coming back to a heavy conversation compacts exactly when the cache is gone, including the 5-minute API-key case.
- With idle compaction on, a conversation that is never resumed still spends one compaction per idle spell; that is why it is opt-in.
