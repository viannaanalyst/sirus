# ADR-043: Team orchestration is plan-confirmed, worktree-isolated and merged explicitly

**Status:** Accepted

## Context

People want to split one request across several coding agents, possibly different providers and models, and still review the result like any other change. A model-driven loop that spawns agents, answers their approvals or merges on its own would widen authority beyond the person's choices. Provider-native subagents (ADR-035) stay inside one provider and cannot do this. Claude Code agent teams only form in interactive sessions, while Sirus runs Claude headless.

## Decision

A **Team** is an opt-in, one-shot composer mode (`+` → Equipe). It is memory-only in the composer context, and turning it on turns off planning and debugging. All state is native. The coordinator session owns `Session.team`, and each helper session carries `Session.teamWorker`.

### Flow

1. **Plan.** `SendPromptRequest.team` admits a normal turn with three changes:
   - Native code forces it to read-only planning, downgrading Full to Ask.
   - Only Codex, Claude Code or OpenCode may coordinate, because these providers have verified planning turns.
   - The process prompt is wrapped with coordinator instructions and the offered catalog: enabled team providers with at most 8 cached native models each. The persisted user message keeps the visible text.

   When the turn settles, `team.rs` extracts the last `<sirus_team_plan>` JSON block and validates it as untrusted input:
   - 1–3 tasks;
   - unique ids of 16 characters or less;
   - printable titles of 120 characters or less, and instructions of 4000 or less;
   - providers and models present in the offered catalog;
   - 1–8 relative paths per task, each an exact file or `folder/**`, with no `..`, absolute paths, backslashes or other wildcards;
   - a dependency may point only to an earlier task, so plan order is a valid merge order.

   The visible reply drops the block. An invalid plan becomes `failed` with a short reason; the person can discard it and try again.
2. **Confirm.** Nothing runs before the closed `team_action { type: "start" }` call. The person may only edit task titles, change the provider/model (revalidated natively) or remove tasks. Adding tasks or changing instructions and paths is unavailable. Helpers use Ask or Auto, taken from the coordinator's last admitted approval; Full is refused.
3. **Run.** Each task creates a normal session in a new isolated worktree.

   The worktree starts from a snapshot of the coordinator checkout **as it is now**:
   - Native code stages `add -A` into a private temporary index (a copy of the real index), then writes `write-tree` and a `commit-tree` on top of `HEAD`.
   - Uncommitted edits, deletions and untracked non-ignored files are included. Ignored files such as `.env` or `node_modules` are not.
   - The person's index, files and branch are untouched. A clean checkout uses `HEAD`. Tasks without dependencies start through the existing `send_prompt` admission, with a worker prompt that carries only that task. Every helper's approvals arrive in its own session for the person; no model answers them.

   After each native settlement (`agent-exit`), `team::settled` records the result and starts a dependent task only after its prerequisite **succeeded**. A failed or stopped prerequisite stops its dependents. Nothing is polled. `stop` interrupts running helpers and marks unstarted tasks stopped.
4. **Merge all.** This is an explicit action, processed one task at a time in plan order:
   - Native code stages the helper's changes inside the app-owned worktree and reads a binary patch, at most 8 MiB.
   - It stops with `outOfScope` if any changed path is outside the task's planned paths.
   - Because helpers start from the uncommitted state, local edits are expected. `git apply --check` verifies every hunk's context against the current files. Edits made since the snapshot merge only when they touch different lines; otherwise the run stops with `conflict`.
   - Only then does it run `git apply` on the coordinator working tree.
   - Nothing is committed, staged in the coordinator checkout, pushed or forced.
   - The first stop applies nothing from that task and ends the run.

   From a stop, the person can choose:
   - `skip`: the task stays recorded as skipped, with its worktree kept.
   - `mergeAnyway`: only for `outOfScope`.
   - `resolve`: only for `conflict`. All Git work happens in the app-owned helper worktree, never in the person's files:
     1. Native code snapshots the coordinator checkout as it is now, including merged teammates and later edits.
     2. It stashes the helper's own changes (`stash push --include-untracked`), moves the helper's app-owned branch to that snapshot (`checkout -B`) and pops the stash with Git's three-way merge.
     3. A clean result returns the task to "ready to merge". Remaining conflicts send the helper one turn listing the conflicted files, under the team's Ask/Auto approval, to resolve them in its own copy. The person then merges again explicitly.

   `stash` joined the native filter guards. Merge refuses any patch that still adds conflict markers.

   Merged work is reviewed and committed with the existing Changes/Commit flow.
5. **Cleanup.** `cleanup { confirm: true }` removes the helpers' sessions and worktrees through the existing confirmed removal, once the team is done or stopped. Merges apply patches without commits, so each helper worktree still holds its edits; cleanup first commits them to the helper's own `sirus/...` branch (fixed Sirus identity, hooks and signing disabled) so the worktree is clean and is removed without force. Nothing is discarded, and the branches stay for inspection or manual deletion.

### Recovery and limits

After a restart, nothing resumes:

- planning becomes failed;
- running tasks take their helper's recovered status;
- unstarted tasks stop;
- finished work stays mergeable.

Limits: 3 tasks per team and one live team per coordinator. Helpers cannot coordinate.

### Security review

- **IPC.** `team_action` is a closed tagged enum with `deny_unknown_fields`. Every action revalidates session ownership and team state natively. Responses return only the coordinator and its helper sessions.
- **Git.**
  - `git apply` joined the native guards that refuse configured hooks and external filters.
  - `run_input` passes the patch on stdin through the same guarded command.
  - The patch is computed only in app-owned worktrees.
  - Planned paths are advisory for the agent and enforced at merge time. They are not an OS sandbox; each provider's own sandbox and approvals still apply.
- **Prompts.** Coordinator instructions become helper prompts. Repository content can therefore influence helper instructions, just as it can influence any prompt. Helpers run under the person's chosen Ask/Auto policy, never Full.
- **Credentials and capabilities.** No credentials, capabilities, plugins or new process kinds were added. Helpers are ordinary sessions.

## Consequences

The person decides at two points: the plan before anything runs and the merge after everything finishes. Teams cost one provider turn per helper plus the planning turn, each against that provider's own account limits.

Not available in V1:

- automatic conflict resolution in the person's files (resolution only happens in a helper's own worktree, on request);
- coordinator follow-up loops, agent-to-agent messages or an MCP tool that lets a coordinator spawn helpers;
- free-text assignment such as "Claude does X".

Tests cover:

- plan parsing and refusal of untrusted plans;
- scope matching;
- dependency advancement, including blocked dependents;
- restart recovery;
- closed actions;
- a disposable-repository merge: applied without commits, out-of-area stop, local-edit stop and a real conflict that leaves the working tree unchanged.

No live multi-provider run is claimed.
