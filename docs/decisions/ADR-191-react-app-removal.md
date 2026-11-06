# ADR-191: The React and Tauri app is removed; the phone carries its own shared code

- Status: accepted
- Date: 2026-11-06

## Context

Sirus (the GPUI window in `native-spike/`, ADR-189, and the iPhone app in `mobile/`, ADR-135) replaced the Tauri and React app, and 0.1.0 is published. The old app was still the source of three things: the phone's pure client code (`@sirus/*`, mapped by Metro and `tsconfig.json` to `../src`), the `dist/` bundle the host served (including the Mermaid and KaTeX chunks the window's hidden page loads), and the `desktop` Cargo feature with Tauri.

## Decision

- **The phone owns its shared code.** `mobile/src/shared/` holds exactly the modules the phone reaches (client types and transport, protocol helpers in `lib/`, and the strings it shows), copied with their relative layout and cut down to the declarations the phone uses. `@sirus/*` points there in `tsconfig.json` and `metro.config.js`; `src/shared` imports only itself, with relative paths. The `@/*` alias is gone. `i18n` is one generated file with the keys the phone uses (Portuguese, English fallback). The root tests that covered those modules were cut down and moved to `mobile/tests/`, run by `npm test` with `mobile/scripts/register-test-loader.mjs`. A protocol change updates the copy by hand.
- **Mermaid and KaTeX are committed.** `native-spike/assets/reply-render/` holds a fresh build of Mermaid 12.1.0 and KaTeX 0.16.47 (stable entry names `mermaid.js`, `katex.js`, `katex.css`, hashed chunks and woff2 fonts, 6 MB) with their licences (`LICENSES.txt`, summarised in `THIRD-PARTY-NOTICES.md`). The earlier `dist/assets` chunks could not be reused as they were: Mermaid's entry imported the React app's main chunk. `build/build.sh` rebuilds them in a temporary folder. `bundle.sh` copies the folder to `Contents/Resources/reply-render`; it no longer runs npm. A test checks that every file the entries reach exists and that nothing in the folder is unreached.
- **The host serves only `/assets/*`** from `--assets-dir` (`SIRUS_ASSETS_DIR`, else `reply-render/` beside the binary). The root answers a short plain-text line and every other path a plain `Not found.`; `POST /api/pair` and the socket API are unchanged.
- **Web push is gone** (`remote/push.rs`, `/api/push`, `/api/push/identity`, `web-push-native`, the VAPID fields of `remote.json`, the device list's `push` flag). Only the browser PWA used it: the iPhone app has no push code and the window never called it.
- **The updater key** moved from `tauri.conf.json` to `native-spike/updater.pub` (the same key; installed apps verify against it). `app_update.rs` embeds the file and `scripts/release-sirus.ts` verifies against it. A tiny root `package.json` keeps only `@tauri-apps/cli` for `tauri signer`. `~/.config/sirus-code/updater.key` is unchanged.
- **Tauri is removed** from `src-tauri`: the `desktop` feature, `main.rs`, `tauri.*.json`, capabilities, plists, icons, the desktop-only modules (`app_update`, `devices`, `browser_recording`, `window_reveal`), every `cfg(feature = "desktop")` item and the `tauri*` crates. The crate is the host only (`host` is the default and only feature). The command table is generated from `src-tauri/src/commands.list` instead of Tauri's `generate_handler!`; the set is unchanged (129 commands; web push had none).
- **Deleted:** `src/` (909 files), the HTML entries, Vite, Vitest, ESLint, root `tsconfig*`, `public/`, `native/libghostty-vt`, `scripts/` except the release script and its helper, the old root tests except the release helpers', `.github/workflows/windows.yml`.

## Consequences

- Sirus no longer needs Sirus Code's `com.siruscode.app` folder. The first launch copies its state when the folder exists and otherwise starts empty (this used to fail); usage history only adds Sirus Code's account profiles when present; the iOS simulator hub looks for its own install first.
- What deleting `com.siruscode.app` costs: the host's login checks never read it (a named account's profile lives in the host's own data folder, `provider-accounts/<provider>/<id>`; the default account uses `~/.claude` and `~/.codex`), so Sirus's own logins stay. Lost: the Sirus Code-only named accounts (on this Mac `Trabalho`, which Sirus does not list, so it would be added and signed in again), the past usage history that comes from those profiles' logs (`SIRUS_USAGE_EXTRA_ACCOUNTS`), the pinned iOS simulator hub install (Sirus installs its own on demand), and the folder a session's worktree path still points at if it was made by Sirus Code. Sirus Code itself, if still installed, loses everything.
- Keychain items ("Sirus Code ..." logins and connectors) are separate from that folder: the window's host keeps its own and copies a Sirus Code item only when macOS allows it without a prompt.
- Code written for the Tauri window and no longer reached remains in `src-tauri` under `#![allow(dead_code)]` until it is removed module by module.
