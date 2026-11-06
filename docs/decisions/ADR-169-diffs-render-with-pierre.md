# ADR-169: Diffs render with @pierre/diffs

**Status:** Accepted (2026-10-28).

## Context

Step three of the Glass redesign (ADR-167) is the diff. The viewer drew its own rows: a virtualized
list (ADR-145), a regex tokenizer running in a worker (`code-tokens.ts`), hand-made headers and
line-number buttons. It worked, but its colors were a coarse approximation of real syntax
highlighting, it had no word-level changes, no unmodified-lines separators, and every improvement
meant more custom layout code to maintain.

`@pierre/diffs` (Apache-2.0, by The Pierre Computer Company) is a diff renderer built for this: a
virtualized `CodeView` of many files, highlighting in a worker pool, sticky file headers, word-level
changes, hunk separators and line selection. Another open-source client (MIT) already uses it with version 1.6.1; its
theming and worker setup were the reference.

## Decision

- Diffs are drawn by Pierre's `CodeView` (`src/components/diff/DiffCode.tsx`), loaded on demand so
  the highlighter and its grammars stay out of the main bundle. Highlighting runs in a shared pool
  of web workers (`DiffWorkerPool.tsx`), which follows the app's light/dark theme, lives 30 s after
  the last diff closes, and falls back to the main thread if workers cannot start.
- `src/lib/diff-pierre.ts` maps our model to Pierre's. We still parse diffs into `DiffLine` rows
  (`diff-cache.ts`) because comments, review-guide anchors and find use row indexes. Pierre gets a
  patch built from the same text (untracked files become an all-added patch), and a selection in
  Pierre (side and line number) is translated to a range of our rows and back.
- Theming: dark uses Pierre's `pierre-dark`; light uses `sirus-light`, `pierre-light` with each
  syntax color darkened until it reads on tinted rows (`diff-theme.ts`). Surfaces, added and removed
  rows (`--success` and `--danger` mixed over the pane), headers, separators and selection come from
  the app tokens through `unsafeCSS`, and code takes `--code-font-family` and `--code-font-size`.
  The row height follows the code font size (26px on the phone's review).
- Find stays ours. Matches are computed on our rows (`diff-find.ts`, including files Pierre has not
  drawn). The viewer scrolls to the current match and paints matches inside Pierre's shadow roots
  with the CSS Custom Highlight API (`diff-find-dom.ts`), plus an attribute on the current line.
- Kept around Pierre, in `DiffViewer.tsx`: the file list, find bar, comment form, collapsed files,
  remembered scroll position and file in view, ⌘F ownership, Alt+↑/↓ between files, and ↑/↓
  (Shift extends, Esc clears) to move a chosen comment range. Collapse toggle and Copy diff are
  slotted into Pierre's file header. Files with no text diff (binary, omitted) keep a header with the
  reason instead of rows.
- Removed as dead: `diff-window.ts` (our virtualization), `diff-highlight.ts` and its worker, and
  `diff-document.ts` (the flat row document). `code-tokens.ts` stays for code blocks.
- Licenses: `@pierre/diffs`, `@pierre/highlights`, `@pierre/theming` and `@pierre/theme` are
  Apache-2.0; their texts are in `public/licenses/pierre/`. Transitive packages are MIT
  (`shiki`, `lru_map`, `hast-util-to-html`) and BSD-3-Clause (`diff`).

## Consequences

- Diffs look like a code review tool: real syntax colors, word-level changes, "N unmodified lines"
  separators that expand, sticky headers, in light and dark.
- A first diff loads about 430 kB (110 kB gzipped) of code, plus grammars as needed, instead of
  growing the main bundle.
- Lost with our renderer: selecting text by dragging across rows to start a comment (selection
  inside Pierre's shadow DOM is not readable that way) and Tab-focusing individual line buttons.
  Lines are chosen from the gutter (click, Shift-click, drag, and on a phone tap then tap again),
  and the arrow keys move the choice once there is one.
- Only the unified layout is offered, as before. Pierre also supports a split layout if it is wanted.
- Find highlights rely on the Custom Highlight API inside shadow roots. It was checked in Chromium;
  where it is missing, the current match's line is still marked and scrolled to.
