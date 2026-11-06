# ADR-067: Sirus identity

**Status:** Accepted

## Context

The product took the name **Sirus**, with a silver "S" orbit mark. Tauri derives the app-data directory from the bundle identifier, so the identifier change also moves where data lives.

## Decision

- **Identity.** `productName` is `Sirus`, the identifier is `com.siruscode.app`, the crate is `sirus-code` (lib `sirus_code_lib`) and the npm package is `sirus-code`. Internal names follow (`SirusClient`, `SIRUS_*` environment variables, `sirus_browser` / `sirus_computer` MCP servers, `sirus.session:` notification identifiers, the default branch pattern `sirus/{session-name}`).
- **No compatibility layer.** The app reads only its own identifier, `.sirus/skills` and its own `worktrees/` root. The previous name does not appear in code, docs, previews or data. The owner's existing data, worktrees and branches were migrated once by hand when this decision was taken; the app carries no migration code.
- **Artwork.** The bundle icon, the embedded Smoked Glass and White Dock alternatives, the in-app glyph and the favicon derive from the owner's mark. The 16–64 px bundle sizes use a higher-contrast copy of the mark. Dock selection stays the closed set from ADR-033: no user-supplied icon path or image.

## Consequences

- **Positive:** one name everywhere; no dead migration path to maintain.
- **Negative:** an install of the previous build on another machine would start empty. macOS treats the identifier as a new app, so Microphone, speech, Accessibility, Screen Recording and notification permissions are granted again.

## Alternatives considered

- **Keep a startup data move and a legacy worktree root.** Rejected by the owner: it keeps the previous name in code and on disk.
- **A user-supplied icon file, as Ghostty's `macos-icon = custom` offers.** Rejected: the renderer may not pass images or paths to the Dock (ADR-033).
