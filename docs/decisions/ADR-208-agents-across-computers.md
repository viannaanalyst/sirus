# ADR-208: Agents across computers

**Status:** Accepted (2026-11-11)

## Context

An agent in a conversation on the Mac could not hand work to the VPS (run the tests or a build there, read the result) and an agent on the VPS could not reach the Mac. Delegation (ADR-111) and the `sirus_*` tools (ADR-076) only see this computer's sessions. The window already pairs with other hosts (ADR-133), but the pairing and its tokens live in the window's Keychain, and the VPS host has no window.

## Decision

- **A host pairs with another host.** `sirus-host peer add <https://address> <code>` redeems the other host's `sirus-host pair` code at `/api/pair` and stores the token in `peers.json` in the data folder (mode `0600`, at most 8 computers). `peer list` and `peer remove <id>` manage it; the running host reads the file on every call. It is written only by this CLI, never by a command a device can call, and a computer's name and address are all an agent ever sees. Pair each direction you want (Mac to VPS, VPS to Mac).
- **Same transport, no new port.** The sending host opens a short `wss://…/api/socket` connection (`peers::invoke`, rustls with the system's roots, like the window's `Oneshot`), sends `hello` with the token and one `invoke`. Only paired computers resolve; the address must be `https`.
- **Tools.** `sirus_list_computers` (always on) names the paired computers. Every tool that works on sessions (`sirus_delegate`, `sirus_create_session`, `sirus_send_to_session`, `sirus_read_session`, `sirus_session_status`, `sirus_delegation_status`, `sirus_delegation_cancel`, `sirus_list_sessions`, `sirus_models`) takes an optional `computer` (name or id; `here`, `local` or nothing stays on this computer). Without it nothing changes. `sirus_answer_approval` and the visual and review-guide tools stay local.
- **The receiving side** is one host command, `peer_tool`, that runs the same operations for a caller who lives elsewhere. It needs "Let agents manage sessions" turned on at the receiving computer, looks the project up by name or id (required, no caller folder), starts a normal visible session there (an isolated worktree by default) and heads its first prompt "Task from the session “…” on the computer …, not from the person". The session shows in that computer's window and on the Mac's list of that computer's conversations as any other.
- **Permissions.** The caller's approval mode is the ceiling (stepped down if the provider lacks it), a planning or Ask-about-a-pull-request caller cannot start or change anything remotely, starts count against the same 4 per turn, and a session started this way may start only one more level (it is recorded at depth 1), so work cannot bounce between computers. Approvals the new session needs wait for the person on that computer; the sender cannot answer them.
- **Async only.** There is no parent link across computers, so a remote delegation never wakes the sender: it returns `taskId` (the remote session id) at once and the agent polls `sirus_delegation_status` or `sirus_session_status` with the same `computer`.

## Consequences

- The window needs no change; the Mac and the VPS each need this host version, and each needs its own `peer add`.
- Tokens stay in one `0600` file per host and in the `Peer` type, whose `Debug` and public list leave them out.
- A paired host runs `peer_tool` for any of its paired devices (a phone included), which is no more than a device can already do by starting a session.
- The cross-computer work is limited to the tools above; no file transfer, so the project must exist on both computers (same name).
