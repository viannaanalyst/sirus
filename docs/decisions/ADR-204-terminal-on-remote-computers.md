# ADR-204: The terminal and project actions on a remote computer

- Status: accepted
- Date: 2026-11-10

## Context

Sirus Native can switch the whole window to another computer (ADR-133), usually a headless `sirus-host` on a Linux server. The Terminal (side panel, bottom strip and title-bar tabs) opened a PTY on this Mac, so on a remote computer it was switched off with "Disponível só neste Mac…", and so were project actions, which run their command in a terminal of their own. The host already runs shells for the phone: `start_terminal`, `write_terminal`, `resize_terminal` and `stop_terminal`, keyed by session and terminal id, with the shell (`$SHELL`, else `/bin/bash` on Linux) started in the session's folder on the host and the project's Infisical keys added there, and the output sent as `pty-output` / `pty-exit` events.

## Decision

- **The shell runs on the host; the window draws it.** On a remote computer each new terminal (`panel_terminal.rs` `Terminal::new`) gets `Pty::Remote` (`remote_pty.rs`) instead of a local PTY. It calls `start_terminal` for the conversation's session on the host and feeds every `pty-output` naming its terminal id into the same engine (alacritty or Ghostty), so drawing, keys, selection and links are unchanged. The engine's replies (device status, colour queries) and the keyboard go back with `write_terminal`, resizes with `resize_terminal`; `pty-exit` ends it with `[process exited]`. Closing the terminal sends `stop_terminal`. On "Este Mac" nothing changes: no remote link, the same local PTY.
- **One connection, held.** The terminal holds the connection it started on (`backend::RemoteLink`, the active computer's `Link`), which survives that link's reconnects and is closed for good when the window switches computers, so a key can never reach another computer's shell. Leaving a computer drops its title-bar terminal tabs (`drop_remote_title_tabs`) as well as its panes. Stream events (`pty-output`, `pty-exit`) go only to the terminals' listeners (`subscribe_all`), not through the window's event queue.
- **In order.** The host runs each command as its own task, so two writes sent together could land out of order. One thread per terminal sends its requests one at a time and waits for each answer, joining what was typed meanwhile into the next write and keeping only the last of consecutive resizes (`coalesce`).
- **Busy, and which agent.** A new host command, `terminal_foreground`, returns the command line of the process group in front of the PTY, or null while the shell itself is there (`tcgetpgrp` through `process_group_leader`, then `ps`). The window asks once a second, as it asks the local PTY, so closing a remote terminal running something asks first, a project action reuses only an idle terminal, and a tab running an agent CLI shows its mark. A terminal that just took an action counts as busy until the next answer. An older host without the command reads as idle.
- **No orphan shells.** Terminal ids are `sirus-<install>-<launch><g generation>-<n>`: `<install>` is eight hex characters kept in the window's data folder (`terminal-prefix`), `<launch>` is random per launch, `<generation>` counts `connect_remote` calls. On each connection, after `ready`, the window sends `stop_terminals { prefix: "sirus-<install>-", keep: <this connection's prefix> }` (reply dropped). The host stops every terminal under the prefix but not under `keep`: the shells this window left from a dropped connection, a quit, or a switch whose `stop_terminal` was lost. The phone's terminals and another Mac's never match. `stop_terminals` refuses a prefix shorter than eight characters or a `keep` that does not extend it.
- **Project actions** (`shell_actions.rs`) run through the same terminals, so they need no change beyond lifting the guard; a title-bar tab is reused only if it is on the active computer.
- **No session, no shell.** The host keys terminals by session; a window with no conversation open shows "Abra uma conversa para usar o terminal neste computador." in the terminal.

## Consequences

- Typing has a round trip per batch; on a slow link keys arrive in bursts but in order.
- Output sent while the link is down is lost (the shell keeps running and the stream resumes after the reconnect); there is no scrollback replay.
- Files and Changes stay Mac-only on a remote computer.
- Shells stopped by `stop_terminals` are only the window's own, and only on its next connection to that computer; until then they count against the host's limit of eight terminals per session.
- The host starts the shell as it does for the phone (no login shell), so a server's `~/.profile` additions may be missing from it.
