# ADR-010: Claude approvals use native stream-json and exact resume

**Status:** Accepted; fixed approval-profile restrictions superseded by [ADR-017](ADR-017-per-turn-approval-profiles.md).

## Context

Claude's single-shot invocation cannot answer host approvals or preserve exact vendor continuation. Its native stream-json protocol differs from Codex JSON-RPC, but creating a second renderer reply channel would duplicate security and lifecycle authority. The installed CLI and official Agent SDK source verify the native control envelopes and the stdio permission callback entry point.

## Decision

- `claude.rs` owns initialization, user input, native control requests, assistant text reconciliation and result completion. It uses `--input-format stream-json`, `--permission-mode manual`, `--permission-prompts host` and `--permission-prompt-tool stdio`. It retains normal CLI authentication and configuration; there is no credential reader, permission amendment or configuration mutation.
- Each turn owns one process. Follow-ups use only `--resume=<exact native session ID>` with the persisted session/project/canonical workspace/model binding. Identity mismatch or unavailable native resume fails explicitly. Native continuation never receives textual transcript replay and never uses latest-session selection.
- The existing typed `respond_agent_request`, bounded native reply queue and monitor lifecycle are shared with Codex. Vendor wire envelopes stay adapter-specific. Native callbacks have opaque renderer IDs and remain bound to the owning Sirus session, process generation and turn. Original vendor tool input stays in Rust; acceptance echoes that exact input. The renderer cannot rewrite tool parameters or propose `updatedPermissions`.
- Built-in Bash, Read, Write, Edit, MultiEdit, Glob and Grep requests can expose their bounded original native input for one-request accept/decline/cancel. Unsupported tools, MCP callbacks, AskUserQuestion, secret input, plan/grant/configuration requests are explicitly denied. Claude questions require a separately verified question-to-answer adapter before they can be exposed. Existing CLI permissions may allow some calls without a host callback; Sirus never claims every tool must prompt or that Claude offers Codex's platform sandbox guarantees.
- Native `control_cancel_request` invalidates the matching pending callback. Completion, stop and shutdown invalidate all callbacks. Stop sends the native interrupt request, waits at most two seconds, then shared cleanup closes pipes and kills the owned process group. Native result success owns completion; renderer output/status cannot manufacture it. Background work is not retained after the owned turn ends.
- The shared NDJSON reader retains accumulated frame bytes outside cancellable read futures. Reply/cancel processing can interrupt a partial stdout read without losing bytes. Protocol lines, transcript text, callback input, identity lengths, pending count and retained callback IDs are bounded.

## Consequences

- **Positive:** reviewable native approvals and exact vendor continuity reuse one typed host authority. Disposable runtime fixtures verify workspace writes, native follow-up memory and denied writes.
- **Negative:** follow-ups initialize a new process, and Claude question/MCP interactions remain unsupported. Older CLI versions may reject verified flags.
- **Accepted trade-off:** a bounded built-in tool subset and owned per-turn processes keep authority explicit while preserving existing vendor configuration.

## Alternatives considered

- Null stdin or `--permission-prompts none`: cannot route native approvals.
- Generic renderer control envelopes or editable tool input: rejected because they expand native permission authority.
- Textual history replay alongside exact resume: rejected because it duplicates context and obscures vendor identity.
- Drive an interactive CLI through the user terminal: rejected because it mixes lifecycle and trust boundaries.
