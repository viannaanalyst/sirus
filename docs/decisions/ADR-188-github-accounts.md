# ADR-188: GitHub accounts in Settings, through GitHub CLI

- Status: accepted
- Date: 2026-10-29

## Context

Every GitHub feature in Sirus (Pull requests, PR watches, CI auto-fix) reads through `gh`, and `git push` over HTTPS uses `gh` as its credential helper. `gh` keeps several accounts per host with one active (`gh auth switch`), but switching or adding one meant a terminal. The reference app and another open-source client also rely on `gh` and point at `gh auth login`; another open-source client has its own GitHub App sign-in, which needs a registered app.

## Decision

- **No app of our own.** Sirus keeps using GitHub CLI's accounts and token store; it never sees a token.
- **Settings → Git → GitHub accounts** lists the github.com accounts from `gh auth status --json hosts` (active first, an expired token marked), switches with `gh auth switch --user`, and signs one out with `gh auth logout --user` after a confirmation.
- **Add account** runs `gh auth login --web --git-protocol https` without a terminal (prompts disabled, `GH_BROWSER=true`): `gh` prints a one-time code and the device URL, which the section shows with Copy and Open GitHub; Sirus waits up to 15 minutes for the process to finish, then lists the accounts again. Leaving Settings, Cancel, or a new attempt kills the waiting process.
- One closed action enum, `github_account_action` (`list`, `switch`, `logout`, `loginStart`, `loginWait`, `loginCancel`), desktop only; the section is hidden in the remote UI. Logins are checked against GitHub's format and against the signed-in list before `gh` sees them. A change clears the `gh` read cache (ADR-100) so pull requests reload for the new account.

## Consequences

- Switching the active account also changes which account `git push` uses, as in the terminal; the section says so.
- Tests: the status JSON (active first, other hosts and invalid logins ignored, expired tokens), login validation, and reading the device code and URL.
