# ADR-064: CI auto-fix

**Status:** Accepted

## Context

When a pull request's checks fail, the person has to notice, open the logs and ask the agent to fix them. The owner asked for failing CI to be fixed by itself.

The owner asked for something more automatic, with no banner, approvals or clicks. That conflicts with four standing rules:

- turns start from an explicit Send;
- commit is an explicit user action;
- push is an explicit user action;
- nothing polls.

## Decision

The owner authorizes one global exception, `AppSettings.ciAutoFix` (Settings → Git, off by default). This is the third exception to "turns start from an explicit Send", after automations (ADR-051) and the armed resume (ADR-063).

**Watching.** One native timer (`ci_autofix.rs`) runs every 60 seconds and does nothing while the setting is off.

- **Which sessions:** each round takes at most 8 sessions, most recent first. A session qualifies if it was active in the last 72 hours or is already tracked. Side chats and archived sessions are skipped.
- **How:** it reads the session's PR through the same read-only probe as the Environment card (`pull_requests`).
- **Tracking:** progress is stored per session in `AppData.ciAutoFix`, at most 200 entries. When the PR is closed or merged, the entry is dropped.

**Deciding.** `decide` is a pure, tested function.

- **Wait** when any of these is true:
  - the session is busy (a running turn, pending approvals or questions);
  - the person turned auto-fix off for this PR, or it is paused;
  - the checks are not settled yet (some are still running or unknown).
- **Fix** when the settled checks contain a failure on a head the loop has not handled yet. There is one fix per PR head.
- **Pause** after 3 fix attempts without a green result.
- **Green** when the checks pass after one or more attempts. The attempt count resets and the person is notified.

**Fixing.** The session receives an automatic Auto-approval turn, never Full and never planning. The prompt carries:

- the failing check names;
- their summaries and annotations;
- the bounded Actions log excerpts from `github_inbox::failures`.

All CI content is quoted as untrusted data. The agent is told not to commit or push, and to change nothing if the PR did not cause the failure.

**Sending.** When the turn settles, a settlement hook wakes the timer.

- **Commit:** Sirus stages only the files that turn changed, through the same guarded index path as Live Changes. It commits them as "Fix CI: <checks>".
- **Push:** it pushes the session branch with `git::push`: never forced, with the same network-helper and hook guards.
- **Pauses** happen, with a notice, in these cases:
  - the turn failed or stopped;
  - the turn changed nothing;
  - unrelated changes were already staged;
  - the push failed.

**UI.**

- The Environment card shows a compact row only while a fix runs (with the attempt number) or is paused (with the reason). The row turns auto-fix off or back on for that PR, through the closed `ci_autofix_action` (status / setEnabled).
- The `ci-autofix-changed` event refreshes the state.
- A green result or a pause raises an app toast and a system notification, following the notification preferences.
- The automatic turn appears in the transcript as a normal user message that starts with "Auto-fix CI".

## Consequences

- **Positive:** a session's PR goes back to green without the person watching CI. The agent gets the logs directly. Pushing is done natively rather than by the agent, which also works when the provider sandbox blocks network access.
- **Negative:**
  - While the setting is on, Sirus polls GitHub once a minute for up to 8 sessions.
  - It starts turns, commits and pushes without a per-action confirmation.
  - Auto approval lets the agent edit files and run reviewed commands under each provider's own policy.
  - Concurrent manual edits in the same workspace between the turn and the commit can race the staging. Staging only the turn's files and refusing a pre-staged index limits this, but does not prevent it.

## Alternatives considered

- **A per-PR banner.** Rejected: the owner wanted no extra clicks. The per-PR switch in the Environment card covers opting out.
- **Agent pushes.** Rejected: Codex's workspace sandbox blocks the network, and app-side pushes keep the existing guards.
- **A "commit only, review before push" switch.** Rejected: without the push CI never reruns, so the loop would stop after one fix.
