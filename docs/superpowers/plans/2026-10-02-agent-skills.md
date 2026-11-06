# Agent skills implementation plan

**Goal:** Bring grouped, cross-provider skill settings search/preview/composer selection into Sirus's existing design.

**Architecture:** Native bounded discovery reads only SKILL.md files under fixed skills roots and owned project/worktree roots. A closed catalog/preview IPC accepts owned IDs, never filesystem paths. Enable/disable preferences belong to AppSettings and affect Sirus's catalog and explicit skill invocation only; provider installations are untouched. Native prompt preparation resolves an explicit leading `/skill-name` invocation, prefers the selected provider's copy, and inlines bounded instructions without modifying the persisted visible prompt or approval policies.

**Constraints:** Existing Client/Transport boundary; no new dependencies, credentials, shell/fs capabilities, installer, skill execution or file deletion. No Computer Use: inspect both local reference repositories and validate through code/tests/build. No Git metadata is available in this checkout.

- [x] Implement native discovery, grouping, frontmatter parsing, source IDs, preview and explicit invocation with temporary-directory regressions for bounds, symlinks, duplicate precedence and disabled skills.
- [x] Wire the closed command and aligned types; persist bounded disabled-name preferences through existing save_settings.
- [x] Add Agent skills settings with search, source/provider metadata, compact switches, read-only preview, refresh and page-wide restore.
- [x] Add a searchable skills page to the compositor's Add menu; selecting a skill adds its visible invocation to the current owner's draft without sending.
- [x] Review trust boundary, run frontend/native checks and build the signed desktop app; document actual behavior and limitations.

Validation: typecheck, ESLint, frontend unit tests and real-component SSR fixtures passed; cargo fmt and strict all-targets Clippy passed; native suite passed (206 tests, 17 existing ignored). Independent security review findings were fixed, including retained-boundary reads and provider/profile catalog invalidation. Graphify was incrementally updated. The desktop bundle was built and signed locally. No Computer Use was performed.
