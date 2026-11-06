# ADR-170: Terminal on Ghostty's VT core, drawn on a canvas

**Status:** Accepted (2026-10-28).

## Context

This is the fourth step of the premium redesign (after ADR-167). The terminal ran on xterm.js with
its fit and web-links add-ons. It parsed and drew in JavaScript on the main thread, took its glyph
widths and ligature handling from the DOM, and needed `allowTransparency` (a slower composite) for
window glass. Prompt icons (Powerline, Nerd Font) depended on a locally installed patched font.

Another open-source client (MIT, already credited in `THIRD-PARTY-NOTICES.md`) replaced xterm with `libghostty-vt`,
the VT core of Ghostty (MIT), compiled to WebAssembly, with a Canvas 2D renderer. Ghostty parses
escape sequences, tracks grapheme clusters and Unicode widths, encodes keyboard and mouse input
(including the Kitty keyboard protocol) and answers terminal queries itself.

## Decision

- `src/terminal/ghostty/` is ported from another open-source client: `runtime.ts` (one WebAssembly instance), `core.ts`
  (per-terminal handles and render snapshots), `renderer.ts` (Canvas 2D, batched backgrounds and
  style runs), `surface.ts` (input, IME, selection, scrolling, sizing, links, cursor blink),
  `keyCodes.ts`, `links.ts` and `platform.ts`. Its tests came along, using Vitest because they need
  Vite's `?raw` and `?inline` imports and mocks (`vitest.config.ts`, run by `npm test`).
- The prebuilt `vendor/ghostty-vt.wasm` (about 630 KB) and `vendor/ghostty-write-pty.wasm` are
  committed as read-only assets, with the pinned revision in `native/libghostty-vt/VERSION`, its
  headers and its MIT license. `npm run build:ghostty-wasm` (`scripts/build-libghostty-wasm.sh`)
  rebuilds them from that revision; it pins Zig 0.15.2 and downloads it into
  `~/.cache/sirus-code` unless a matching `zig` is on `PATH`. `runtimeAbi.test.ts` checks the
  artifact's embedded revision against `VERSION`.
- The symbols-only Nerd Font (`fonts/SymbolsNerdFontMono-Regular.woff2`, MIT, license beside it) is
  registered lazily as a fallback, so prompt glyphs render without an installed Nerd Font. Vite
  emits the wasm and the font as assets; the production CSP adds `'wasm-unsafe-eval'` to
  `script-src` (the dev CSP already had it).
- Sirus keeps `TerminalPanel.tsx` as the owner of the shell lifecycle, tabs and the PTY transport
  (`pty_term.rs` and the client are unchanged). Each live terminal now holds a
  `GhosttyTerminalSurface`, created asynchronously; the shell starts once the surface exists so no
  output is dropped, with the surface's own columns and rows.
- Sirus specifics live in `src/lib/terminal-theme.ts`:
  - `ghosttyTheme` maps `terminalAppearance` (the Sirus ANSI palette, light and dark) to Ghostty's
    16 palette slots, foreground, cursor and selection.
  - Window glass (ADR-102) sets `transparentBackground`: the canvas is created with alpha and
    cells with the default background are cleared instead of painted, so the glass shows through.
    Cells with their own background, reverse video included, stay opaque and use the theme color,
    never black. The dock-surface tint (`terminalSurface`) still decides the color and
    transparency once styles apply.
  - `terminalFontFamily` passes only the chosen family (a canvas font string rejects keywords such
    as `ui-monospace`); the surface appends SF Mono, Menlo and Nerd Font fallbacks. A family that
    is not monospace falls back to the default stack.
  - The cursor setting (block, bar, underline) applies when a program has not chosen a shape of
    its own with DECSCUSR; blinking follows Ghostty's state and `prefers-reduced-motion`.
- Behavior kept: ⌘C copy and ⌘V bracketed paste, selection (double and triple click), the "Add to
  chat" selection button, scrollback with a scrollbar, http(s) links and OSC 8 hyperlinks opening
  through `openLinkFrom` (ADR-149) with the hovered link feeding the right-click menu, IME,
  focus on show, several terminals per session and the detach and re-attach of a live terminal
  (ADR-104). Hidden tabs stop drawing. ⌘-chords other than copy, paste and select all are left to
  the app's shortcuts.
- `@xterm/xterm`, `@xterm/addon-fit` and `@xterm/addon-web-links` are removed. `vitest` is added as
  a dev dependency for the ported tests.

## Consequences

- Parsing and key and mouse encoding run in WebAssembly; drawing is a few `fillRect` and
  `fillText` batches per dirty row, with no per-cell DOM. Text is ligature-free, widths follow
  Unicode (CJK, emoji, combining marks) and Nerd Font prompt glyphs render out of the box.
- The bundle gains about 630 KB of wasm and 1.2 MB of font, fetched when the first terminal opens.
- Scrollback is read when a terminal is created, so changing the setting applies to terminals
  opened afterwards. The font, size, cursor shape and theme update live.
- There is no search or in-terminal clear action; the panel never had one.
- Selection, text and links are not DOM, so the webview's find and native text services do not see
  the terminal; the hidden textarea carries input and IME.
- Updating Ghostty means changing `VERSION`, rebuilding the wasm and running the ABI tests; the
  header files in `native/libghostty-vt/include` must be refreshed with it.
- The terminal element keeps the `xterm` class only because `DiffViewer`'s keyboard guard names it;
  the app's own checks use `.terminal-canvas` and `.terminal-input`.
