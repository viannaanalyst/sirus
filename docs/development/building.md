# Building and running Sirus

Sirus is three parts: the **window**, a GPUI app in `native-spike/` ([ADR-189](../decisions/ADR-189-sirus-native.md)); the **host** `sirus-host`, the Rust back end in `src-tauri/` that the window starts and the phone talks to ([ADR-131](../decisions/ADR-131-headless-linux-host.md)); and the **iPhone app** in `mobile/` ([ADR-135](../decisions/ADR-135-react-native-iphone-app.md)). There is no Tauri or React app any more ([ADR-191](../decisions/ADR-191-react-app-removal.md)).

## Requirements

- Rust stable (built with 1.98), Xcode command line tools, Git; macOS for the window
- Node.js 22.18+ for the release script and the phone app
- Optional: `codex`, `claude`, `opencode`, `cursor-agent`, and/or `grok` on `PATH` to run agents

## The window and the host

```bash
cd src-tauri && cargo build --release --no-default-features --features host --bin sirus-host \
  # with CARGO_TARGET_DIR=../native-spike/target/host, as release-sirus.ts does
cd native-spike && cargo build --release && ./bundle.sh     # Sirus.app
```

`bundle.sh` wraps the window, the host and `assets/reply-render` (the committed Mermaid and KaTeX files that replies use, see its `build/build.sh` to update them) as `Sirus.app`, signed with the local identity. The window starts the host on `127.0.0.1:7811`; other devices reach the Mac only while Settings → Conexões allows it (port 7711, HTTPS on 8443).

## Checks

```bash
cd native-spike && cargo test --features gpui/test-support && cargo build --release
cd src-tauri && cargo fmt --check
cd src-tauri && cargo clippy --no-default-features --features host --all-targets -- -D warnings
cd src-tauri && cargo test --lib --no-default-features --features host
node --test tests/*.test.ts                      # the release script's helpers
cd mobile && npx tsc --noEmit && npm test
```

Use `CARGO_TARGET_DIR=../native-spike/target/host-check` for the `src-tauri` commands to keep them out of the release host build. Format only the files you touched. Rust tests use real temporary Git repositories and owned subprocesses.

## Data on disk

The window keeps its state in `~/Library/Application Support/com.siruscode.native/` (`state.db`, transcripts, worktrees, settings files). Its first launch copies Sirus's `com.siruscode.app` state when that folder exists (`sqlite3 .backup`, with automatic work off) and starts empty when it does not. Removing a project from the sidebar deletes metadata only, not the user's folder.

For isolated validation, set an absolute `SIRUS_DATA_DIR` for a launched host. Never launch a second host against the data folder of a running app.

## Headless host (Linux server)

`sirus-host` also runs without a window on a Linux server and serves the phone app. On the server (Rust, a C compiler, Git, the provider CLIs logged in as the same user, and `cloudflared` pointing at `127.0.0.1:7710`, ADR-155):

```bash
cd src-tauri
cargo build --release --no-default-features --features host --bin sirus-host
./target/release/sirus-host serve
./target/release/sirus-host pair            # in another shell: code + QR for the phone
```

`serve` listens on loopback, port 7710 (`--port`), and answers only loopback peers; other devices arrive through the tunnel. `remote-public.json` in the data folder names the public address for pairing links. State lives in `~/.local/share/sirus-code` (`--data-dir`, or `SIRUS_DATA_DIR`). The host has no web app: a browser gets a short plain-text reply, and only `/assets/*` (the Mermaid and KaTeX files, `--assets-dir`) is served. `sirus-host devices` lists paired devices and `sirus-host revoke <id>` removes one. Provider CLIs are found on `PATH`, `~/.local/bin`, `~/.npm-global/bin`, `$NPM_CONFIG_PREFIX/bin` and nvm's Node versions. From a Mac, `cargo zigbuild --no-default-features --features host --bin sirus-host --target aarch64-unknown-linux-gnu` cross-builds the Linux binary.

A systemd user unit is enough to keep it running:

```ini
[Service]
ExecStart=%h/sirus/sirus-host serve
Restart=on-failure
[Install]
WantedBy=default.target
```

## Opt-in provider smoke tests

These tests make real inference calls using existing CLI authentication, inside owned temporary Git/worktree fixtures. They are excluded from ordinary tests; no authentication files are changed. Run a specific provider intentionally:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features host --lib codex::tests::live_codex -- --ignored --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features host --lib claude::tests::live_claude -- --ignored --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features host --lib opencode::tests::live_opencode -- --ignored --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features host --lib live_opencode_workspace -- --ignored --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features host --lib live_cursor_analysis -- --ignored --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features host --lib live_cursor_workspace -- --ignored --nocapture
cargo test --manifest-path src-tauri/Cargo.toml --no-default-features --features host --lib live_claude_catalog -- --ignored --nocapture
```

The Codex and Claude native fixtures exercise exact continuation and host approval refusal; Claude also approves one reviewable Write. Legacy raw-CLI smoke tests remain opt-in. `SIRUS_SMOKE_MODEL` optionally selects an actual CLI model for the legacy workspace fixtures. Cursor uses Auto-review with sandbox enabled (requires a compatible CLI); the edit test verifies actual safe writes. `live_claude_catalog` initializes the CLI without a user prompt or inference. Tests cap execution, stop only their owned process group on timeout and assert the original checkout is unchanged. Passing does not guarantee all account models or every vendor permission flow.

## Native iPhone app

A React Native / Expo app in `mobile/` ([ADR-135](../decisions/ADR-135-react-native-iphone-app.md)). It needs Xcode, an iPhone and a free Apple ID (Personal Team).

```bash
cd mobile
npm install                          # also applies the patches and copies GhosttyKit (GHOSTTY_KIT_DIR)
npx expo prebuild --platform ios --no-install && (cd ios && pod install)
xcodebuild -workspace ios/Sirus.xcworkspace -scheme Sirus -configuration Release \
  -destination 'generic/platform=iOS' -allowProvisioningUpdates build   # then devicectl install
npm run publish:update               # JavaScript-only changes reach the installed app from this Mac
```

`mobile/.env.local` (untracked) holds `SIRUS_UPDATE_URL`, the Mac's `https://…/api/app-update/manifest`. Bump `RUNTIME_VERSION` in `mobile/app.config.ts` when native code changes. The LaunchAgent `com.siruscode.renew-iphone` runs `mobile/scripts/renew-iphone.sh` to renew the 7-day signature over Wi-Fi. The first time, enable Developer Mode on the iPhone and trust the developer under Settings → General → VPN & Device Management.

`npx tsc --noEmit` and `npm test` (Node's test runner over `tests/`) check it. The protocol code it shares with the Mac lives in `mobile/src/shared` and is imported as `@sirus/…` ([ADR-191](../decisions/ADR-191-react-app-removal.md)); a protocol change updates it by hand.

## Releases

`npm install` at the repository root installs only the Tauri CLI, whose `signer` signs the update archive. `node scripts/release-sirus.ts` ([ADR-140](../decisions/ADR-140-in-app-updates-and-releases.md)) is a dry run: it checks the tree, runs the window's tests (skip with `--skip-tests`), builds the host and the window, bundles `Sirus.app`, packs `Sirus-arm64.dmg` and `Sirus-arm64.app.tar.gz`, signs the archive and checks the signature against `native-spike/updater.pub`, and writes `sirus-latest.json` under `native-spike/target/release-upload/sirus-v<version>/`. `--publish` creates the GitHub release (tag `sirus-v<version>`, never marked latest) and replaces the feed on the fixed `sirus-channel` release; it needs `gh` signed in. The version is the one in `native-spike/Cargo.toml`.

The update signing key is `~/.config/sirus-code/updater.key` (no password; `--key` or `SIRUS_UPDATER_KEY` point elsewhere, `SIRUS_UPDATER_KEY_PASSWORD` if it has one). Keep a backup: without it installed apps cannot accept new updates. Only its public half is in `native-spike/updater.pub` (the app embeds it); it must never change, and key files are ignored by Git.

The app is signed with "Sirus Code Local Signing" when that identity is in the keychain (ad-hoc otherwise) and is not notarized, so a downloaded DMG opens the first time through System Settings → Privacy & Security → Open Anyway. The website is the static `website/public/` folder, served by Cloudflare Workers static assets; publish it with `cd website && npx wrangler deploy` (run it inside `website/`).

## Measuring CPU and memory

With Sirus open, `native-spike/scripts/measure.py <label> --seconds 60` reads CPU and memory, appends to `native-spike/target/perf/readings.jsonl` and shows the difference from the last reading of that label; `--budget <percent>` fails when median CPU is over it. Compare only readings taken in the same state: `idle` with no agent working, `working` with one replying, `hidden` with the window covered.
