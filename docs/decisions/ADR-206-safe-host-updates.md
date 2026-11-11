# ADR-206: Safe host updates on a Linux computer

**Status:** Accepted (2026-11-10).

- Amends: ADR-131 (headless host)

## Context

The VPS keeps `sirus-host` at the latest `main` with a root timer (`sirus-host-update`, every 10 minutes): fetch, rebuild when the host's code changed, install and restart. Two problems:

- It installed only when no `claude`, `codex` or `opencode` process was a child of the host. Agents stay alive between turns (ADR-107), so that almost never happened: a ready build waited for hours while nothing was actually running.
- A new host that failed to start, crashed in a loop or broke the database had no way back. The script lived only on the VPS.

## Decision

- **`sirus-host busy`.** A new control-socket request (`busy`) counts conversations in a turn or waiting on the person (`SessionStatus::is_active`: starting, running, waiting). The CLI prints the count and exits 0 when none, 1 when some, 3 when no host runs. A restart cuts exactly those; idle agent processes are restarted on the next turn anyway.
- **The updater in the repo.** `scripts/vps/sirus-host-update` (installed by `scripts/vps/install-updater.sh` with its service and timer) installs only when `busy` says 0 or no host runs. `--now` installs a waiting build anyway.
- **Copies before installing.** An online copy of `state.db` (`sqlite3 .backup`) goes to `update-backups/` in the data folder (0700, newest three kept), and the running binary to `/usr/local/bin/sirus-host.prev`.
- **Health check.** After the restart, `/api/health` must answer within 60 s and the service must then stay up for 20 s without systemd restarting it.
- **Rollback.** Otherwise the previous binary and the copied database come back (the WAL files are removed), the service restarts on them, and the commit is written to `~/.sirus-host-bad`, so the timer skips it until a newer one lands.

## Consequences

- A new host lands within ten minutes of the last conversation going quiet, instead of waiting for every agent process to exit.
- A broken host costs about a minute and a half, then the previous one is back with the database as it was before the update. What was written in between is lost, which is only the failed version's own start.
- The first install of a host with `busy` has to be done with `--now` (or by hand), since the running host doesn't know the request yet.
- The Mac app's host is updated with the app (ADR-140) and is not affected.
