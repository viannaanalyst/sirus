# ADR-199: Ghostty's libghostty-vt as a second, switchable terminal engine

- Status: accepted
- Date: 2026-11-09

## Context

The Terminal panel parses the shell's output with `alacritty_terminal` and draws the grid in GPUI. Ghostty's core is a stronger VT implementation (reflow on resize, Kitty keyboard and graphics, grapheme clustering, synchronized output) and since 2026 it ships a supported C ABI, `libghostty-vt`, for embedding the parser and screen without Ghostty's renderer or app.

T3 Code (our fork at `~/Projetos/t3code`) already runs on it: `native/libghostty-vt/` pins a Ghostty commit (`VERSION`) and keeps the headers and the MIT license, `apps/web/scripts/build-libghostty-wasm.sh` builds it with Zig 0.15.2 (downloaded into a cache by `ensure_zig`), `apps/web/src/terminal/ghostty/core.ts` drives the C ABI and turns the render state into snapshots for its canvas, and `apps/mobile/modules/t3-terminal/` bridges the same library on iOS and Android.

## Decision

- **An engine trait behind the renderer.** `terminal_engine.rs` defines `TerminalEngine`: feed bytes, resize, a viewport `Snapshot` (cells with character, colours, flags and selection; the cursor), scroll, modes (application cursor, bracketed paste, alternate screen and alternate scroll, mouse tracking, cursor shown), title, PTY replies, selection and the wrapped line under a point (for links). `AlacrittyEngine` wraps the existing `Term` with unchanged behaviour; `GhosttyEngine` (`ghostty_vt.rs`) wraps `libghostty-vt`. `panel_terminal.rs` draws either from the snapshot, so the GPUI drawing, colours, cursor styles and link handling are shared.
- **The PTY.** Alacritty keeps its own event loop. Ghostty has none, so `DirectPty` reads the PTY on a thread, feeds the engine, writes its replies (device status, attributes, colour and size queries) back before the keyboard's next bytes, and reports wakeups and the shell's exit as the same events the panel already handles.
- **Same revision as T3.** `native-spike/vendor/libghostty-vt/VERSION` pins `9f62873bf195e4d8a762d768a1405a5f2f7b1697`, the commit T3 Code ships; the headers and Ghostty's MIT `LICENSE` sit next to it. `native-spike/scripts/build-libghostty-vt.sh` clones that commit into `~/.cache/sirus`, downloads Zig 0.15.2 there when no matching `zig` is on `PATH`, and builds the static library for macOS arm64 into `vendor/libghostty-vt/lib/` (ignored by Git). On macOS SDKs whose libSystem stubs are arm64e-only or whose `math.h` needs a newer clang, it builds against a shim SDK (the newest installed SDK with usable headers plus Zig's libSystem stubs); Sirus links only the static library.
- **Off by default.** The cargo feature `ghostty` declares the FFI and makes `build.rs` link the library; without it the app builds and runs as before and never needs Zig. Settings → Terminal → Motor do terminal (`terminalEngine`: `alacritty` | `ghostty`, default `alacritty`) lists Ghostty only in builds that carry it and applies to terminals opened after the change. A saved `ghostty` in a build without it, or an engine that fails to start, falls back to alacritty.
- **The bindings are hand-written** for the subset Sirus uses, from the pinned headers (`ghostty_terminal_*`, the render-state row and cell iterators, grid refs, selection and formatting). Bumping the pin means re-reading those headers.

## ABI gaps at this revision

- OSC 52 has no callback, so programs cannot read or write the clipboard on the Ghostty engine.
- Cells name palette indices and Sirus paints them with its own palette, so colours a program sets with OSC 4 are not drawn (queries are answered from Sirus's palette).
- Selection lives in Sirus, in screen coordinates, and is handed to the terminal on each change; when a full scrollback trims its oldest lines a held selection can drift, and a resize clears it.
- Keys and mouse reports stay on the shared encoder (`key_bytes`), not Ghostty's key and mouse encoders.

## Rollout

The default stays alacritty. Release builds keep the feature off until the Ghostty engine has been used day to day; then the release script builds the library and turns the feature on, and a later decision can make Ghostty the default. The phone keeps its own terminal for now; T3's mobile bridges show the path if it moves to the same core.

## Consequences

Terminal behaviour can be compared engine against engine in the same window. Tests cover both engines through the trait (text, colours, cursor moves, resize and reflow, alternate screen, modes, replies, scrollback and selection; the Ghostty ones only with the feature). The price is a Zig toolchain for builds that want Ghostty and a hand-kept FFI that must follow the pin.
