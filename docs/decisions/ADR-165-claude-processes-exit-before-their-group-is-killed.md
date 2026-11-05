# ADR-165: Claude processes exit by themselves before their group is killed

**Status:** Accepted (2026-10-26).

## Context

After the Mac had been off overnight, the first conversation used in the morning kept failing with
`Failed to authenticate: OAuth session expired and could not be refreshed`, even after ADR-162's
retries. A new conversation on another Claude profile worked. The default profile's Keychain item
(`Claude Code-credentials`) was left blanked: `accessToken` and `refreshToken` empty, `expiresAt: 0`.
It had been modified at the same second a `/compact` in that conversation failed with `OAuth token
revoked`.

The access token expires overnight, so the first `claude` process that needs it refreshes it.
Refreshing rotates the refresh token on the server, and the CLI then persists the rotated pair by
running `security add-generic-password` as a child process in its own process group. On launch,
the usage probe (`provider_usage.rs`) is often that first process. It runs on the open
conversation's account, sends `get_usage` and, as soon as the answer arrived, sent `SIGKILL` to its
whole process group. Kept agent processes (`agent_pool.rs`) were stopped the same way on idle
timeout, eviction, a key change and quitting, and a finished turn's process got only 2 s.

A kill that lands between the server-side rotation and the Keychain write loses the new token. The
stored refresh token is already spent, so the next process fails with "revoked" or "could not be
refreshed", and the CLI writes its failed, empty state back over the item. A fresh process can never
recover from that.

## Decision

- `agent::retire_owned` ends a process whose input the caller has closed. It waits up to
  `agent::GRACEFUL_EXIT` (5 s) for the leader to exit by itself, then kills what is left of its group
  and reaps it. `claude -p --input-format stream-json` exits at once on end of input, after
  finishing its pending writes.
- The usage probe closes its input and retires its process in the background, both after reading
  the answer and when it is cancelled or past its deadline. It no longer kills the group first.
- Kept processes (idle timeout, eviction, a key change, a discarded session) drop their wire and
  are retired in the background. Quitting closes every kept process's input, polls their exits
  without reaping for up to 2 s, then kills the groups.
- A finished turn's process gets `GRACEFUL_EXIT` once its wire is dropped. An ADR-162 retry still
  stops the failed process after 2 s, because its input stays open and it would not exit by itself.

## Consequences

- A refresh that has started is allowed to be saved, so Sirus no longer burns the stored login of
  the profile it probes or keeps processes for.
- Stopping an idle process can take up to 5 s in the background. Quitting waits up to 2 s more than
  before, but only when a kept process does not exit on end of input.
- The upstream race between unrelated processes refreshing at the same moment (ADR-162) remains.
  This removes the case Sirus caused by itself.

## Later: Stop and quit

Stop used to SIGKILL the group at once, with the same risk for a token being saved. `stop_tree` now sends the leader SIGTERM and the group goes when the leader exits or after `LEADER_GRACE` (2 s). Quit waits for that too, so a tool may run for up to 2 s after Stop. The Claude model catalog is cached for six hours per account, invalidated when the CLI binary changes, so fewer short-lived probes run.
