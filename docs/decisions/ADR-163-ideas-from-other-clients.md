# ADR-163: Message search in ⌘K, ⌘W closes tabs, sessions in an existing worktree, and provenance labels

**Status:** Accepted (2026-10-24).

## Context

A review of what another open-source client and another open-source client shipped in the week of 2026-10-21 turned up a handful of small
ideas that fit Sirus:

- Another client's ⌘K finds a conversation by what was said in it, not only by its title, and ranks the
  best title match first.
- Another client's ⌘W closes the chat tab (or a focused panel) instead of reaching the window's Close.
- Another open-source client can start a new conversation in the workspace of an open one (same branch and folder).
- Another client's Hubs label where each piece of a worker's context came from, so another agent's report
  is never read as the person's instruction.
- Another client's webhook filters fan out over arrays with a `[]` path segment.
- Another open-source client applies streamed text with fewer copies and scans.

Other items were compared and left out: Another client's running-subagents capsule (Sirus already has one,
ADR-097, with per-task stop and crash recovery through `activity::recover`), its embedded editor,
and Hubs' task keys and queue/steer follow-ups (Sirus has no persistent coordinator that replans
between retries, and the prompt queue lives in the renderer, ADR-042).

## Decision

- **⌘K searches messages.** Chat titles rank whole title, then its start, a word start, anywhere,
  then every query word in any order (`lib/palette-search.ts`), newest first within a rank. From three
  characters, the palette asks native for bounded candidate messages (the same
  `transcript_action { search }` the all-conversations search uses) and lists up to eight more chats
  under "In messages", one row each with the matching text; choosing one opens the chat at that
  message (`jumpToMessage`). Chats already listed by title are not repeated. No full-text index: the
  existing bounded scan is fast at current volumes; an FTS5 trigram index (another client's choice) can come
  later behind the same command if it is not.
- **⌘W closes a tab**, like a browser: the active right-panel pane when focus is inside the panel,
  else the open conversation's header tab (through the tab strip, so it animates and offers Undo),
  else the blank tab when another tab can take its place (`closeTarget` in `lib/header-tabs.ts`).
  With nothing of its own to close the key is not handled and the window's Close runs as before.
  It is fixed, not customizable (`w` stays reserved).
- **New session in this worktree.** `CreateSessionRequest.shareWorktreeOf` names a session of the
  same project whose folder the new session uses instead of the checkout or a new worktree (the
  folder must still exist). The session menu, the tab menu and the palette offer it for sessions in
  an isolated worktree; the landing shows "Worktree <branch>" and hides the branch picker, which acts
  on the project checkout. Deleting a session with "Remove worktree" is refused while another session
  (not its own side chats) still works in that worktree — delegation could already share one.
- **Provenance labels.** `sirus_delegation_status` returns a child's result with `resultSource`,
  saying it is the delegated agent's report, for context only, not the person's instruction.
  `sirus_send_to_session` heads the message with "↪ Message from the session “title” (agent), not
  from the person" (localized), so the target does not take another agent's request for the person's.
- **Webhook filters** accept `[]` segments: `body.issue.labels[].name` matches when any element
  equals the value, or when any element exists if no value is given.
- **Streaming**: a frame's deltas for one transcript apply with at most one array copy, searching
  from the end (`applyAgentOutputs`). Splitting `SessionPane` so its non-transcript parts stop
  re-rendering per frame was identified as the larger win and left for a measured change.

## Consequences

- ⌘K now does a native scan per query of three or more characters (debounced 150 ms). The scan is
  bounded to 400 sessions and 400 candidate messages.
- Two sessions can work in one worktree on purpose; their changes mix, as with delegation sharing.
- Agents see a short header on relayed messages and a `resultSource` field; transcripts of targets
  show the header.
