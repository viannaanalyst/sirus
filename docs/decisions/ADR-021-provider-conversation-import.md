# ADR-021: Read-only import of Claude Code and Codex conversations

**Status:** Superseded — removed on 2026-10-02. The landing banner, import dialog, `provider_import_catalog` / `import_conversations` commands and `provider_import.rs` were deleted. Existing sessions keep the persisted `importOrigin` field for compatibility; Sirus no longer reads Claude Code or Codex history directories.

## Context

Users with existing Claude Code or Codex history have no way to continue those
chats in Sirus. The CLIs' own history can be read and imported as threads.
That crosses the FS boundary: Sirus must read files outside its managed
projects while treating project contents as untrusted and never letting the
renderer choose a path.

## Decision

- `provider_import.rs` is the only reader of provider history. Sources are fixed:
  `$CLAUDE_CONFIG_DIR|~/.claude/projects/<project>/<uuid>.jsonl` for Claude Code
  and `$CODEX_HOME|~/.codex/sessions/**/rollout-*.jsonl` for Codex. The renderer
  supplies a provider id and conversation ids only — never a path.
- `provider_import_catalog` scans bounded transcripts (32 MiB per file, walk
  depth 4, 200 conversations) and returns title, cwd, updatedAt and message
  count. Claude UUID-named transcripts only; Codex `rollout-*.jsonl` only.
  Subagent/sidechain entries are skipped and directories are never recursed into
  beyond the fixed layout.
- `import_conversations` accepts at most 25 ids, parses only text blocks
  (200 KiB per message, 500 messages per conversation, 3 000 per batch),
  requires the recorded cwd to still exist, reuses or creates the matching
  Project, derives the branch from the real repository when available, and
  creates Completed sessions with `importOrigin = { provider, conversationId }`.
  Imported sessions never claim a native thread and are deduplicated by origin.
  One persist call commits the batch.
- The landing shows the Sirus glyph and a dismissible import banner that
  opens the import dialog (provider tabs, searchable list, multi-select).
  All toasts moved to the top-center below the window header.

## Consequences

- **Positive:** users can bring existing Claude/Codex conversations into
  Sirus and continue them; the file surface is fixed read-only CLI history
  directories with hard bounds; conversations can be re-imported safely because
  duplicates are skipped.
- **Negative:** imported transcripts are a best-effort text extraction (tool
  calls, images and non-text blocks are dropped), provider-native resume is not
  available for imported chats, and very large histories are truncated to the
  latest bounded messages.
- **Accepted trade-off:** reading CLI history means reading user files outside
  projects; this is limited to the two documented directories, bounded, and
  never exposed as a path-based IPC.

## Alternatives considered

- Import from a renderer-selected folder: rejected — the renderer must not
  choose FS paths.
- Copy the CLI's native session ids so chats resume natively: rejected — the
  transcript is reconstructed, not the vendor state, and vendor state differs
  per CLI.
- Background auto-import: rejected — import is an explicit user action.
