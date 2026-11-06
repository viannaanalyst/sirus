# ADR-036: Native-owned skill discovery and explicit portable invocations

**Status:** Accepted

## Context

Skills live in several CLI-specific and shared directories. Sirus needs a searchable view and explicit draft selection without changing external providers or exposing arbitrary filesystem reads. A grouped catalog with preview and composer selection is the interaction reference.

## Decision

`skills.rs` discovers `SKILL.md` in fixed provider/shared user directories, the owned project/worktree and native app-data `skills` folder. Selected named Codex/Claude profiles replace that provider's default user root. Project Pi uses `.pi/skills`; user Pi uses `.pi/agent/skills`. Other private account profiles and plugin caches are not enumerated.

The closed `skill_action` IPC offers Catalog and Preview through Client/Transport. It accepts owned IDs and native-derived source IDs, never paths. Roots must remain within their project, home, profile or app-data boundary. Reads use the existing jailed regular-file helper and verified handles. Discovery runs on workers; ownership is rechecked before publication. Names are grouped while source metadata is retained.

Limits are 512 names, 2,048 documents, 8,192 entries, four nested levels and 64 KiB per regular UTF-8 document. Frontmatter parses scalar/multiline names/descriptions without arbitrary YAML tags. Symlink cycles and foreign project root redirects cannot expand authority.

`disabledSkills` persists bounded names. Switches hide picker entries and reject explicit disabled invocations; they do not uninstall skills or control vendor automatic discovery. Page-wide Restore defaults affects only this preference.

The Add menu inserts leading `/skill-name` into the owner's unsent draft. Send resolves up to four distinct leading known names, preferring project sources, then the selected provider, shared `.agents`, portable Sirus and foreign copies. Selected documents and reference directories are inlined within an aggregate 64 KiB limit. Provider/cwd/account/preferences are rechecked before turn mutation. Visible persisted user text and existing execution validation, approvals and process policies remain unchanged. Unknown slash commands remain vendor commands. This delivers instructions rather than executing scripts in the host.

Settings uses Arc primitives, compact switches, theme tokens and text-only previews. Owner/provider/account/workspace changes invalidate pending requests. Refresh is explicit; no polling is introduced.

## Consequences

- **Positive:** A cross-provider catalog and explicit portable workflows without vendor mutation or arbitrary renderer filesystem authority.
- **Negative:** Toggles cannot prevent vendor auto-discovery. Invalid, oversized, deeper and plugin-cache documents are omitted; scan truncation is reported.
- **Accepted trade-off:** Instructions are snapshots at send time; relative asset access depends on the provider and existing approvals. No installer, skill editor, deletion or automatic host execution is added.

## Alternatives considered

- Discovery in React: violates native authority and Client/Transport.
- Renaming/copying installations to enforce switches: mutates external user configuration and profile isolation.
- Sending the whole enabled catalog every turn: activation is not invocation, and unbounded context distorts prompts.
