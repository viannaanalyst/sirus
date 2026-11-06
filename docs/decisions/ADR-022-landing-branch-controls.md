# ADR-022: Explicit branch listing, switch and creation from the new-thread landing

**Status:** Accepted

## Context

A new-thread landing should let the user pick the project, the workspace mode
(Local project / New worktree) and the branch (search, switch, create) before
typing. Sirus only had these choices in the New session dialog and no
branch surface at all.

## Decision

- The landing composer row (`LandingControls.tsx`) shows three chips: project
  (search, switch, **New project**), workspace (`Local` = project checkout,
  `New worktree` = isolated, disabled when the project is not a Git repository)
  and branch (search local branches, current tag, **Create and checkout new
  branch…**). Selections feed the existing draft state: `selectedProjectId`,
  `draftIsolated`, and the real checkout branch.
- Native additions (`git.rs` + IPC): `list_branches(project_id)` runs
  `git branch --format=%(refname:short)%09%(HEAD)` (read-only, bounded to 500
  entries); `checkout_branch(project_id, branch, confirm)` and
  `create_branch(project_id, branch, confirm)` run `git checkout <branch>` /
  `git checkout -b <branch>` with `confirm: true` required. Branch names are
  charset-validated, must exist for a switch, and pass
  `git check-ref-format --branch` before creation. Both reuse the existing Git
  guards (configured hooks refused, external checkout filters refused, no shell
  strings, bounded output).
- The decorative background (`LandingOrbits.tsx`) renders only on the empty
  landing and stops animating under reduced motion. Its motion lifecycle is
  documented in the [UI guide](../development/ui-arc.md#new-thread-orbit-background).

## Consequences

- **Positive:** the new-thread flow is complete; branch switching uses the
  project checkout and never a renderer-provided path; hooks/filters guards stay
  in force.
- **Negative:** switching branches mutates the user's checkout (uncommitted
  changes can block it and git's own error is surfaced); creating a branch is a
  real write operation that only an explicit click triggers.
- **Accepted trade-off:** checkout/create are not journaled beyond the existing
  state persistence; recovery relies on Git itself.

## Alternatives considered

- Selecting a branch without checking out (e.g., a worktree at an arbitrary
  ref): rejected for the Local mode — it would silently diverge from the
  project checkout the session actually uses.
- Implementing branch switching through the renderer shell: rejected by the
  Client/Transport boundary; all Git writes stay in Rust.
