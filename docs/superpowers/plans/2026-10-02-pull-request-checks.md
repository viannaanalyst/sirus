# Pull Request and Checks Implementation Plan

> Execute inline using executing-plans in the authorized checkout. No delegation, commit, push or computer use.

**Goal:** Compact silver switches and a real branch-owned PR/checks card in Environment.

**Architecture:** One fixed read-only native command accepts a session ID, resolves its cwd/actual Git branch/origin/HEAD, then uses the installed GitHub CLI's own authentication for bounded GitHub REST GET requests. Native identity is rechecked before returning typed metadata. Zustand holds memory-only snapshots and deduplicates requests; hidden sections do not probe. Existing opener and Popover own navigation and interactions.

**Tech Stack:** Existing React/Arc/Radix, Zustand/Client/Transport, Rust/Tokio, gh, atomic JSON settings.

## Constraints

- Switch track 36×20 px, 14 px thumb and 18 px orbit; 42×32 px control hit area. Keep keyboard, finite motion, reduced motion and silver tokens.
- GitHub origin only. Native branch and commit only; no renderer-selected repo, URL, endpoint, executable, credentials or flags.
- Fixed `gh api --hostname github.com --method GET` for PR list, check runs and combined commit statuses. CLI executes outside the project, with debug/browser/pager/proxy overrides removed and bounded output/process-group cleanup. No login, mutation, fetch, merge, rerun, comment or credential reading in Sirus.
- Prefer the newest open PR, otherwise newest closed/merged PR for the branch. Checks belong to the PR head SHA. Display local/PR commit mismatch accurately.
- At most four 8-second probes, 100 runs + 100 statuses. Partial failures/truncation must never report all checks passed. Validate/escape text and only admit credential-free GitHub HTTPS check links.
- Real no-PR, missing CLI, disconnected CLI, detached, unsupported remote, not-Git, error and loading states. Manual refresh; no polling.
- `showEnvironmentPullRequest` defaults true, participates in General's single page reset and never persists PR snapshots.

## Tasks

- [x] Shrink shared `orbit-switch.css`; retain existing semantic SSR coverage.
- [x] Add `src-tauri/src/pull_requests.rs`: typed statuses/PR/checks, strict repository/SHA parsing, fixed REST endpoints/CLI process, normalization and fixture tests (ownership, argv, auth/error sanitization, partial checks and timeout cleanup).
- [x] Add bounded Git read probes reusing native hook guards. Register `session_pull_request(session_id)`; resolve session cwd on a worker and revalidate cwd/repository/branch/HEAD after the CLI completes. Add native ownership/stale-context fixtures.
- [x] Align TS/Rust settings and types, Client method and existing Zustand memory cache. Test coalescing, refresh, removal and late responses. Add General visibility row and scoped-page reset membership.
- [x] Add `EnvironmentPullRequestSection` and check details Popover using existing primitives, typography and localized status strings. Add real SSR checks for states, escaping, labels and hidden section.
- [x] Run typecheck/lint/Node/SSR and native fmt/check/Clippy/tests; rebuild/sign desktop. Document the reviewed IPC in ADR-031/AGENTS/reference map and update Graphify incrementally.

## Interfaces

`session_pull_request(session_id: String) -> Result<PullRequestSnapshot>`; `Client.sessionPullRequest(sessionId): Promise<PullRequestSnapshot>`.

`PullRequestSnapshot` includes sessionId, repository, branch, localHead, status, checkedAt and optional pullRequest. PR includes number/title/state/draft/url/head/base refs/headSha/checks/checksComplete/checksTruncated/localCommitDiffers. Each check has stable id/name/status and optional validated GitHub URL.

Store `pullRequestsBySession: Record<string,{loading:boolean,error:string|null,snapshot:PullRequestSnapshot|null}>` and `refreshPullRequest(sessionId,force=false): Promise<void>`; cache 60 seconds, single flight per owner, no periodic refresh and no removed-owner resurrection.

## Verification

- Application/test TypeScript and ESLint passed.
- Node tests: 110 passed, 0 failed; real General/PR SSR checks passed.
- Rust fmt/check/Clippy passed; 162 tests passed, 17 opt-in provider tests ignored, 0 failed.
- Installed GitHub CLI fixed GET smoke passed against a public fixture branch with no PR. No private account/API output was displayed.
- Desktop app rebuilt with Sirus Code Local Signing; deep/strict signature verification passed.
- Incremental Graphify completed with its known inline-import TypeScript parser warning; TypeScript compilation passed. Updated documentation links resolve.
- No computer use, commit, push, GitHub mutation, credential reader or provider inference.
