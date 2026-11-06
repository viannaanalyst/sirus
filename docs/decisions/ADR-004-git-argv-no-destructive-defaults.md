# ADR-004: Git via argv; no automatic destructive operations

**Status:** Accepted

## Context

Sirus’s product model is parallel Sessions on Git worktrees. A host that silently `reset --hard` or `clean -fd` will destroy user work. Shell strings (`git $user`) invite injection.

## Decision

- Configured `hook.*` entries are refused before native Git operations: Git 2.54 executes configured hooks despite `core.hooksPath=/dev/null`. Traditional hooks/fsmonitor remain disabled. Inspectors and operations share cwd, safe overrides and environment, removing the config-only `GIT_CONFIG` override. Hook/filter inspectors capture names only with bounded output and a two-second deadline; failed inspection refuses the operation. Filter and hook checks are preflight guards, not a guarantee against concurrently modified external config. Networking/ref import requires separate config isolation; automatic fetch remains unavailable.
- All Git is `std::process::Command` with explicit argv (`git.rs`, `worktree.rs`).
- Isolated worktrees: `git worktree add -b sirus/<slug>-<id> <dest> HEAD` under `{app_data}/worktrees/{project_id}/`.
- `git worktree remove --force`, `reset --hard`, and `clean -fd` are **never** issued automatically.
- Removing an isolated worktree requires `confirm: true` from the caller; otherwise `confirmation_required`.

## Consequences

- **Positive:** parallel work without mixing trees; user data is hard to delete by accident.
- **Negative:** a dirty worktree may refuse `worktree remove`; the UI must explain, not force.
- **Accepted trade-off:** slower “make it clean” UX versus irreversible data loss.

## Alternatives considered

- **libgit2 only:** weaker worktree UX than the Git CLI the user already has.
- **Force-remove on session delete:** rejected.
