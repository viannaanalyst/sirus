# Editor repair and turn review previews

## Scope

Repair actual manual editing of existing workspace code/Markdown. Study turn summaries, diffs and checkpoint revert, and provide interactive visual options before implementing native turn checkpoints.

## Steps

- [x] Verify the CodeMirror style/CSP path and preserve newer edits across save acknowledgments.
- [x] Supply the existing document style nonce to CodeMirror, use palette-aware syntax tokens, retain aligned gutters and correct code scrolling/indentation.
- [x] Replace unsafe timed/unmount writes with explicit Save/Command-S; retain owner buffers and confirm closing dirty inline files.
- [x] Trace completed-turn file summaries, turn/file diff navigation, file-only undo and thread/message rollback. Document exact behavior and missing Sirus native checkpoint capability.
- [x] Create three preview summary options with dark/light/System/glass, actual in-memory CodeMirror code/Markdown editing, split/unified right-side diff and simulated undo/revert confirmations.
- [x] Run meaningful regressions, typecheck/lint/full tests, cargo check, desktop build/signature and a focused code review. Update ADR-019 and incremental Graphify.

No new IPC or real rollback is implemented by these previews. Fixtures are isolated and never operate on user repositories.

## Verification

- Typecheck, lint, 154 Node tests and all component/CodeMirror SSR checks pass.
- Shared save-gate regression covers concurrent mounted views, later text, failure retry and removed/recreated buffers.
- Isolated headless Chromium regression passes actual CodeMirror layout and manual edits/Command-S under nonce-only style CSP, Markdown, popup glass, three summary options and separate mock Undo/Keep/Revert flows.
- The negative control removing the editor nonce reproduces the stacked layout (`display: block` instead of `flex`).
- Preview build/typecheck and http://localhost:4195/ pass.
- Native cargo check, desktop build and strict code-signature verification pass.
- Focused reviewer found no remaining important issues after the shared Save gate and startup nonce changes.
- Incremental Graphify updated; its existing TypeScript parser warning for app-store remains, while actual TypeScript checks pass.

The app bundle was rebuilt and signed, without restarting the user's running app. Native checkpoint/rollback remains a proposal, not a live command.
