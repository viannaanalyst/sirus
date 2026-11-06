# ADR-195: Reveal motion, photos, sounds and "open in" follow Monocode, and sent pictures survive a restart

- Status: accepted
- Date: 2026-11-06

## Context

Comparing the window against Monocode's native app turned up a set of gaps, and some real defects: the caret overlay and the drawing canvas were absolute with no offsets, so they sat after their siblings, which broke the composer's caret follow and hid the strokes drawn on a picture. Sent pictures read only from the in-memory attachment store, so they vanished after a restart. `open_in_editor` and `app_icon_png` had been stubs since the React app went (ADR-191), so "open in Finder" did nothing and showed a folder for every app.

## Decision

- **Fixes.** The overlay and canvas are pinned with `top_0().left_0()`. A new host command, `message_thumbnail`, serves the copy kept with a message; the bubble reads it first, and the viewer falls back to it once the full picture is gone. `open_in_editor` runs `open -a`, and app icons come from the bundle's `.icns` through `sips`.
- **Motion (`disclosure.rs`).** Groups of steps and the "worked for" fold reveal with Monocode's `Disclosure`: the body is measured at its full height, its height follows `cubic-bezier(0.22, 1, 0.36, 1)` over 320 ms, it fades over 220 ms, and the chevron turns over 200 ms. A group that stops running closes the same way.
- **Tree line.** `rail_color` is 14% ink, translucent. Monocode mixes the same 14% with an opaque base, but on the window's glass a colour mixed from the opaque pane reads as a black line.
- **Photos.** A draft picture is a 36 px tile. A sent one keeps its shape with its longest side at 240 px and a 16 px radius. Images carry their own radius, since a parent clips only to a rectangle.
- **Sounds (`sound.rs`).** Monocode's synthesized cues replace the system sounds: one for a finished reply and one for a reply that needs the person. A saved system sound name maps to one of the two.
- **Reading.** Prose is 78% ink and emphasis 100%. List markers take a 24 px column, quotes a 4 px bar, code is 12/20 and tables 12/18.
- **Other.** "New" (⌘T) heads the sidebar. Usage restores its last reading from `usage-cache.json` at launch and asks the host to read the logs then. Provider marks always sit on their tile (`provider_mark`), since the glyph alone is white. The message trail moved to the left, by the owner's choice, which differs from Monocode.

## Consequences

Sound choices in stored settings no longer pick a sound of their own. Reopening a picture sent in an earlier run shows the kept copy, at most 1,280 px, not the original. Inline code keeps its highlight rather than Monocode's pill, which needs a separate element per run.
