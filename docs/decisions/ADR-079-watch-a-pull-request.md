# ADR-079: Watch a pull request

**Status:** Accepted

## Context

CI auto-fix (ADR-064) wakes a session only when its PR's checks fail. Reviews and merge conflicts still need the person to notice, open GitHub and write a follow-up. A thread can watch its PR and wake the agent on new checks, comments and conflicts, with a wake limit against chatty bots and no polling once a thread settles.

## Decision

A session can watch its own pull request. This is the fourth exception to "turns start from an explicit Send", after automations (ADR-051), the armed resume (ADR-063) and CI auto-fix (ADR-064).

**Opt-in.**

- `AppSettings.prWatch` (Settings → Git, on by default) is the owner switch. While it is off the timer idles and new watches are refused.
- Each watch is per session. It starts from the "Watch PR" row in the Environment card or from the agent's `watch_pull_request` tool, which the agent calls when the person asks it to watch the PR.
- Side chats cannot watch; they share their parent's branch.
- The agent tool lives on the existing per-session MCP bridge (`browser_mcp`), so the token decides the session. It has no arguments: an agent can watch only its own session's PR, resolved through the same read-only probe as the Environment card (`pull_requests::load`). `unwatch_pull_request` stops it.

**Watching.** `pr_watch.rs` runs one native 60-second timer.

- At most 8 PRs are watched at once. Each one is read at most once every 55 seconds, whatever wakes the timer.
- One fixed GraphQL query through the hardened `gh` path (`github_inbox::graphql`) reads the PR state, head, `mergeable`, the latest check rollup and the last 30 reviews with their inline comments. All text is bounded and stripped of control characters.
- Busy sessions are skipped before any GitHub call: a running turn, pending approvals or questions, or an archived session. Events wait, untold, until the session is idle.
- A merged or closed PR removes the watch and notifies the person.

**Deciding.** `evaluate` is a pure, tested function that compares the PR with what the agent was already told.

- **Checks:** a failing check is reported once per PR head. A rerun that fails again is reported again. While CI auto-fix is on and not turned off for the session, failures are left to it, so the session never gets two turns for one failure.
- **Reviews:** a review counts when it is "changes requested", or a comment with a body or inline comments, submitted after the watch started, by someone other than the signed-in `gh` account, and not already seen. Seen review IDs are kept (at most 200). Approvals do not wake.
- **Conflicts:** reported when `mergeable` becomes `CONFLICTING`. `UNKNOWN` (GitHub still computing after a push) keeps the last answer.
- **Bound:** each wake counts. A new PR head resets the count. After 5 wakes in a row without a new head, the watch stops itself with a notice, and the person can watch again.

**Waking.** The session gets one automatic Auto-approval turn (never Full, never planning) that starts with "PR watch:". It lists the failing checks, each review with its author and state, the quoted review text, every inline comment as `path:line` with its quoted text, and a conflict notice naming the base branch. GitHub content is quoted as untrusted data and the prompt is bounded to 24 KiB. Unlike CI auto-fix, Sirus does not commit or push afterwards; the agent follows the session's usual rules. If the turn cannot start, the watch stops with a notice.

**UI.**

- The Environment card has a "Watch PR" row. Its badge shows the PR number and a green dot (watching) or an amber dot (stopped). Its popover holds the switch, the watched events with their last seen state, who started the watch, and the last update. A stopped watch offers "Watch again" or "Dismiss".
- On the Pull requests page, a PR watched by sessions shows an eye in its list row. Its Summary lists those sessions with what they watch and a Stop button.
- State lives in `AppData.prWatches`, changes through the closed `pr_watch_action` (status / set), and refreshes on the `pr-watch-changed` event.

## Consequences

- **Positive:**
  - Review feedback and conflicts reach the agent without the person relaying them.
  - The agent can arm the watch itself when asked.
  - Polling is bounded: 8 PRs, one query each per minute, and none at all for busy sessions.
- **Negative:**
  - While something is watched, Sirus queries GitHub once a minute per PR.
  - Automatic turns run under Auto approval without a per-turn confirmation.
  - Comments by the person's own `gh` account never wake the agent, including the person's own review notes.
  - Edited reviews are not reported again.
  - Conversation comments that are not part of a review are not watched, which keeps deploy and coverage bots from waking the agent.

## Alternatives considered

- **Extending CI auto-fix in place.** Rejected: auto-fix is global and commits and pushes by itself. A review or a conflict needs judgement, so the watch only describes what happened and lets the agent act.
- **Watching conversation comments too.** Rejected for now: bots that post to the conversation would spend the wake budget. The five-wake limit would still cover it if it is added later.
- **A per-tool-call PR number.** Rejected: an argument-free tool bound to the session token cannot be pointed at someone else's PR.

## Amendment (2026-10-19): watching again does not repeat news

Stopping a watch (the switch, Dismiss or `unwatch_pull_request`) now keeps its record as `status: "ended"`, hidden from `pr_watch_action` and never read by the timer. Watching the same PR again starts from what the last watch of that session told (head, failing checks on it, conflict, told review IDs), so failing checks and a conflict already reported are not told twice; a new failure, a new head or a new review is still news, and the wake budget starts over. A round that finishes after the watch ended leaves it ended. Forks never inherited watches: a watch belongs to its session id, and `fork_session` copies none.
