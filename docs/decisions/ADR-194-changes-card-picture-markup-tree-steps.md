# ADR-194: The changes card, drawing on a draft picture, and a tree closer to Monocode

- Status: accepted
- Date: 2026-11-06

## Context

The files a reply changed showed as one faint line under it, easy to miss. Long text in the composer did not follow the caret, pictures waiting to be sent carried a name and a size nobody needed, sent pictures could show as empty tiles, and the old app's drawing on a picture before sending was lost with it. The activity tree gave every step a coloured tile, which Monocode's native app gives only to a group's header, and it pooled a turn's prose after its steps, where Monocode keeps them in order and folds the work once the turn ends.

## Decision

- **Changes card (`turn_review.rs`).** A card under the reply with the totals, Undo, Keep and Review, and the first three files. "Show more" opens the rest and a file opens its diff in place. Keep only hides the card.
- **Composer.** The text scrolls to keep the caret's line in view whenever the caret moves; the wheel stays free otherwise.
- **Pictures.** A draft picture is only its image with a cross in the corner. A sent picture falls back to its draft's small copy, and the host is asked for one only when no copy exists.
- **Drawing (`image_markup.rs`).** The viewer offers a pencil, undo, clear and "Replace original" on draft pictures. Strokes are kept in the picture's proportions and burned into a new PNG off the UI thread; the new attachment takes the original's place and the original is released. Sent pictures stay read-only.
- **Tree.** A turn is a list of beats in the order they happened, built from the reply's text, each step's `offset` and each steer's `offset` (`data.rs`). While it runs, groups of steps and prose appear inline; the running group spins and opens by itself, inside a window that follows the newest step and fades at its edges with Monocode's alpha mask. Settled groups stay closed until clicked. When the turn ends, everything before the final answer folds under "worked for"; the header opens it on a rail. A turn with no steps before its answer has nothing to fold. Steps are bare text at 14 px on 28 px rows; only a group's header has a tile.
- **Instructions.** A steer shows as the person's own bubble where it arrived in the reply, with its pictures. The window used to ignore `message.steers`, so "Instruct now" reached the agent but never appeared.
- **Footer.** Usage has a refresh button beside its numbers. "Not a git repository" from a project without Git is no longer reported as a problem.
- **Smaller matches.** Show more sits inside the prompt bubble, the sidebar's options menu takes Monocode's size and neutral switch, and the queue card and the jump button use the frosted surfaces instead of an opaque fill.

## Consequences

Prose from before the answer is not selectable with the transcript's selection, which covers the answer. Keep does not persist across restarts, as no host command records it. Burning a stroke re-encodes the picture as PNG, so a photo can grow; the usual attachment limit still applies.
