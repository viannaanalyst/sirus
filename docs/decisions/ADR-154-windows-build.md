# ADR-154: Windows build, starting with a compile check and an installer on demand

**Status:** Accepted (2026-10-19).

## Context

Sirus is built and released on an Apple silicon Mac only (`scripts/release.ts`), and the app had never been compiled for Windows. People asked for a Windows `.exe` in Releases. The macOS-only parts (WKWebView browser, computer use, native notifications, Keychain, cookie import) already sit behind `cfg(target_os = "macos")`, and the terminal uses `portable-pty`, which supports ConPTY, so the question was how much of the rest assumes Unix.

## Decision

- **A workflow, not the release script.** `.github/workflows/windows.yml` runs on `windows-latest` (the repository is public, so the runner is free). On every push to `main` that touches `src-tauri/` it runs `cargo check --all-targets` for the desktop app and writes the error and warning counts to the run summary, with the plain log as an artifact. Started by hand (`workflow_dispatch`), it also builds an unsigned NSIS installer (`npx tauri build --bundles nsis`) and uploads it as an artifact.
- **The first compile pass.** The desktop app failed with seven errors: three tests that drive Unix tools (`/bin/cat` in a process group, `libc::kill`, symlinks) are now `cfg(unix)`, and the non-macOS browser stub gained the profile argument that ADR-150 added to the macOS one. With those, the desktop app compiles for Windows; about 175 warnings remain (code that only macOS uses).
- **No Windows headless host.** `sirus-host` (ADR-131) is a server for Linux and macOS; its control socket (`host.sock`) and signal handling are Unix-only, so the workflow does not build it for Windows.
- **Not yet in Releases.** The installer is a test artifact until someone runs it on Windows: nothing here has been run on Windows yet, the updater's `latest.json` lists macOS only, and the installer is not code-signed, so SmartScreen warns on first launch.

## Checking from a Mac

`cargo check --target x86_64-pc-windows-gnu` works from macOS with zig as the C compiler for `x86_64-windows-gnu` (a wrapper that drops the Rust triple cc-rs passes) and a stand-in `x86_64-w64-mingw32-windres` that writes an empty resource object, since a check never links. It reproduced the runner's errors exactly. It is a local aid only; the runner (MSVC) is the reference.

## Consequences

- Back-end changes now show Windows compile errors on `main` within minutes.
- Open work before a release: run the installer on Windows and fix what fails at run time (agent spawning, the MCP bridges' Unix sockets, paths), decide on the warnings and on clippy for Windows, add a `windows-x86_64` entry to the updater, and decide on code signing.
