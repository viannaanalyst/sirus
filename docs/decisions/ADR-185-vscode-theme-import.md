# ADR-185: Import VS Code themes

**Status:** Accepted (2026-10-28).

## Context

Settings → Appearance offered Light, Dark and System, with the Glass palette (ADR-033, ADR-167), a
Pierre-based code palette for diffs and Markdown (ADR-169, ADR-179) and a fixed terminal ANSI palette.
Owners asked for the colour themes they already use in VS Code. Another open-source client (github.com/pingdotgg/another open-source client, MIT)
imports them from Open VSX: it searches open-vsx.org, downloads the `.vsix`, extracts the theme JSON,
follows `include` chains and hardens the download. This ADR ports that design to Sirus.

## Decision

- **Rust does the network and the zip.** another open-source client runs in a browser, where the renderer is the only place to
  do it. Sirus has a trusted native side, and the renderer is the part that renders Markdown, previews
  and other untrusted content, so the safer split is:
  - `src-tauri/src/vscode_theme.rs` is the only code that talks to Open VSX or opens a package. The renderer's
    CSP (`connect-src 'self'`) stays closed to the internet, and the renderer receives only a small validated
    value (`VsCodeTheme`), never bytes from a package.
  - Network calls go only to `open-vsx.org` over HTTPS, plus `openvsx.eclipsecontent.org`, the CDN Open VSX
    redirects package downloads to (observed with `curl -I`). The client follows at most 3 redirects and only
    to those two hosts; every URL taken from an Open VSX response is checked the same way (HTTPS, no
    credentials, port 443).
  - A decompression bomb cannot run in the UI process, and entries are read with `Read::take`, so a lying
    header cannot expand past the cap either.
- **Hardening (another client's limits).** 20 MiB `.vsix`, 100 MiB uncompressed in total, 5,000 entries, 200x
  compression ratio, all read from the central directory before any entry is read; 256 KiB per theme file and
  manifest, 512 KiB search and 256 KiB detail responses, 40 themes per extension, `include` depth 8 with cycle
  detection and a bounded number of resolved files, entry names with `..`, absolute paths or NUL are refused,
  and an `include` cannot climb out of `extension/`. Two deliberate differences: the ratio is judged only for
  entries of 1 KiB or more (padding in a tiny file is not a bomb, and the totals still cap everything), and
  the package's SHA-256 is checked against Open VSX's published checksum, as another open-source client does. The packaged
  `package.json` must match the chosen listing (publisher, name, version, licence).
- **Licence allowlist.** `0BSD`, `Apache-2.0`, `BSD-2-Clause`, `BSD-3-Clause`, `CC0-1.0`, `ISC`, `MIT`,
  `MPL-2.0`, `Unlicense`, as in another open-source client, from the Open VSX detail document and again from the packaged manifest.
  A file the owner picks from disk is theirs to use and is not licence-checked; it keeps every size and
  path limit.
- **No new dependencies.** The back end already had `zip`, `reqwest`, `sha2` and `serde_json`; a small
  JSONC reader (comments, trailing commas, BOM; strings untouched) replaces `jsonc-parser`, and the renderer
  needs no `jszip`. No new licence files or notices were needed beyond the existing another open-source client entry.
- **What is kept.** Only 66 workbench colours Sirus maps (`COLOR_KEYS`) as `#rrggbb[aa]`, and TextMate rules
  reduced to scope, foreground and font style (at most 400 rules, 16 scopes of 128 characters each). Anything
  else, including `semanticTokenColors` and external `.tmTheme` references, is dropped.
- **Storage.** `AppSettings` gains `vscodeThemes` (at most 48 themes, 48 KiB each), `lightVscodeTheme` and
  `darkVscodeTheme` (ids). They are saved with the rest of the settings in `state.db` and validated by
  `AppSettings::validate` (`validate_stored`): unique ids of the form `vsc-<16 hex>`, closed colour keys, valid
  colours and scopes, and an active id must exist and have the slot's kind. The renderer's `mergeSettings`
  applies the same rules to what it loads (`vscode-theme.ts`). Types are aligned in `models.rs` and `types.ts`.
- **Mapping (`src/lib/vscode-theme.ts`).**
  - Surfaces: `editor.background` is `--background-0`; `sideBar.background` (or a step toward the ink colour)
    is `--background-1`; two more steps give `--background-2/3`; the widget/menu colour is the elevated
    surface. Hairlines are alpha tints of the text colour unless the theme's own border shows (contrast of
    1.15 or more on the editor).
  - Text: `foreground` is lifted to 7:1 on every surface; secondary and muted derive from it and keep 5:1 and
    4.5:1. Accent (`button.background`, then `focusBorder`, `textLink.foreground`) and the status colours
    (error, added, warning, info, purple) get 4.5:1 on the three base surfaces.
  - Code: each of Pierre's "highlights" keys takes a role (comment, string, keyword, function, ...), each role
    a list of TextMate scopes, resolved with TextMate precedence (longest selector, later rule wins).
    Colours get 5:1 on the code surface, comments 3.2:1; anything the theme does not colour takes its default
    text colour, so no Pierre colour leaks into a theme.
  - Terminal: `terminal.*` colours (falling back to `editor.*`), foreground 7:1, cursor 3:1, ANSI 3:1 (the
    black of a dark theme and the white of a light one 2:1, since they are meant to sit near the background),
    and Sirus's own ANSI colours the theme leaves out are held to the same floor. An opaque selection colour
    is capped to 40% alpha so it never hides the text.
  - The lift is an OKLCH lightness move that keeps hue and chroma, so a guarded colour still looks like the
    theme's colour.
- **Applying it.** `applyAppearance` sets the tokens as inline custom properties on `<html>` for the palette in
  use (the existing snapshot in `localStorage` restores them at launch), and `code-theme-state.ts` holds the
  syntax palettes so `@pierre/diffs` stays out of the main bundle; `diff-theme.ts` registers them lazily
  under a name that includes a hash of their content, and the diff workers, code blocks and highlight cache
  follow it. Glass is untouched: window, sidebar and popup materials still `color-mix` the themed tokens with
  transparency (the one hard-coded sidebar colour in `appearance.css` now reads `--sidebar-base`).
- **Light and dark are separate slots.** A theme goes only into the slot of its kind (`uiTheme`, then the
  file's `type`, then the editor background's luminance), which keeps the palette and the tokens that depend
  on it (shadows, ink tints, the Sirus mark) consistent. High-contrast variants map to their base kind.
- **Default (Sirus) is the default and resets everything.** Choosing it in a slot, or Restore defaults,
  clears the selection, which removes every inline token, the syntax theme and the terminal palette. Installed
  themes are kept until removed.
- **UI.** Settings → Appearance → VS Code themes: a theme picker per slot, a search panel that expands and
  collapses with animation (query, sort, results with licence and downloads, a preview that downloads and
  shows each theme as a miniature in its own colours, Install and Install and use), "From a file…" for a
  `.vsix` or `.json` (a native picker that remote devices cannot use), and the installed list with Remove. UI
  strings are in `src/i18n/vscode-theme-strings.ts` (English and Portuguese).

## Consequences

- Themes are static data once installed: they work offline and survive Open VSX changes. Updating one means
  installing it again (same id, so it replaces in place).
- Syntax colours depend on TextMate scopes in the theme; themes that rely on semantic highlighting alone look
  plainer than in VS Code. The regex colours of non-Pierre code (languages Pierre lacks) still use the default
  palette.
- The workbench colour list lives in two places (`COLOR_KEYS` in Rust and the keys read by `vscode-theme.ts`);
  a key added in one needs the other.
- Verified by Rust tests (zip limits, include resolution, sanitising, host allowlist, storage validation) and
  `tests/vscode-theme.test.ts` (mapping, contrast floors, terminal, storage and reset).

Credit: the design, limits and licence allowlist come from another client's Open VSX theme import (MIT, T3 Tools Inc.).
