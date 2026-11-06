# ADR-180: Small performance wins from Zeron, the reference app and another open-source client

**Status:** Accepted (2026-10-28).

## Context

Three similar apps publish performance findings: The reference app batches agent output per frame in
Rust, another open-source client measured style recalculation cost of `has-*` selectors, Zeron found a settled
animation still spinning (92.8% CPU, 8.3% once stopped). Sirus already had much of this
(ADR-047, 048, 099, 143, 147; one rAF flush of `agent-output`; bounded transcript cache; ambient
gate for animations). Each item was checked against the tree first; this ADR records what was
added and what was left alone.

## Decision

1. **Native output batching (new).** `src-tauri/src/output_batch.rs` sits between the provider
   readers (`agent.rs`, `codex.rs`) and the `agent-output` event. Per session: the first event
   after idle is sent at once; later events in the same 16 ms window merge into one event
   (same message and stream, contiguous UTF-16 offsets; the first snapshot's `message.content`
   grows with it); the batch leaves when the window ends, or at once at 1 MiB. A non-contiguous
   event sends the waiting batch first. `transcript_view::emit_from` (every `session-updated`:
   approvals, questions, status, turn end) and the end of a turn call `output_batch::flush`
   first, so those events are never delayed and never overtake text; the turn's own drain of the
   readers already ran before that flush, so the last output leaves immediately (far inside the
   500 ms budget). The core `Batcher` takes the clock as an argument and is unit tested.
2. **Renderer cadence (tightened).** The store already applied output once per frame. It now goes
   through `src/lib/output-scheduler.ts`: the visible conversation (selected, or a split pane)
   applies at most every 33 ms on an animation frame; other conversations, and everything while
   the document is hidden, every 250 ms; selecting a session, changing the split or showing the
   window flushes at once. An application that changes nothing no longer replaces `sessions`.
   Sidebar identity was already handled by `selectSessionsMeta` (ADR-126) and is unchanged.
3. **`has-*` audit.** Replaced with data attributes: `.transcript-shell:has(.message-trail)` ->
   `[data-trail]` (set from `hasMessageTrail`, counted with early exit), `.conversation-row:has(.header-tab-status)`
   -> `[data-status]` (one per sidebar row), `.sidebar-card-title:has(.sidebar-row-action)` -> a class,
   and the message actions' `has-[[aria-expanded=true]]` -> the popover open states lifted into
   `MessageActions`. Kept, with reasons: `:has(:focus-visible)` on sidebar rows and the composer
   (focus-visible has no equivalent on the row; they re-evaluate on focus changes only),
   `.subagent-card:has(> .subagent-card-head:hover)` (child combinator, hover only), and
   settings-only pages.
4. **Animation clocks.** The audit found most loops already gated by `ambientActive()` or the
   document visibility (avatars, Astro icons, runner, provider scene, activity timers). Changed:
   `useSmoothText` no longer runs an animation frame loop once the shown text caught up (it
   restarts with new text), and the 1 Hz elapsed counters of the working panel, background tasks and
   subagent team share one timer (`src/lib/shared-clock.ts`), which runs only while subscribed
   and the document is visible. Text-effect and spinner timers (15 fps steps) are CSS.
5. **Memory bounds.** The closed-transcript cache already evicts least recently opened first with
   an estimated size (no serialization); its limits go from 8 conversations / 32 MiB to 6 / 12
   MiB (the native side already keeps 6). Terminal scrollback already defaults to 2000 lines
   (1000 to 10 000, a setting). Attachment images are files on disk (the `sirus-thumb`
   scheme); no JavaScript or native image cache holds their bytes, and the disk store refuses
   new drafts at 256 MiB rather than evicting a draft the owner still needs, so no 64 MiB cache
   was added.

## Consequences

- A token stream costs about 60 IPC events a second per session at most, instead of one per token.
- Background conversations lag the visible one by up to 250 ms and catch up when shown.
- A conversation that is not selected shows text 250 ms late in the sidebar-driven views that
  read messages (rare); status and approvals are unaffected.
