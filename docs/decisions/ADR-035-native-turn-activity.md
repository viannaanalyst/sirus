# ADR-035: Turn activity is bounded, read-only native history

**Status:** Accepted.

## Context

A compact model/working/elapsed line and grouped tool/child rows require facts that assistant prose and the mutable session model cannot establish. The renderer cannot declare completion, infer successful edits, count hidden reasoning as work, or acquire child process authority. Interrupted restarts also must not count offline time as work.

## Decision

- An optional `Message.activity` belongs to the native-admitted assistant response. It retains the provider/model choice at admission, optionally enriches the model from native initialization, and records native start/end/wait boundaries. Existing transcripts without this field remain unchanged. Pending callbacks remain live-only as in ADR-009.
- Session lifecycle transitions own pause/resume/finality. UI time is a display-only second counter, paused while waiting, offscreen or document-hidden; it never polls a provider, grants an answer or represents a deadline. Restart recovery caps an interrupted turn at its last retained native observation, rather than reopening time. Wall-clock recovery is approximate and makes no uninterrupted CPU-time claim.
- At most 128 native tool/child identities are retained per response. Closed kinds/states and bounded labels/model IDs cross existing `session-updated` snapshots. Updates dedupe, preserve omitted metadata/status, and do not reopen completed tools on repeated start snapshots. Observed child identities can work again after new native input. Later updates to retained items remain available after the bound; omitted new items set a truncation marker.
- Codex observation occurs only after matching the active parent thread and turn. Command/file/tool item envelopes and explicit collaboration child states or V2 child lifecycle envelopes are read-only observations. A successful spawn identity can appear with unknown child state; completion of a spawn/wait call is never child completion. Spawn model metadata applies only to that single spawned identity, never other children in a wait. The [official Codex presentation code](https://github.com/openai/codex/blob/main/codex-rs/tui/src/multi_agents.rs) distinguishes spawn metadata, repeated child input and V2 child lifecycle.
- Claude observes top-level tool-use/result envelopes and explicitly reported local-agent task starts. Later task notifications update only known local child IDs; shell/background tasks and nested child transcript frames do not become independent agents. ACP observes offered tool-call/update kinds/status after fresh prompt admission, excluding load replay. Other adapters show truthful provider/model/time/status without invented tool or child details.
- Tool arguments, commands, tool output, prompts, child reports and hidden reasoning are not duplicated into activity metadata. Group counts describe observed calls, not files changed or undisclosed work. Unknown native outcomes stay unreported; parent success never promotes unfinished child/tools to success. Child details offer retained metadata only, with no child stop/send, Sirus session creation, worktree ownership or extra IPC.
- Activity snapshots remain live; persistence shares the existing event-driven one-second checkpoint with text streaming. Lifecycle and finality still persist immediately. No new commands, events, frontend APIs, capabilities or credentials.
- The approved Line 01 uses existing typography/theme/motion roles. Native questions keep offered single answers and generation-bound one-shot delivery, with explicit step navigation/submission. The shared Shortcut tooltip uses compact themed surfaces and keyboard chips, preserving deliberately opted-out tooltips.

## Amendment (2026-10-03): child steps and timing

Each child row may also keep its own observed work and its native time window, so the trail can show a subagent as a branch: name, model, step count, elapsed time and its steps.

- **Retained data:** at most 20 child steps, the newest ones. Earlier steps are only counted in `hiddenSteps`, and a turn retains at most 400 steps across all children.
  - Steps use the same closed kinds, states and generic labels as parent tools.
  - `startedAt`/`endedAt` record when the row was first observed and when it reached a settled native state. A child that works again reopens its window.
  - When the turn or the child finishes, running steps settle as stopped or unreported, never as succeeded.
- **Claude source:** `task_started.tool_use_id` links a local agent task to its delegation call. Complete child messages tagged with that `parent_tool_use_id` become the child's steps. The delegation call's own row and result fold into the child.
  - The only input read from the call is its `model` choice, and only from `Agent`/`Task` calls.
  - Child prose, prompts and tool arguments or output are still excluded.
- **Codex source:** `item/started` and `item/completed` envelopes from non-parent threads on the turn's own app-server become steps of the child row `agent:<thread>`.
  - Nested spawns inside a child do not become new rows.
  - Work that arrives before its child row is buffered, at most 32 observations, oldest dropped, and discarded at turn end. Work from a thread that never becomes a child row is never shown.
- **OpenCode source:** ACP `kind: "think"` (its `task` tool) is a child row named by `rawInput.description`. ACP forwards no child steps, so these rows have none (see ADR-011).
- **Live-only routing:** the routing links (`source`, `parent`) are never serialized.
- **Unchanged:** no child control, child session, IPC or capability is added.

## Consequences

Activity survives native persistence with the response it describes and remains separate from model memory. Observability varies by provider/version; names, model metadata, nested tools and final states may be unavailable. Historical imported responses do not gain synthetic activity, and child control remains outside this change.

Deterministic tests cover timing, recovery, retention bounds, replay/deduplication, partial metadata, child reactivation, excluded content and shell-task exclusion. UI checks cover localized rendering, themes, offered answer validation and explicit approval actions. These fixtures do not claim a paid live-provider integration test.
