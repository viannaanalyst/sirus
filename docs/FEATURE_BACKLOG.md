# Feature backlog

Backlog written on **2026-09-30** for the overnight MVP mission; the implementation status below was updated on 2026-10-01. Track actual implementation and validation in [OVERNIGHT_PROGRESS.md](OVERNIGHT_PROGRESS.md). The [project contract](../AGENTS.md) and accepted ADRs remain authoritative.

## NOW — close the local desktop loop

The priorities and acceptance criteria below are Sirus proposals for the overnight mission.

| Priority | Proposal | Acceptance criteria | Dependency / guardrail |
| --- | --- | --- | --- |
| P0 | Reliable new-session destination | Before creation, show project, provider/model, and current checkout versus isolated worktree. The created session retains the exact selection. Canceling creates no process. | Existing Session and Worktree domain; Rust validates cwd. |
| P0 | Preserve existing model identity | Async discovery or enable/disable preference changes never replace an existing session's model. A missing or unavailable model is identified clearly; new sessions use enabled, available choices. | Model catalog and saved session state; provider-qualified model keys. |
| P0 | Honest process recovery | Reopening persisted `starting`/`running` sessions without a live owned process marks them interrupted/stopped, preserves messages, and offers a fresh prompt. Never imply provider conversation resume when unsupported. | Persistence and native lifecycle ownership; no automatic agent execution on startup. |
| P0 | Safe worktree lifecycle | Creation, Git status, diff, and terminal use the session's worktree. Explicit removal rejects active processes and dirty worktrees and explains the reason. Metadata removal never deletes a user project directory. | [ADR-004](decisions/ADR-004-git-argv-no-destructive-defaults.md); no force removal. |
| P1 | Readable live transcript | Output appears before process exit. Following the latest output works while at the bottom; scrolling upward keeps the reader's position and exposes a return-to-latest action. Stop and failure retain received output. | Existing event stream; no percentage progress or polling loop. |
| P1 | Fast provider/model navigation | Cached detection does not block Settings opening. Explicit refresh shows pending/error states. Favorites and model search include provider identity and remain usable with arrows, Enter, and Escape. | Discovery results must come from actual CLI capabilities or a documented adapter strategy. |
| P1 | Complete keyboard path | Add/open project, create session, choose model/workspace, send/stop, inspect diff, and open terminal work by keyboard. Cmd+B persists sidebar visibility; focus returns to the trigger after overlays close. | Central shortcut matcher; terminal input must retain its own editing shortcuts. |
| P1 | Review at the session boundary | Changed-file list and diff identify the active session path/branch. Refresh on relevant lifecycle/user events; selecting another session cannot retain another worktree's stale result. | Git CLI argv, request identity checks, no aggressive polling. |
| P1 | Clear provider limitation states | Installation, execution failure, unavailable catalog, and unsupported capability have distinct messages. A missing catalog never presents invented model availability. | Existing CLI authentication remains outside Sirus. |

## Current implementation status

The NOW session destination, model identity, recovery, safe removal, transcript following, catalog navigation, keyboard source paths and Session-scoped Git review are implemented and regression-tested. Actual native edits passed for Codex/Claude/OpenCode and Cursor Auto-review with sandbox enabled. Codex and Claude also passed exact vendor-native follow-ups and host approval refusal. OpenCode ACP now has exact bound continuation and native once-only edit approvals; initial-policy live fixtures passed, while final hardened-policy inference is unverified due APIError also seen with CLI run. Actual desktop fixtures cover palette/sidebar, session/worktree creation, Settings focus, PTY input/resize/close/reopen, Git/diff and Claude Write approval/follow-up. Exhaustive gesture/platform coverage remains unverified.

Per-session/project drafts now persist in owner-validated native JSON and survive graceful restart without being sent. Settings focus is contained in the existing Radix Dialog scope. Manual workspace inventory is implemented with real path/branch/HEAD/lock/prunable data and referencing Sessions; destructive actions remain explicit Session deletion, never retention cleanup.

## NEXT — after the MVP loop is validated

| Proposal | Benefit and minimum condition |
| --- | --- |
| Session search and status filters | Find running, interrupted, and completed work quickly using local metadata. No filesystem indexing is required. |
| Session recap/export | Export a user-requested local recap with provider, model, worktree, messages, and validation notes. Output must identify omissions and must not claim an agent's self-report is a passed check. |
| Extend provider-native continuation | Codex and Claude exact bound continuation are implemented and tested. OpenCode ACP negotiation, exact resume and once-only permissions are implemented; repeat final hardened-policy live acceptance when the shared provider APIError clears. Cursor lacks a verified machine approval response protocol; Grok requires vendor login. |
| Provider-reported turn metadata | Show token usage or runtime activity only when emitted by the adapter. Missing metrics stay absent; no estimated billing or fabricated progress. |
| User-reviewed handoff draft | Prepare a bounded context draft from a session for a different provider, with workspace identity and review first. Sending and actual continuation remain explicit actions. |
| Workspace inventory refinements | Inventory and linked Sessions are implemented. Any additional removal surface must preserve dirty/active checks and explicit confirmation. |

## LATER — separate scope and security decisions

Remote/mobile transport, provider plugins, agent-controlled orchestration, browser automation, external issue inboxes, scheduled runs, and commit/push/PR delivery all require separate design and security review. Competitor documentation makes these interesting future directions; it does not justify importing their privilege model into the current Tauri app.

Do not implement these during the overnight mission: no RemoteTransport, mobile host, secret storage, paid cloud, automated Git cleanup, automatic publication, or broad command-execution IPC. Keep [ADR-002](decisions/ADR-002-client-transport-boundary.md), [ADR-003](decisions/ADR-003-cli-agent-providers.md), and [ADR-007](decisions/ADR-007-pty-is-user-shell-only.md) intact.

## Evidence limitations

- No competitor runtime, authentication, throughput, accessibility, or security behavior was tested.
- No code or brand asset was copied from competing applications. Official provider assets need their own provenance check.
- The backlog ranks engineering value for Sirus; it does not rank competitors or infer unsupported CLI behavior from their integrations.
- Recheck upstream documentation before implementing a runtime-specific protocol. The local adapter and installed CLI remain the source of truth for actual availability.
