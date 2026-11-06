# ADR-065: Environment Git actions, worktree handoff and stopping servers

**Status:** Accepted

## Context

The Environment card lacked Pull, Create PR, "Hand off to new worktree" and a stop button for detected local servers. Each one touches the network, GitHub or processes, so each needed a security review. The owner approved all four.

## Decision

**Pull.**

- A new `pull { sessionId, confirm: true }` variant of the closed `git_workspace_action` runs `git pull --ff-only --no-rebase origin <current branch>` in the session's own workspace.
- It never merges or rebases. Git itself refuses when local changes would be overwritten.
- It uses the same refusal of project-local network helpers as Push, plus the usual hook and filter guards.
- It is an explicit action. Automatic fetch stays unavailable.

**Create PR.**

- A new `create { sessionId, title, body, draft, confirm: true }` variant of the closed `pull_request_action` first pushes the session branch with `git::push` (never forced).
- It then opens the PR into the repository's default branch through the hardened `gh api` REST path.
- The repository is derived natively from the session's own `origin` and must belong to a saved project.
- Limits: the title is 1–256 characters and the body at most 64 KiB.
- Only the new PR's `https://github.com/…` URL is returned.

**Hand off to new worktree.**

- `handoff_session` gains an optional `newWorktree`.
- When it is true, the source checkout is snapshotted with Team's private snapshot commit (uncommitted and untracked non-ignored files included; the person's index and files untouched), before the data lock.
- A new isolated worktree is created from that commit, using the configured location and branch pattern.
- The handoff session is created in it with the usual recap, and opens in a pane beside the source.
- If creating the handoff fails, the just-created, still-empty worktree is removed.

**Stopping a local server.**

- A new closed `local_server_action { stop { sessionId, url } }` accepts only an http(s) URL on localhost, 127.0.0.1, ::1 or 0.0.0.0 with a port.
- It finds the listener with a fixed `/usr/sbin/lsof -nP -iTCP:<port> -sTCP:LISTEN -t` probe (bounded, cleared environment, 3 s).
- It walks each PID's parent chain with fixed `/bin/ps`, at most 64 hops, and sends SIGTERM only to processes that descend from Sirus itself: terminal shells' children or agent tools.
- Anything else is refused with "not started by Sirus".

## Consequences

- **Positive:** the Environment card covers the everyday Git and server actions without leaving the app.
- **Negative:**
  - Pull and Create PR reach the network with the person's Git and `gh` credentials. Both are explicit, confirmed actions.
  - A same-user process could still start a server under a Sirus terminal and have it stopped. That is the intended scope: the app only stops servers it started.

## Alternatives considered

- **Merge or rebase pulls.** Rejected: they can leave conflicts in the person's tree. Fast-forward only fails cleanly instead.
- **Killing any listener on the port.** Rejected: it could stop unrelated services.
- **Opening the PR with `gh pr create` from the project directory.** Rejected: the project's `gh` configuration could change its behaviour. The fixed REST path keeps the same hardened invocation as the review inbox.
