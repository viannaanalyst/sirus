# ADR-001: Tauri 2 + React/Vite, not Electron or Next.js

**Status:** Accepted

## Context

Sirus is a desktop host for coding agents. It must read the local filesystem, spawn processes, drive a PTY, and talk to Git. A website runtime (Next.js) cannot own those privileges. Electron would work but ships a full Chromium and a Node-shaped attack surface that we do not want for a tool that executes agents against user repositories.

## Decision

Build the product as **Tauri 2**: React + TypeScript + Vite in a webview, Rust in `src-tauri`. Identifier `com.siruscode.app`. Dev URL `http://localhost:1420`.

Lives in `src-tauri/tauri.conf.json`, `package.json`, `src-tauri/Cargo.toml`.

## Consequences

- **Positive:** small runtime, explicit IPC, Rust for privileged work, web UI for iteration speed.
- **Negative:** two language ecosystems; agents must not “fix frontend” by reaching into the OS.
- **Accepted trade-off:** macOS is the V1 target of record; Windows/Linux are possible later via Tauri but unverified.

## Alternatives considered

- **Electron:** rejected for size and implied Node IPC habits (`exec` from the renderer).
- **Next.js:** rejected; this is not a site and must not grow a server.
- **Pure native (Swift/AppKit):** would fit a Mac-only tool but would block a future remote web/iOS client sharing the same Client API.
