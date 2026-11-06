# Session notifications implementation plan

**Goal:** Bring Notifications settings to Sirus and add independently configurable approval, question and completion sounds.

**Architecture:** Native lifecycle events alone produce alerts. One closed `notification_action` command reports authorization, explicitly requests it, previews an allowlisted sound or sends a fixed test. React uses the existing Client and Arc controls; settings persist through `save_settings`.

**Constraints:** No frontend Tauri imports, shell/fs plugins, renderer-generated notification text, renderer paths or approval responses. No alerts on restored history, cancellation or failure. Original Dock artwork is unchanged. This checkout has no Git metadata; execute inline without commits/worktrees.

- [x] Add closed persisted notification preferences with default migration and page-only reset. Test invalid sounds, independent event selection and unrelated preference preservation.
- [x] Add native macOS UserNotifications authorization/delivery and native closed NSSound presets (Glass, Ping, Pop, Submarine, Tink, Hero). Recheck authorization before automatic banners; do not prompt automatically. Keep native clicks owner-validated and all notification identifiers/counts bounded.
- [x] Hook only fresh native pending callbacks and settled successful completions in `codex.rs`/`agent.rs`. Deduplicate by process/request or user-turn identity. Revalidate the retained session before delivering; never include transcript/tool content. Suppress system/sound while foreground unless explicitly enabled.
- [x] Add Notifications navigation and a standard Settings page with Arc dropdowns, compact switches, sound previews, native permission state and a test action. One Restore defaults button at the top. Deliver app notices through existing store and Arc surfaces; clicks select the owned session without approving anything.
- [x] Test migration, dedupe, lifecycle exclusions, callback identity, settings reset and real SSR controls. Run TypeScript/lint/tests, Rust fmt/tests/Clippy and desktop build/signature checks. Document the IPC review in ADR-034 and update Graphify incrementally; record external check failures separately below.

## Verification evidence

- TypeScript, ESLint, all 130 Node tests, 100/100 Arc preview renders and the dedicated localized Notifications SSR checks passed.
- `cargo test` passed: 186 passed, 17 intentionally ignored live-provider tests. `cargo fmt --all -- --check` passed.
- Two-pass notification review found no remaining high/medium issues after expiry, stable toast timers and final native delivery checks were corrected.
- Canonical `npm run build:desktop` produced the app with Sirus Code Local Signing; `codesign --verify --deep --strict` passed. No app launch, OS authorization change, banner delivery or audible playback was performed.
- Graphify was updated incrementally. Its AST parser reports partial extraction for the store's inline imported TypeScript types; TypeScript itself passed.
- Full-tree Clippy was run but is not green: the latest run reports unused `browser_mcp::provider_env` at `browser_mcp.rs:71` in concurrent native browser work. Earlier browser dictionary and borrow errors were corrected. These are outside Notifications; the remaining MCP integration belongs to that work.
- Native permission prompts, actual macOS banners, click navigation and audible output remain interactive acceptance checks through Allow/Test/Preview in the signed app.
