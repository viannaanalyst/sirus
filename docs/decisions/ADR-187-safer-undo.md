# ADR-187: Safer undo

**Status:** Accepted. Amends ADR-061 (undo a turn's changes).

## Context

ADR-061 reversed a turn's retained diffs file by file with `git apply -R`. It already left lines edited again afterwards alone, but it applied the files that still matched and only counted the rest, it did not notice that Git history had moved, and it only looked at other sessions that were running. The reference app's checkpoint guards (`src-tauri/src/checkpoint.rs`, 0.12 changelog) refuse to restore over later edits, across a changed Git `HEAD`, or while another active session overlaps the same files. This ADR adopts those three guards.

## Decision

Before an undo (of a whole turn or one file) Sirus refuses, changing nothing, when:

1. **Later edits.** A file's reverse patch no longer applies (`git apply -R --check`). The message says whether a later reply of the same session also reported that file or the person edited it. Edits elsewhere in the file are still kept, as in ADR-061.
2. **`HEAD` moved.** Each turn review records the workspace's `HEAD` commit when the turn settles (`TurnReview.head`, absent outside a repository and on older turns, which are not blocked by it). If `HEAD` differs now (commit, checkout, rebase, pull), undo is refused and the message points to reverting by hand from the diff or with `git revert`.
3. **Another session in the same working copy.** A session with the same folder that is running, or that has unkept changes (a review not kept, files not undone or committed) to any of the files being undone, blocks the undo. The message names that session; the dialog has an "Open session" link. Sessions in isolated worktrees have another path and never conflict.

**Atomic.** The refusal reasons come from read-only checks. `undo_files` dry-runs every patch first and applies none if any fails. Files without a usable diff (binary, expired) are still refused individually.

**IPC.** `check_turn_undo({ sessionId, messageId, paths? })` returns the first reason as a tagged `UndoBlock` (`headMoved`, `otherSession`, `changedAgain`) or `null`. `undo_turn_changes` runs the same check and returns its English message as an error. Checks are local: `git rev-parse HEAD` and `git apply --check`, no network.

**UI.** Opening the Undo confirmation runs the check. A refusal shows (localized, EN/PT) in the dialog, disables the confirm button, becomes the button's tooltip, and offers to open the other session. A refusal that appears only after clicking (a race) is shown as the error in the dialog.

## Consequences

- **Positive:** undo can no longer mix a turn with commits, rebases or a neighbouring agent's work, and it never half-applies.
- **Negative:** one stale or conflicting file now blocks the whole undo; the person can undo the other files one at a time. Turns from before this change have no recorded `HEAD`, so guard 2 does not cover them. Unkept changes of another session in the same folder block undo until kept or undone there.

## Alternatives considered

- **Storing full checkpoints (the reference app).** Rejected again: see ADR-061; retained diffs are enough.
- **Blocking on any later turn that touched the same file.** Rejected: it would give up ADR-061's "edits elsewhere in the file are kept".
