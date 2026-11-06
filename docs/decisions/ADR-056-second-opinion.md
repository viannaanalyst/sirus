# ADR-056: Second opinion

**Status:** Superseded by ADR-198: the action was removed from the turn's menu in Sirus Native.

## Context

"Second opinion" under a finished turn: another agent reviews the work. Our handoff (ADR-028) moves the conversation to another provider instead of reviewing it. The owner asked for the review.

## Decision

**Where it starts.** Assistant message actions get a Second opinion button. Its menu lists installed, enabled providers, including the same provider with a different model; the turn's own model is not offered.

**What a pick does** (`secondOpinion` in the store):

1. Creates a session in the same project and workspace through the existing `handoff_session`, drops its recap with `dismiss_handoff`, and renames it `Second opinion · <title>`.
2. Opens it beside the source pane (ADR-053).
3. Sends one review prompt through `send_prompt`, with planning (read-only) where the provider supports it.

**The review prompt** (`src/lib/second-opinion.ts`) contains:

- the user request (≤600 characters);
- the reply (≤1,600 characters);
- the files that turn changed, from the native turn review (≤40 paths).

It asks for findings by severity, with files and lines, and tells the reviewer not to change anything. Nothing returns to the source conversation.

## Consequences

- **Positive:** a different model checks the work in one click, without the person writing the review prompt.
- **Negative:**
  - It costs a turn of the reviewer.
  - Providers without planning mode are asked, not forced, to stay read-only.
- **Security:** no new IPC, capability or permission. The person's click is the explicit Send, and approvals are unchanged.

## Alternatives considered

- **A reviewer that fixes things.** Deferred: reporting first keeps changes behind the person's next request.
- **A diff in the prompt.** Rejected for now: paths keep the prompt small, and the reviewer reads the files itself.
