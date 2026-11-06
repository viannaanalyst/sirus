# ADR-194: The changes card, drawing on a draft picture, and a tree closer to Monocode

- Status: accepted
- Date: 2026-11-06

## Context

The files a reply changed showed as one faint line under it, easy to miss. Long text in the composer did not follow the caret, pictures waiting to be sent carried a name and a size nobody needed, sent pictures could show as empty tiles, and the old app's drawing on a picture before sending was lost with it. The activity tree gave every step a coloured tile, which Monocode's native app gives only to a group's header.

## Decision

- **Changes card (`turn_review.rs`).** A card under the reply with the totals, Undo, Keep and Review, and the first three files. "Show more" opens the rest and a file opens its diff in place. Keep only hides the card.
- **Composer.** The text scrolls to keep the caret's line in view whenever the caret moves; the wheel stays free otherwise.
- **Pictures.** A draft picture is only its image with a cross in the corner. A sent picture falls back to its draft's small copy, and the host is asked for one only when no copy exists.
- **Drawing (`image_markup.rs`).** The viewer offers a pencil, undo, clear and "Replace original" on draft pictures. Strokes are kept in the picture's proportions and burned into a new PNG off the UI thread; the new attachment takes the original's place and the original is released. Sent pictures stay read-only.
- **Tree.** Only a group's header has a tile: it spins while running, with the chevron under the pointer. Steps are bare text on the rail, at 14 px on 28 px rows. The live window's edges fade with Monocode's alpha mask instead of a colour laid over the glass.
- **Smaller matches.** Show more sits inside the prompt bubble, the sidebar's options menu takes Monocode's size and neutral switch, and the queue card and the jump button use the frosted surfaces instead of an opaque fill.

## Consequences

Keep does not persist across restarts, as no host command records it. Burning a stroke re-encodes the picture as PNG, so a photo can grow; the usual attachment limit still applies.
