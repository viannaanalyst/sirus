# ADR-031: Branch-owned read-only pull requests and checks

**Status:** Accepted. Amended by [ADR-050](ADR-050-review-inbox.md): the review inbox may change GitHub through four confirmed actions.

## Context

Environment needs a real PR/checks card for the session workspace. A provider's
text output cannot establish repository identity or CI success. GitHub access
also must not introduce renderer-selected API requests, app login, token storage
or Git mutations. Existing native Git guards, bounded CLI probes and the user's
GitHub CLI authentication supply the required boundaries.

## Decision

`session_pull_request(session_id)` is the sole new command. A native worker
resolves the owned session cwd, actual symbolic branch, HEAD and normalized
GitHub `origin`. It ignores persisted branch labels when inspecting Git. Missing
Git/commits, detached HEAD and unsupported remotes are typed states. Configured
Git hook guards and bounded read probes remain in force.

`pull_requests.rs` runs installed `gh api --hostname github.com --method GET`
with fixed native endpoints: branch-filtered PR lists, check runs and combined
commit statuses. The newest open PR is preferred; the latest closed/merged PR is
the fallback. Scope is the origin repository with a matching head/base repository
and branch; upstream/fork PR discovery is not implemented. Repo segments and
head SHA are validated, and query values are percent-encoded. GitHub CLI handles
its own authentication; Sirus does not read credentials or start login.

Each CLI probe executes outside the checkout, disables prompts/update notices,
removes host/repository/debug/browser/pager/proxy overrides, uses null stdin and
bounded stdout/stderr, and has an 8-second deadline with owned process-group
cleanup. A lookup performs at most four probes without retries, pagination or
polling. Only normalized metadata crosses IPC; raw CLI/API output is discarded.
Authentication and other failures are typed, secret-free states.

Checks refer to the PR head SHA. At most 100 latest check runs and 100 commit
statuses are normalized, with explicit partial/truncated flags. Run SHA and
combined-status SHA must match. Unknown conclusions stay unknown, skipped checks
stay skipped, and an incomplete result cannot claim all checks passed. Links
must be bounded credential-free HTTPS URLs in the same GitHub repository, with
query/fragment removed; third-party CI links are
not opened. The native PR URL is generated from the verified repo and number.
The UI identifies the checked commit and indicates a different local HEAD
without claiming whether the branch is ahead or behind.

Before publishing, a worker re-resolves the owner and Git context. Closing,
removed owners and changed cwd/repository/branch/HEAD refuse the snapshot. This
is a before/after validation, not a Git transaction against concurrent external
changes. Existing `open_external_url` handles explicit link actions.

The existing Zustand store holds memory-only snapshots with a 60-second cache,
single-flight requests per session and explicit Refresh. Workspace/branch hint
changes queue a fresh probe behind an in-flight one; removed owners cannot be
revived by late responses. Native revalidation remains authoritative over a stale
frontend Git snapshot. Hidden/unmounted sections do not start probes. General
persists `showEnvironmentPullRequest`, defaulting true and participating in its
single page-level Restore defaults action.

### Security review

Renderer authority is one existing session ID. It cannot select a repo, endpoint,
host, binary, credential, CLI flag or request method. Native bindings establish
the workspace; all requests are GET reads to GitHub via the CLI's own account.
No merge, rerun, comment, fetch, login, provider inference, persistent PR data,
new dependency, plugin or capability is added. Metadata is rendered as escaped
text. Fixture tests validate ownership, encoded endpoints, fixed argv, partial
results, auth sanitization and stale-context refusal without live private data.

## Consequences

- **Positive:** real PR/check information with explicit commit identity, existing
  account authentication, manual refresh and no new mutable GitHub authority.
- **Negative:** the GitHub CLI must be installed and connected; upstream/fork
  discovery review/repair/merge actions are unavailable.
- **Accepted trade-off:** bounded partial previews and short memory caching,
  rather than unbounded pagination or continuous background checks.

## Alternatives considered

Inferring CI from agent output would invent status. Reading GitHub credentials
in Sirus or adding an app OAuth flow would expand auth authority. A generic
API bridge would expose arbitrary endpoints. Continuous polling conflicts with
the host's long-running performance contract. These were rejected.

## References

- [GitHub CLI api command](https://cli.github.com/manual/gh_api)
- [List pull requests](https://docs.github.com/en/rest/pulls/pulls#list-pull-requests)
- [Check runs for a Git reference](https://docs.github.com/en/rest/checks/runs#list-check-runs-for-a-git-reference)
- [Combined commit status](https://docs.github.com/en/rest/commits/statuses#get-the-combined-status-for-a-specific-reference)
