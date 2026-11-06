# AI commit title implementation plan

> **For agentic workers:** Use subagent-driven-development for implementation and scoped reviews.

**Goal:** Replace the filename suggestion with an explicit provider-generated, editable commit title.

**Architecture:** A closed native `commit_title_action` derives the prepared diff from a session-owned workspace and selects an enabled installed Codex or Claude CLI. This is a cancellable, bounded utility request in a private empty temporary directory, separate from chat turns. The existing Changes pane applies a result only to its captured workspace and unchanged input revision.

**Tech stack:** Existing React/Client/Transport, Rust/Tokio, Git argv, vendor CLI authentication. No new dependencies, persistence, capabilities or shell authority.

## Global constraints

- This checkout has no root Git metadata. Do not initialize Git, commit or create worktrees; preserve concurrent Team changes. Review source snapshots instead of Git diffs.
- Read AGENTS.md and relevant source before changes. UI accesses native functions only through Client/Transport.
- The user has approved provider-generated titles; no additional approval gate is needed. Generation itself never commits, stages or pushes.
- Only native-owned prepared changes are sent, after whole-index scope/conflict/completeness and expected-index checks. Never include unstaged contents or transcript context.
- Prefer the session's provider when supported; otherwise select an installed enabled supported provider. Preserve the session's account binding for that provider, otherwise its retained binding or native selected account. Never silently change account or retry inference through another account/provider.
- Initial utility adapters support Codex and Claude, whose local help exposes hardened configuration isolation. No guessed CLI flags or generic execution endpoint.
- Fixed argv, stdin prompt, private empty cwd, ignored user/project customizations, no app MCP integration, restrictive provider mode and disabled tools. Vendor authentication stays in the CLI and profile environment stays child-only.
- One utility job at a time, 90-second deadline, bounded input/output, owner/request-bound cancellation and shutdown-owned process-group cleanup. No content, raw diagnostics, credentials or provider envelopes in logs/IPC.
- Read a bounded excerpt (up to 8 KiB filenames and 40 KiB patch) and return `partial` when summarizing an excerpt. Recheck owner/provider/account/executable and index before starting inference and before publication.
- Generated title is a nonempty printable single line of at most 72 Unicode characters. English code/commit language follows project conventions. Structured provider output only; do not accept stderr/reasoning/tool text as a title.
- UI provides generating/cancel/error/provider/partial feedback, permits title typing during generation, and preserves edits made after the request. Session changes/unmount cancel the job and reject late results.
- Keep current staged-only Commit and confirmed Push behavior.

## Task 1: Native utility and security contract

**Files:** Create `src-tauri/src/commit_title.rs`; extend `src-tauri/src/git_workspace.rs`; register in `src-tauri/src/lib.rs`; add shutdown cleanup in `src-tauri/src/commands.rs`; document in ADR-046 and AGENTS.md/ADR index. Do not modify frontend files.

**Interfaces:**

```ts
type CommitTitleAction =
  | { type: "generate"; sessionId: string; expectedIndex: string; requestId: string }
  | { type: "cancel"; sessionId: string; requestId: string };
type CommitTitleResponse =
  | { type: "title"; title: string; provider: "codex" | "claude"; partial: boolean }
  | { type: "cancelled" };
```

Native command: `commit_title_action(state, action)`. Serde rejects unknown fields and request IDs must be UUIDs. Session ID never supplies a path or executable. If no enabled installed adapter is available, report that generation requires Codex or Claude; preserve manually entered titles.

- [x] Capture before-edit source snapshots for review.
- [x] Implement/test native prepared context: empty/stale/conflicted/outside-scope refusal, staged vs later unstaged bytes, unborn HEAD, UTF-8 excerpt bounds and no external diff/textconv.
- [x] Implement/test selection/account binding, fixed isolated argv, strict structured output parsing, empty/oversized/multiline/error replies, request ownership and concurrency.
- [x] Implement/test owned bounded stdin/stdout/stderr capture, deadline/cancel/shutdown/drop cleanup with disposable fake children. Do not infer CLI success from parseable output alone.
- [x] Wire native command and shutdown, write ADR including capability/security review and boundaries. Extend ADR-045 to point at the replacement title flow.
- [x] Run focused Rust tests, format/check/clippy as appropriate, record evidence and source diff in `/tmp/sirus-ai-commit-native-report.md` and `/tmp/sirus-ai-commit-native.diff`.
- [x] Obtain scoped spec/code-quality review and resolve findings before Task 2.

## Task 2: Client and Changes UI

**Files:** `src/client/index.ts`, `src/client/types.ts`, `src/components/ChangesPane.tsx`, `src/lib/commit-title.ts`, `tests/commit-title.test.ts`, `src/i18n/git-workspace-strings.ts`, workspace preview mocks/copy under `previews/changes-review`.

**Interfaces:**

```ts
gitCommitTitle(sessionId: string, expectedIndex: string, requestId: string):
  Promise<{ type: "title"; title: string; provider: "codex" | "claude"; partial: boolean } | null>;
cancelCommitTitle(sessionId: string, requestId: string): Promise<void>;
```

The client sends only the closed actions above and maps a cancelled generation to null. Generation button becomes “Generate title” / “Gerar título”, shows pending state and Cancel. Keep editable input. On result, apply only if alive, same request, same captured snapshot and same input revision; otherwise preserve user's text and show suitable feedback. UI cancellation and unmount reject late response publication. Clear any provider attribution when title changes manually or prepared index changes. Generation and index writes share the existing synchronous UI gate; release it only after the owned Generate request settles, because Cancel acknowledgment can precede process cleanup.

- [x] Update shared types/client and add mocked-transport contract tests for generated and cancelled responses.
- [x] Replace local title generator with provider request lifecycle; remove dead metadata-only behavior/tests. Test typing during pending generation, cancellation, unmount/owner changes and late replies with deterministic fake APIs, without native inference.
- [x] Update both locales, including native error translations and disclosure that prepared code is sent to the provider.
- [x] Update the existing browser preview using simulated generation/cancel/provider/partial responses; clearly label simulated behavior and never call a provider.
- [x] Run focused tests, typecheck/lint/build; record report/diff under `/tmp/sirus-ai-commit-ui-*`.
- [x] Obtain scoped review and resolve findings.

## Final verification

- [x] Whole-feature security/spec review from source snapshots.
- [x] Required frontend and native checks, Graphify incremental update and desktop bundle build. Run disposable live inference only with a synthetic diff and no user repository/credentials read by Sirus; report any unavailable runtime honestly.
- [x] Confirm bundle completion and share the updated application and browser preview links. Do not quit the person's app, replace /Applications or perform real commit/push.

Verification completed: native294 passed19ignored; frontend217 plus verification scripts passed, final utility lifecycle15 tests passed; typecheck/lint/build/preview builds, format/check/clippy, synthetic native Codex/Claude inference and desktop signature passed. Scoped and final reviews approved. Rendered gate/attribution component coverage remains a minor follow-up; existing chunk-size warnings remain.
