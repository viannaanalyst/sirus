# ADR-207: "Atualizar tudo": the app, the CLIs and each computer's host in one place

**Status:** Accepted (2026-11-11).

- Amends: ADR-020 (provider CLI updates), ADR-140 (in-app updates), ADR-205 (provider updates card), ADR-206 (safe host updates)

## Context

Three things need updating, and each had its own place: the Mac app (Settings → Avançado, or its card), the provider CLIs on this Mac (their card), and the Sirus host on the owner's VPS (a root timer that came round every ten minutes, with no way to ask it from the window and no way to see where it stood). The CLIs on a paired computer could not be updated from the window at all: `provider_updates` and `update_providers` were this Mac's own commands (`computers::LOCAL_COMMANDS`).

## Decision

- **One section.** Settings → Avançado → Atualizações has a row per thing to update: "Este Mac" (the app, with its own buttons as before), "Provedores neste Mac", and for every paired computer "<nome>: Sirus" and "<nome>: provedores". "Atualizar tudo" is the primary button at the right of the section's heading. Each row says where it stands (Atualizado, Atualizando…, Compilando a versão nova…, Esperando N conversas terminarem, Instalado, Voltou para a versão anterior, Falhou) and offers "Tentar de novo" after a failure or "Instalar agora" while a host waits. Opening the page reads where every row stands, without updating anything.
- **Every paired computer, not only the active one.** One thread per computer: this Mac through the window's own connection to its host, every paired computer through a short-lived connection of its own (`computer_move::Oneshot`, as a move uses). So the CLIs on the VPS update while the window works on this Mac, and the other way round. `provider_updates` / `update_providers` stay local in the router; the update threads choose their connection themselves.
- **The CLIs** go through the host's own `provider_updates { refresh: true }` and `update_providers { confirm: true }` with the CLIs that have a newer version and a one-click update (ADR-020, ADR-198): the same fixed argv per install method, on that computer.
- **The Mac app** goes through app_update.rs as before: check, then download and install when there is a version. The restart stays the person's ("Reiniciar agora").
- **The host on a Linux computer.** A new host command, `host_update_action` (`status`, or `request { now }`), bridges the host (its own user) and the updater (root) with two files in the host user's home: the host writes `.sirus-host-update-request` (`check` or `now`, renamed into place), a systemd path unit starts the updater within seconds, and the updater writes `.sirus-host-update-status` (JSON: `phase`, `commit`, `running`, `waitingFor`, `at`, `message`; renamed into place, 0644). The window watches the status every 4 s until it settles; a status older than its request (`at` < `requestedAt`, or the request still there) is still the check starting. A host that restarts drops the connection; the next round connects again. The Mac's host answers `supported: false`: it updates with the app (ADR-140).
- **Asking before cutting conversations.** "Atualizar tudo" asks the updater for `check`, which installs only when no conversation is in a turn or waiting on the person (ADR-206). When a new build is ready and waits for N conversations, the window asks once: "Atualizar agora" (primary, sends `now`: installs and cuts them) or "Quando terminar" (the updater installs on its own when they are done). The row keeps "Instalar agora" for later.
- **The provider card.** Its "Atualizar tudo" starts the same run, with this Mac's CLIs limited to the offers on the card; the card's rows show their progress as before. While a run is already going it updates only its own offers.

## Consequences

- One click updates everything the owner runs, and the Settings rows show what happened on each machine, including a rollback on the VPS.
- A computer whose host predates `host_update_action` says so in its row; the first update there is the timer's (or `sirus-host-update --now`).
- The path unit must be installed once on each Linux computer (`sudo scripts/vps/install-updater.sh`).
- The status file is written by root into the host user's home through a temporary file and a rename, so a link left in its place is replaced, not followed. A request can only say `check` or `now`.
- The phone has no Atualizações section; it is not affected.
