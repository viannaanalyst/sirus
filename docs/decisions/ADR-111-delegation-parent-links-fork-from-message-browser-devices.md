# ADR-111: Delegating to other models, parent links, fork from a message, browser devices

**Status:** Accepted (2026-10-11)

## Context

The owner asked for a chat to hand a task to another provider or model without any subagent setup, child conversations linked back to their parent, and a browser with a device mode, crash recovery, recording and picture in picture. Also: forking from an exact message, refusing to undo while another session edits the same folder, skipping colour on huge code blocks, sending a running subagent to the background and answering Team helpers' approvals from the coordinator. The owner asked for all of them (the agent-device panel and passkeys stay out).

## Decision

- **Delegate to other models, always on**. Four tools sit on every session's MCP bridge, outside the "agents manage sessions" switch:
  - `sirus_models` lists installed, enabled providers and their models (cached catalogs);
  - `sirus_delegate` starts a visible child session with only the task prompt, on any provider and model, in the caller's folder unless `newWorktree`, with the caller's approval mode;
  - `sirus_delegation_status` reads a child's state and, once it stopped, its final answer (8,000 characters), which also acknowledges it;
  - `sirus_delegation_cancel` stops a child like its Stop.

  `mode: "async"` (default) returns at once; when every task of that turn settles and the parent is free, the parent gets a short message naming them, unless it already read them or the person stopped it (ADR-101). `mode: "wait"` blocks up to 300 s (Codex allows 330 s per tool call) and keeps the wake-up when it times out. Children carry `delegated_by {sessionId, turn, notify, settled, delivered}`; sessions started with `sirus_create_session` get the link without `notify`. Starts reuse the `sirus_*` bounds (4 per turn, two levels deep). The reply that delegated shows each task as a live card (provider, model, state, Stop), and Settings → MCP says the feature exists.
- **Parent breadcrumb**. Delegated or agent-started sessions, team helpers and sessions an Astro launched show "Delegated by / Started by / Helper of / Started by Astro › parent › this conversation" at the top; the parent opens on click. Side chats sit beside their parent and forks keep their own line.
- **Team approvals in the coordinator**. A helper's pending requests render under its row in the coordinator's progress card with the same approval card as in its own chat.
- **Fork from a message**. Each settled answer has "Fork from here": a new session with everything up to that answer and nothing after it (the existing `fork_session` snapshot).
- **Undo checks the folder**. Undo is refused while another session's agent is starting, running or waiting in the same folder (canonical path), naming it.
- **Huge code blocks are not coloured**. Above 120,000 characters or 3,000 lines a code block renders as plain text.
- **Subagent to the background**. A running Claude subagent card offers "Background" (the CLI's `background_tasks` control request, its Ctrl+B) and "Stop" (`stop_task`, now for foreground subagents too). A backgrounded subagent is tracked like one launched with `run_in_background` (ADR-097). Child rows now keep `source`, the launching tool call id.
- **Browser**:
  - *Device mode*: 17 presets, width × height fields (240–3840), rotate and close. The page lays out at the device's CSS size: the native view gets a centred, scaled-down frame and `pageZoom` equal to the scale.
  - *Resize rails* outside the frame on the left, right, bottom and both bottom corners (the frame stays centred); arrow keys resize by 10 px, Shift by 50.
  - *Crash recovery*: when a tab's WebContent process ends, the tab reloads by itself up to three times in 30 s, waiting 250 ms, 500 ms, then 1 s.
  - *Recording*: native JPEG snapshots (15 fps, up to 1280 px wide) are drawn on a canvas that MediaRecorder encodes (MP4, else WebM). Each second is streamed to `Movies/Sirus/Browser recording <time>.<ext>` (at most 1 GiB, 10 minutes), shown in Finder at the end.
  - *Picture in picture*: the active tab's view moves into an always-on-top, resizable panel on every Space; closing it or the pane's button puts it back.
  - Passkeys are still out: WKWebView needs Apple's managed browser entitlement.

## Consequences

- Delegation works from any provider whose session has the Sirus MCP bridge (Codex, Claude, OpenCode). Children are ordinary sessions: they can be opened, continued and archived. A child's link survives restarts, but the Stop cascade still uses memory-only launch records.
- The wake-up message is a visible user turn in the parent, like an Astro's report notice.
- Recording runs in the renderer and depends on WebKit's MediaRecorder; frame rate follows how fast snapshots return.
- A floating tab is not shown in the pane, which offers to bring it back.

## Addendum: delegated tasks, T3 Code style (2026-11-09)

A delegated task that waits on an approval or a question now shows that request inside its row in the parent's task card, with the approval card its own tab uses (`pending_request_card`), and answering there resolves it in the task. The person is still the only one who answers questions. The host also wakes the parent once per waiting episode (`DelegatedBy.asked`, cleared when the request is answered or the task settles) with a short prompt that names the task and points to `sirus_answer_approval` (decline always, accept only with full access) or to telling the person; a busy or stopped parent is not woken and gets it when it settles. Delegated tasks no longer list as conversations: the sidebar nests them, collapsed, under their parent row behind an "N tarefas" disclosure that shows a dot while one of them waits, they are never restored as tabs on launch, and a task whose parent is gone stays in the list. Two or more tasks of one reply form a foldable group ("3 tarefas delegadas · 2 concluídas") and each row shows its elapsed time. Delegating never opens a tab by itself.
