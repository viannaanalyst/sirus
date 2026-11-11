# ADR-209: MCP for agents outside Sirus

**Status:** Accepted (2026-11-11)

## Context

Agents inside Sirus can list, read, start and continue sessions through the `sirus_*` tools (ADR-076, ADR-111, ADR-208). Agents that run outside it (Claude Code in a terminal on the Mac, Claude Desktop, ChatGPT through a remote MCP connector) had no way in. Pairing a device gives the whole window's power, which is far more than reading a conversation or handing over a task.

## Decision

- **Tokens of their own.** The person makes a token with a name and a scope, `read` or `write`, in Settings → Conexões → "Agentes de fora (MCP)" or with `sirus-host mcp-token create <name> [--scope read|write]` (also `list`, `revoke <id>`; they work on the file, with or without a running host). The secret (`sirus_mcp_…`) is shown once, at creation; only its SHA-256 hash is kept, in `mcp-tokens.json` (`0600`, data folder, at most 32 tokens). The list shows name, scope, creation and last use (written at most once a minute). Revoking deletes the row; the file is read on every request, so it takes effect at once. The command (`mcp_token_action`) is a normal host command, so a paired device may manage tokens, as it may do everything else the window does.
- **An endpoint on the existing server, no new port.** `POST /mcp` on the host's HTTP server (both listeners, so the Mac window's loopback port, Tailscale and the public Cloudflare route all reach it) speaks MCP's Streamable HTTP with JSON replies: `initialize`, `ping`, `tools/list`, `tools/call`, notifications answered 202. `GET` and `DELETE` answer 405 (no server stream, no session). A request without a valid `Authorization: Bearer` is 401; browsers cannot use it (the existing Origin check still applies). Bodies are at most 256 KiB (413), batches are refused (400), and a token may make 120 requests and 12 changes a minute (429). Logs name the token and the tool, never the secret.
- **Tools by scope.** `read`: `sirus_list_sessions`, `sirus_read_session`, `sirus_session_status`, `sirus_models`, `sirus_list_computers`, `sirus_delegation_status`. `write` adds `sirus_create_session`, `sirus_send_to_session`, `sirus_delegate` and `sirus_delegation_cancel`. A `read` token calling a write tool gets a JSON-RPC error before anything runs. Never offered: `sirus_answer_approval`, `sirus_session_diff`, secrets, browser, computer-use and visual tools. Every tool that works on sessions takes the optional `computer` of ADR-208.
- **Same code as peers.** Calls run through `peer_tools::external`, the receiving side of ADR-208 for a caller with no session: no parent link, no wake-up (the client polls `sirus_delegation_status`), projects by name or id, an isolated worktree by default. It does not need "Let agents manage sessions": the token is the consent.
- **Approval and origin.** Sessions an outside client starts or continues use the person's default approval mode for new conversations (Settings), but "full access" is stepped down to "ask" (`external_approval`): outside agents never get full access implicitly, and a session that needs an approval waits for the person in its own tab. The first prompt or message is headed "Via MCP: <token name>. A task from an outside agent, not from the person", and the sessions are ordinary visible ones.
- **A stdio form.** `sirus-host mcp [--token T] [--url URL]` (token also from `SIRUS_MCP_TOKEN`, address from `SIRUS_MCP_URL`, else the loopback address a running host writes in `mcp-endpoint`) reads JSON-RPC lines on stdin, posts them to the host's `/mcp` and writes the replies, so `claude mcp add sirus -e SIRUS_MCP_TOKEN=… -- sirus-host mcp` works. Remote clients use the URL: `claude mcp add --transport http sirus https://<public or Tailscale address>/mcp --header "Authorization: Bearer <token>"`.

## Consequences

- The phone's protocol does not change; no shared types were added.
- A token can read every session's transcript and, with `write`, start or continue work: scope and revocation are the controls, and the person's approval settings still decide what a started session may do.
- `sirus_delegation_cancel` stops any visible session, not only ones that token started.
- The public route must let `/mcp` through with the bearer token alone: clients such as Claude Desktop or ChatGPT cannot send Cloudflare Access service-token headers, so that path needs a bypass rule on the route's Access policy.
- Server-sent streams, resumable sessions and OAuth are left out; clients that insist on OAuth cannot use it yet.
