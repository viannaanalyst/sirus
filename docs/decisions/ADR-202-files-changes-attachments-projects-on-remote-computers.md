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

## Addendum (2026-11-11): videos on a remote computer

- **Reply videos.** The window plays a video with the web view's player, which reads a file on this Mac. After `reply_media` granted a link, the window opened `reply_rich::video_file(cwd, path)` on its own disk, which on a remote computer is a path on the other machine, so nothing played. Now, while another computer is active, the window copies the granted file into a private cache (`media_copy.rs`: `$TMPDIR/sirus-remote-media`, folder `0700`, files `0600`, written to a part file and renamed) and plays the copy. On this Mac it still plays the file in place.
- **Copying.** A new host command, `media_chunk { token, offset }`, returns up to 4 MiB (base64) of the file a media token grants, plus the file's size. It reads only files a token was issued for, so the host's rules do not change: `media_preview`, `reply_media` (videos only inside the session's workspace) and `attachment_media` still decide what can be shown. Copies are named by a hash of what identifies the file on its host (session and link, or owner and attachment id), so each video is copied once. Videos over 100 MiB are not copied, and the oldest copies are deleted once all of them would pass 512 MiB.
- **Attached videos.** A video chip (mp4, m4v, mov, webm, mkv, ogv, as `video_mime`) did nothing when clicked, on this Mac too: `classify` gave it no viewer. It now opens in the viewer as a video. The new `attachment_media { owner, id }` grants the host's snapshot of that attachment under the same owner check as `attachment_preview`, and the window copies and plays it the same way on any computer (an attachment is at most 10 MiB).
- Both sides need this version: a host without the two commands answers an error, and the viewer shows it.
