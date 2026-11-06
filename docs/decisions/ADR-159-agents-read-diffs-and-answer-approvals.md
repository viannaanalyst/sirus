# ADR-159: Agents read a session's diff and answer its approvals

**Status:** Accepted (2026-10-21). Extends ADR-076.

## Context

An agent coordinating others (`sirus_create_session`, `sirus_send_to_session`) could read their messages but not their code, and a session it started that stopped on an approval waited for the person even when the coordinator could judge it. Another open-source client added both to its MCP tools.

## Decision

- **`sirus_session_diff`** (read-only): a session workspace's uncommitted changes against HEAD: a per-file summary (`--stat`), the new untracked files and the unified diff, cut to 60,000 characters; an optional relative `path` (never absolute or with `..`) narrows it to one file. Git runs through `bounded_run` (no hooks or external filters, time and output limits).
- **`sirus_session_status`** lists `pendingApprovals` (request id, type, a short detail and reason); questions are not listed.
- **`sirus_answer_approval`** (counts as mutating, so planning callers cannot use it): only in a session whose `delegatedBy` is the caller; `decline` always, `accept` only when the caller itself runs with full access, so an agent never grants more than it already has; questions stay with the person. It goes through the same `respond_agent_request` path as the card.

## Consequences

- A coordinator can review a worker's changes and unblock it without the person.
- Sessions the person started, or another agent started, cannot be answered by a third agent.
