# ADR-009: Codex interaction is typed and native-owned

**Status:** Accepted; fixed approval-profile restrictions superseded by [ADR-017](ADR-017-per-turn-approval-profiles.md).

## Context

Textual follow-up context cannot preserve a vendor's exact thread, and null stdin prevents host approval and question responses. Giving an untrusted renderer a generic protocol writer would let it manufacture tool inputs, permission grants or authentication operations. The installed Codex app-server schema provides a verified native protocol while retaining the user's existing CLI login.

## Decision

- `codex.rs` owns app-server stdio, initialization, exact thread start/resume, a turn, assistant text reconciliation, pending callbacks and process cleanup. Each turn owns a fresh server process; an idle server is not retained after completion. Native thread identity is persisted with the owning Sirus session, project, canonical workspace and selected model. A mismatch or failed native resume is an error, never a latest-thread fallback or textual replay.
- Thread/turn requests explicitly use workspace-write, on-request and the user approval reviewer. The turn restricts writable roots to its validated workspace and disables network access and temporary-root exceptions. The CLI still determines actual platform sandbox enforcement; this is not a stronger OS isolation claim.
- `respond_agent_request` is the sole new IPC operation. The payload contains Sirus session, opaque callback ID, process generation, turn ID and a typed approval decision or question answers. Rust retains vendor IDs and original requests. It checks binding twice, reserves responses under the session lock, consumes each callback once and returns delivery failure explicitly.
- Approvals expose only one-request accept/decline/cancel. File approvals require reviewable native paths/diffs. Questions expose native offered options and allowed free text, bounded to one answer per question and 8 KiB per answer. Secret questions, permission amendments, persistent grants, grantRoot expansions, managed-network/stdin variants and unsupported methods are denied. There is no auth/config mutation or arbitrary tool/protocol passthrough.
- Pending callbacks are process-local. They are ignored on state deserialization and removed on native resolution, completion, cancellation and shutdown. Only exact thread identity survives restart. Renderer controls use existing Client/Transport and Arc primitives; output/status/persistence remain native facts.
- Stop sends `turn/interrupt`, waits at most two seconds for turn completion, then closes stdio and cleans the owned process group. Shutdown and stream failure retain emergency group termination. Tools that remain in the owned group are cleaned even if the server exits first.

## Consequences

- **Positive:** exact native continuation, reviewable host decisions, bounded typed IPC and no new secret store or frontend privileges.
- **Negative:** each follow-up pays server initialization/resume cost. Only the installed, verified app-server subset is supported; older CLI versions may fail compatibility checks. Non-Codex adapters retain their fallback behavior.
- **Accepted trade-off:** small native authority and no idle process ownership over maximal protocol coverage. Long-running native threads, steering, richer tool timelines and other vendors need separate scoped work.

## Alternatives considered

- Generic JSON-RPC from the webview: rejected because it expands tool, auth and permission authority.
- Drive the vendor TUI through the user PTY: rejected because it mixes unrelated lifecycle/security boundaries.
- Persist pending callback IDs: rejected because a new server process cannot honor callbacks from an earlier generation.
- Keep idle app-server processes indefinitely: deferred; exact thread resume gives continuation without introducing detached idle ownership.
