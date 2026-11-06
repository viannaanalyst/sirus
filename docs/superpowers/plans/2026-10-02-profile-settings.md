# Local Profile Implementation Plan

> **For agentic workers:** Use `executing-plans` inline. The owner approved the
> The supplied sharing reference. No further design gate is
> needed. This workspace has no Git metadata; do not create commits/worktrees.

**Goal:** Add a local Profile settings page and a working activity-card share
dialog using Sirus's existing design and real retained activity.

**Architecture:** Derive retained prompt activity from owned native Sessions,
excluding imports and fork-inherited prefixes. Local profile preferences follow
the existing settings save path. One typed native PNG export command owns
clipboard writes, a native PNG save picker and three fixed social composers.

**Tech Stack:** React 19, existing Arc/Radix dialogs and inputs, Zustand, CSS
tokens, bounded native JSON preferences, AppKit and the existing dialog/opener.

## Global constraints

- UI → Client → Transport → Rust; no frontend Tauri imports or new frontend dependencies. Native PNG/JPEG
  decoding uses the bounded `image` codec and `crc32fast` for all PNG chunks.
- No app login, provider credential reads, fabricated tokens or skill run counts.
- Retained activity is not lifetime telemetry; unknown token totals show `—`.
- Provider/model/reasoning rankings explicitly describe current active-session
  selections, not historical turn attribution.
- Settings General reset preserves profile preferences.
- PNG export accepts bounded bytes, never a renderer path or URL. Native Save
  permits only the one user-chosen PNG destination and refuses symlinks.
- No Computer Use, browser launch by the agent, external posts or polling.

### Task 1: Native profile preferences and safe export

- [x] Add aligned `LocalProfile` preferences, bounded normalization/validation,
  safe legacy defaults and a native home-basename default display name.
- [x] Add `profile_image_action` with closed Copy/Save/X/LinkedIn/Reddit actions,
  bounded PNG signature/chunk/dimension validation, picker-owned writes and
  fixed external composer URLs. Register the command and typed Client method.
- [x] Test Unicode/control/name/handle/color/avatar validation, settings
  round-trip and oversized/truncated image rejection and symlink refusal.

### Task 2: Real activity selectors and the Profile page

- [x] Test retained owned prompt counts, fork prefixes/import exclusion, local
  date buckets/streaks, empty history and current-session selection shares.
- [x] Build `profile-stats.ts` pure selectors and settings/locale wiring.
- [x] Add centered local identity, five summary tiles, rolling nine-month
  activity heatmap, insights, model distribution and honest plugin/token states.
- [x] Add Edit with Arc inputs, avatar palette and bounded locally compressed
  photo; commit only on Save through existing serialized settings action.

### Task 3: Share card and verification

- [x] Draw the same locally rendered PNG for the preview and every export;
  include identity, Sirus brand, activity and real metrics on a white card.
- [x] Add the reference share modal with labelled circular actions, pending/error
  status, native Copy/Save and fixed social composer actions; preserve focus.
- [x] Verify real server renders, pure tests, TS/lint, native tests/check/Clippy,
  desktop build and signing; update the mapping/ADR/AGENTS and Graphify.

## Review

Scope covers the approved Profile page and sharing reference. Token telemetry
and lifetime deletion-preserving ledgers remain unavailable, explicitly shown;
there is no token estimation or imported-history inflation. Native export is
one bounded capability, not a filesystem/URL bridge. Preview and output use
identical PNG bytes. Implementation proceeds inline with existing host skills.

Validation: 116 frontend tests, 167 native tests (17 opt-in integration tests
remain ignored), localized real-component server renders, TypeScript, ESLint,
Rust formatting/check/Clippy and the signed desktop build passed. The signing
identity is Sirus Code Local Signing; deep/strict codesign verification passed.
No browser, app, clipboard or live social interaction was exercised by the agent.
