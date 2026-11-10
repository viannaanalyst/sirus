# ADR-202: Files, Changes, attachments and new projects on a remote computer

- Status: accepted
- Date: 2026-11-10

## Context

When the window is switched to a remote computer (ADR-133), the Files and Changes panes, the composer's workspace chip, attachments and "Novo projeto" were switched off (`this_mac_only`), because each read this Mac's disk or ran this Mac's `git`. Most of them already speak to the host: the Files tree and editor use `list_dir`, `read_text_file`, `write_text_file`; the Changes pane uses `git_workspace_action` and `git_status`; attachments go as bytes through `paste_prompt_attachments`. Only a few spots touched the local disk.

## Decision

- **Files pane.** Opens on a remote computer (`PanelContext.remote`). The tree, the viewer and saving use the host as before. The search has no local file list to rank, so each search asks `workspace_files` (the host's capped, recursive name search) for the first word typed and ranks what comes back with the local fuzzy matcher (`remote_host::host_query`, `ranked_hits`). The "file changed on disk" check, which stats a local path, is off.
- **Changes pane and the top bar's +N -N.** Open on a remote computer; `refresh_diff` was already host-driven. A commit of the history is read by two new `git_workspace_action` actions, `commitFiles` and `commitDiff` (`git_workspace.rs`, same fixed argv and bounds as the other actions: a hex id, a validated path, 2 MiB output), instead of this Mac's `git`; the Mac keeps `local_git` for its own folders. A host without them answers an error in the commit view.
- **Workspace chip.** Whether a project is a Git repository is read from this Mac's disk for its own folders and, on a remote computer, from the host: the first `list_worktrees` answer per project (an error means not a repository), kept in `remote_repos` until the computer changes. The chip, the choice of worktree and the default new workspace use it.
- **Attachments.** The pickers and the window screenshot stay on the Mac; the bytes already went to the active computer through `paste_prompt_attachments`, so the only change is that the two guards are gone.
- **New project.** On a remote computer "Novo projeto" opens a dialog instead of the folder picker (`shell_remote_project.rs`), as the phone does: clone `owner/repo` or an address with an optional folder name into the parent folder of the most recent project (else `~/Projetos`) with `clone_project`, or add a folder typed as a path on the host with `add_project`. The host validates both; the dialog shows its error. A project the host answers with that is already listed is opened, not listed twice.
- The Terminal and project actions remain Mac-only for now.

## Consequences

- The Files search on a remote computer is the host's substring match on the first word, capped at 64 files, not the whole worktree ranked locally. The command palette's file list (`refresh_files`) is still empty there.
- Moving to the trash on a headless host depends on that host having a trash.
- The phone's copy of the protocol needs nothing: the two new actions are additive.
