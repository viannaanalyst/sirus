# ADR-044: Explicit empty workspace entries through native ownership

**Status:** Accepted

## Context

The lazy Files tree can open existing files in the editor, but creating a file
or folder requires leaving Sirus. Giving the webview filesystem access
would bypass the session/worktree and Client/Transport boundaries. Reusing an
editor overwrite operation would also introduce ambiguous replacement behavior.

## Decision

- `FileTree` adds New file, New folder and Collapse all. A selected directory
  supplies the destination; a selected file supplies its parent; otherwise the
  destination is the workspace root. The inline form captures that destination,
  keeps errors and the entered name, and disables cancellation after native
  creation starts. Collapse closes descendants without loading them. A created
  file opens through the existing editor callback and explicit Save semantics.
- `SirusClient.createWorkspaceEntry` admits only a session ID, the closed
  `file | directory` kind, one name and an optional parent. The allowlisted
  `create_workspace_entry` command derives the canonical root from the owned
  session, checks shutdown and live ownership, and holds the metadata admission
  lock through the bounded operation. There is no path-only renderer authority,
  generic filesystem command, recursive mkdir, rename, delete or overwrite.
- `workspace_entries.rs` limits names to one printable leaf of at most 255 UTF-8
  bytes and refuses dot components, separators, controls and ignored/reserved
  workspace names. Destinations must stay in the owned root. Unix creation walks
  root and parent descriptors with `O_NOFOLLOW`, checks their identities, and
  uses exclusive `openat` or `mkdirat`. Existing entries are never replaced.
  Verified type, descriptor path and device/inode checks precede publication.
  Non-Unix targets explicitly refuse creation pending a safe implementation.
- Creation is transient UI state, not persisted settings or a new store. Keyed
  session/worktree trees and late-result guards prevent an old response from
  populating another workspace. Only visible lazy directories refresh; no
  workspace traversal or polling is introduced.

## Consequences

- **Positive:** small explicit creation controls use native session ownership
  and preserve editor saves, lazy traversal and the existing transport boundary.
- **Negative:** creation is currently available on Unix only; native integration
  and fixtures were exercised on macOS, not cross-compiled for other targets.
- **Accepted trade-off:** descriptor checks are admission/publication checks,
  not an OS filesystem jail. An external process may rename a verified parent
  between the last check and creation, or substitute a directory between
  `mkdirat` and opening it. Postchecks can reject publication but cannot safely
  undo an already-created empty entry. There is no automatic rollback/delete or
  claim of immutable namespace ownership after return.

## Alternatives considered

- Frontend filesystem or shell plugins: rejected because the webview is
  untrusted and the native-owned session/worktree must remain authoritative.
- Recursive creation or overwrite-on-collision: rejected because neither is
  needed for a single explicit Explorer action.
- Path-only creation on unsupported platforms: rejected rather than shipping a
  weaker fallback under the same API.

This decision extends the creation restriction in
[ADR-019](ADR-019-header-environment-dock-and-editors.md). Editor overwrite/save
and the no-rename/no-delete policy remain separate.

## Amendment (2026-10-03): explicit Move to Trash

Files offers **Move to Trash** for files and folders through the row context menu. It always passes through a red confirmation.

`trash_workspace_entry(sessionId, path)` derives the owned session root. It accepts only an existing absolute path strictly inside that root, which rules out the root itself. It refuses:

- `.`/`..` segments, control characters and backslashes;
- reserved folder names such as `.git` and `node_modules`;
- a linked or non-directory parent, checked with `symlink_metadata` on every ancestor.

macOS then moves the entry with `NSFileManager.trashItemAtURL`. The entry stays recoverable from the Trash, and a link moves itself, never its target. Nothing is deleted permanently. Other platforms report Trash as unavailable.

Like creation, these are preflight checks, not an OS jail: a concurrent external rename can still race the move.

Tests cover refusal of the root, reserved folders, outside paths, missing entries and linked parents. Moving into the real Trash is not exercised in tests, so they never touch the user's Trash.
