# Sirus

A native Mac app for running coding agents side by side: Claude Code, Codex, OpenCode, Cursor, Grok and more.

**Projects → sessions → agents → worktrees → reviewable changes.**

![Sirus: a Claude session adding a dark mode toggle, with its changes open in the side panel](docs/assets/screenshot.png)

Sirus is a window written in Rust with [GPUI](https://www.gpui.rs) (Metal), on top of `sirus-host`, a headless back end that also serves the iPhone app. Website: [siruscode.com](https://siruscode.com).

## Features

- **Many agents, one place.** Each session runs a provider CLI you already have installed and signed in to. Sessions run in parallel, across projects.
- **Isolated worktrees.** A session can work in the current checkout or in its own Git worktree, so agents don't step on each other.
- **Reviewable changes.** Every turn shows the files it changed. You can undo them, open the diff, stage and commit from the side panel, and create a pull request.
- **Terminal, files and browser.** A side panel next to the conversation has a terminal, a file viewer and editor, the changes and a browser the agents can drive.
- **Pull requests and usage.** A Pull requests page, and each provider's quota and cost at a glance.
- **Connectors.** Remote MCP servers (Supabase, Linear, Notion…), with the sign-in handled by the app.
- **iPhone and other devices.** Pair an iPhone with a QR code and follow or steer your sessions from anywhere. The host also runs headless as `sirus-host`.
- **Local data.** There is no Sirus account and no required cloud. State lives in one SQLite database in the app's data folder. Each CLI keeps its own sign-in.
- **English and Portuguese** interface.

## Install

1. Download [`Sirus-arm64.dmg`](https://github.com/viannaanalyst/sirus/releases/download/sirus-channel/Sirus-arm64.dmg) (Apple silicon, macOS 13 or later).
2. Open it and drag Sirus to Applications.
3. The app is not notarized by Apple yet. The first time, right-click Sirus in Applications and choose **Open**, then confirm. If macOS still refuses, choose **Open Anyway** in System Settings → Privacy & Security.

After that it opens like any app and updates itself from the same release channel.

You also need Git and at least one provider CLI, installed and signed in.

## Getting started

1. Open Sirus and add a local Git folder as a project.
2. Start a conversation (⌘N), pick a provider and model, and choose the current checkout or an isolated worktree.
3. Send an instruction, then follow the output, the changes and the terminal.

Removing a project from the list never deletes files on disk.

## Build from source

You need a current stable Rust toolchain, the Xcode command line tools and, for the phone's web UI that the bundle ships, Node.js 22.18+.

```bash
# The host, from src-tauri (feature "host": no window)
cd src-tauri
CARGO_TARGET_DIR=../native-spike/target/host cargo build --release --no-default-features --features host --bin sirus-host

# The window, from native-spike
cd ../native-spike
cargo build --release
./bundle.sh
open Sirus.app
```

`bundle.sh` signs with a local identity and builds the phone's web UI once. See [`native-spike/README.md`](native-spike/README.md) for the layout of the window's code and [`docs/development/building.md`](docs/development/building.md) for the rest of the build and checks. Tests: `cargo test --features gpui/test-support` in `native-spike`.

## iPhone app

The iPhone app lives in [`mobile/`](mobile/). It talks to your Mac (or a server running `sirus-host`) through your own Cloudflare Tunnel; nothing goes through a server of ours. Pair it from Settings → Conexões in Sirus.

## Documentation

- [`AGENTS.md`](AGENTS.md): how to work in this repository
- [Architecture](docs/development/architecture.md): modules, domain model, providers, Git and security
- [Decisions](docs/decisions/README.md) and [all docs](docs/README.md)

## License

[MIT](LICENSE). Code adapted from other open source projects keeps its original notices in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md).
