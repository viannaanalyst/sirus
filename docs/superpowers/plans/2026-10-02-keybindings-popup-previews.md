# Keybindings and popup-material previews

**Goal:** Bring a searchable keybinding reference/editor into the existing closed Sirus command map, and preview Dark/Light with optional glass.

**Architecture:** Keep centralized keyboard dispatch and existing customShortcuts persistence. Extend only real workspace panel actions and the aligned native allowlist. Reuse Arc's recorder, with page-wide restoration and no individual reset control. Preview-only appearance proposal separates palette (Dark/Light) from material (solid/glass); retained production settings remain unchanged pending visual selection.

**Constraints:** No Computer Use, new dependencies, IPC/capabilities, free-form commands or context expressions. Existing Command/reserved-key policy remains. No Git metadata exists in this checkout.

- [x] Add searchable grouped keybinding page, descriptions/default/changed states and page-wide restore; conflict messages name the occupied action.
- [x] Wire Files, Browser and Environment through the centralized controller; keep native/frontend closed map aligned and test workspace context guards.
- [x] Create browser-accessible popup previews in four palette/material combinations, with real dropdown, tooltip, popover and dialog interactions and opacity adjustment.
- [x] Run frontend/native checks, review relevant changes, update Graphify and build desktop. Verify the live HTTP preview URL using a request, without UI automation.

Validation: typecheck, ESLint, 142 frontend unit tests and localized real-component SSR checks passed; cargo fmt, strict all-targets Clippy and native suite passed (206 tests, 17 existing ignored). Independent code-only review found no important issues. Graphify was updated incrementally. Desktop build and strict signature verification passed. Preview HTTP page/assets returned 200; unlisted paths returned 404. Production appearance preferences were not migrated; desktop glass in the preview is simulated.
