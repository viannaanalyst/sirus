# ADR-168: The composer on TipTap, with `@file`, `/skill` and links as inline chips

**Status:** Accepted (2026-10-28).

## Context

Step 2 of the premium redesign (after ADR-167). The prompt box was a `<textarea>` with a painted
mirror behind it (ADR-037, ADR-149): `/skill` and `@file` tokens and web addresses were colored
text, never a real element. The mirror had to wrap exactly like the textarea, so a token could not
have an icon, a short label or a unit of its own, and the caret walked through it character by
character.

Another open-source client (MIT, already credited in `THIRD-PARTY-NOTICES.md`) builds its composer on TipTap with
inline chips. Its structure was the reference; the code here is written for Sirus, so no
third-party source was copied.

## Decision

- The composer is a TipTap 3 editor over one paragraph (`src/components/ComposerEditor.tsx`).
  The schema (`src/lib/composer-extensions.ts`) has text, line breaks and one atom, the chip. No
  marks, lists, headings or code blocks.
- The draft stays a plain string (`composerDrafts[draftKey]`). A chip keeps the characters it
  stands for in `text`, a line break is `\n`, and `docToText` reads the document back as exactly
  what the textarea held. The pure helpers in `src/lib/composer-doc.ts` do both directions, so the
  round trip is tested in Node (`tests/composer-doc.test.ts`).
- Which text is a token is still decided by `composerPaintSegments` (`/skill`, `@file`,
  `@"quoted path"` and web addresses). The editor turns a token into a chip once the caret leaves
  it: while the caret touches a token it stays text, which is what the `@`/`/` list reads, and a
  directory being browsed (`@src/`) stays text. A token glued to a neighbouring chip stays text so
  the characters read back the same. Nothing converts while an input method is composing. A draft
  restored from the store, a recalled prompt or a stash shows all its tokens as chips.
- A chip shows an icon and a short label (`a.ts` for `@src/lib/a.ts`, `acme/app#12` for a GitHub
  pull request URL) tinted with the accent; links are quieter. The caret moves over a chip as one
  unit and Backspace right after it (or Delete right before it) removes it whole. The CSS is the
  `.composer-chip*` block in `composer.css`. The `.composer-token-*` classes remain for the
  transcript's sent messages.
- The store is the truth. Typing pushes the text to the store; a change that did not come from
  typing (recall, stash, quick reply, send clearing the draft, dictation) replaces the document and
  puts the caret at the end. Switching conversation builds a fresh editor state so undo never
  reaches into another draft. Undo and redo (`UndoRedo`) work inside one draft.
- The `@`/`/` list keeps working in plain-text offsets (`use-composer-suggestions.ts`). The editor
  reports the text and the caret as offsets; choosing a row replaces just the token range, so undo
  survives and the caret lands after it. `activeComposerTrigger` holds the rules for when the list
  may open (focus, no composition, a fresh reading), tested apart from React.
- Paste: files, images and paths still go through the existing attachment flow (owner captured,
  "draft unchanged" check). Everything else is inserted as plain text, so HTML formatting never
  reaches the draft. Copying out of the editor puts the original text on the clipboard.
- Other surfaces no longer look for `textarea[data-draft-owner]`. They use
  `src/lib/composer-focus.ts`: editors register by draft owner, and `focusComposer(owner)`,
  `insertIntoComposer(owner, text)`, `composerFor(owner)` and `composerWithin(pane)` find them.
- New dependencies: `@tiptap/core`, `@tiptap/react`, `@tiptap/pm`, `@tiptap/extension-document`,
  `-paragraph`, `-text`, `-hard-break` and `@tiptap/extensions` (undo/redo), all ^3.31.3 (3.32.0
  resolved). The starter kit is not used.

## Consequences

- Chips are real elements, so they can carry icons and later previews or hover cards without a
  mirror to keep in step.
- The bundle grows by TipTap and ProseMirror. The composer loads with the main view, so the cost is
  paid at start; it is not code-split.
- The editable element is a `contenteditable` with `role="textbox"`, not a `textarea`. Code that
  tests for a textarea must use `[data-draft-owner]` or the focus registry. The phone's focus
  tracking and the 16px anti-zoom rule were updated.
- A token typed at the very end of a draft stays text until the caret moves on or the editor
  blurs; one restored from the store is already a chip. Both read back the same string.
- Editing inside a chip is not possible: it is replaced or removed whole.
