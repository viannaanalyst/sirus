# AGENTS.md

Sirus is a Mac app for running coding agents (Claude Code, Codex, OpenCode, Cursor, Grok and more) side by side: **projects → sessions → agents → worktrees → reviewable changes**. It has three parts: a native GPUI window (`native-spike/`), the Rust back end it drives, `sirus-host` (`src-tauri/`), and an iPhone app (`mobile/`). The earlier Tauri + React app is gone (ADR-191).

Small and focused changes land best. Use your judgement; when something looks wrong, fix it or say so.

## Get it running

You need a current stable Rust toolchain, Xcode command line tools on macOS, Node.js 22.18+ (release script and phone app), and at least one provider CLI installed and logged in.

```bash
cd src-tauri && CARGO_TARGET_DIR=../native-spike/target/host cargo build --release --no-default-features --features host --bin sirus-host
cd native-spike && cargo build --release && ./bundle.sh     # Sirus.app
```

## Where things live

- `native-spike/src/` — the window: `shell.rs` (title bar, tabs, sidebar, transcript, composer), `backend.rs` (starts and pairs the host, WebSocket client), `settings_pages.rs`, `panel_*.rs`, `reply_render.rs` (Mermaid and KaTeX in replies). `native-spike/README.md` has the map.
- `native-spike/assets/reply-render/` — the committed Mermaid and KaTeX files the window loads; `build/build.sh` rebuilds them.
- `src-tauri/src/` — the host's Rust modules, one per area (`commands.rs` and `commands.list` for the command table, `agent.rs` / `codex.rs` / `claude.rs` / `opencode.rs` for providers, `git*.rs`, `worktree.rs`, `persist.rs`, `remote.rs` for the socket and pairing, …).
- `mobile/` — the iPhone app (React Native / Expo). `mobile/src/shared/` is the protocol code it shares with the Mac, a cut-down copy imported as `@sirus/*`.
- `scripts/release-sirus.ts` — releases; `native-spike/updater.pub` is the update key's public half.

For the full map — every module, the domain model, commands and events, provider argv, Git and security choices — see [`docs/development/architecture.md`](docs/development/architecture.md). Decisions and their reasons are in [`docs/decisions/`](docs/decisions/README.md).

## Before you push

```bash
cd native-spike && cargo test --features gpui/test-support && cargo build --release
cd src-tauri && cargo fmt --check && cargo clippy --no-default-features --features host --all-targets -- -D warnings && cargo test --lib --no-default-features --features host
cd mobile && npx tsc --noEmit && npm test
node --test tests/*.test.ts     # release script helpers
```

Use `CARGO_TARGET_DIR=../native-spike/target/host-check` for the `src-tauri` commands so they do not touch the release host build.

## Working notes

- Keep shared types aligned between Rust `models.rs` and `mobile/src/shared/client/types.ts` (`camelCase` JSON); a protocol change updates the phone's copy by hand.
- Shared Rust code imports `AppHandle`, `Emitter`, `Manager`, `State` and `async_runtime` from `crate::runtime`, never from a UI framework (ADR-131). A new host command goes in its module and in `src-tauri/src/commands.list`.
- Phone JavaScript reaches the iPhone over the air (ADR-135 addendum): `cd mobile && node scripts/publish-update.mjs` publishes it to the Mac host (served at the public route's `/api/app-update/manifest`), and the app picks it up at its next launch. A full build (`renew-iphone.sh --force` in the spike after `SIRUS_UPDATE_URL=https://native.siruscode.com/api/app-update/manifest npx expo prebuild --platform ios`) is still needed for native modules, `app.config.ts`, icon or splash changes, and each one also bumps `RUNTIME_VERSION`.
- Persistence is one SQLite database in the app data folder (`state.db`: settings, projects, sessions, one row per message, …; ADR-114). `persist.rs` is the interface; `store.rs` implements it.
- The owner often works in the repo at the same time: format only the files you touched.
- Ask before destructive Git operations (force-push, `reset --hard`, deleting branches or worktrees).
- Notable features get a short ADR in `docs/decisions/`, and the architecture reference is updated when the map changes.
- Labels are never forced into capital letters (no `uppercase`).
- Documentation is in English; the product UI is localized (English and Portuguese).
- Project skills live in `.sirus/skills/`.
- Design previews (HTML mockups to choose a direction) are temporary: build them outside the repo (for example under `/tmp`), never commit them, and delete them once the owner has chosen. `previews/` is ignored.
- Delegating to subagents: pick by difficulty with the agents in `.claude/agents/` — `simples` (Haiku, high effort) for mechanical tasks, `intermediario` (Sonnet, medium) for contained features and fixes, `complexo` (Opus, medium) for cross-cutting work and reviews. When unsure between two, take the cheaper one and escalate if it stalls.
- Button placement: in every page header, dialog, card, sheet, popover and toast, the primary action (Salvar, Criar, Nova tarefa, Enviar, Confirmar…) sits at the far right, secondary actions (Cancelar, Agora não) just left of it, and tertiary actions on the left. Page headers put their main create button on the right too.
