# ADR-196: Hugeicons on Mac and phone, code as pills, ⌘F marks the word

- Status: accepted (replaces the icon set of ADR-177)
- Date: 2026-11-06

## Context

The owner chose the reference app's icons. Inline code read as a faint band, and ⌘F washed the whole turn instead of marking the word.

## Decision

- **Icons.** `native-spike/assets` takes the reference app's files where it has the name and Hugeicons' free set otherwise; provider marks and the three filled shapes stay. The phone draws every symbol, on iOS too, from the generated `mobile/src/components/AppSymbol.icons.tsx` over `react-native-svg`, and Tabler is gone. This replaces SF Symbols on iOS.
- **Code pills (`markdown.rs`).** A paragraph with inline code flows word by word, as the reference app's `rich_inline`, with each piece of code in a pill: Menlo at 11.2 px, 6 px radius, 8% ink, and a file icon when it names a file. Selection records the pieces with a marker (`GLUE`) so copying keeps the line whole.
- **⌘F.** `markdown::set_search` marks the matches themselves, accent on the current turn and amber on the others. The current turn opens a fold that holds the word. The counter and the arrows still step through turns, not single occurrences.
- **Third sound.** The host emits `pr-watch-activity` when a watched pull request wakes its session, and the window plays the reference app's linked-activity chime, unless sounds are off.

## Consequences

Hit marks and pills rebuild each frame for the paragraphs on screen, which is why a plain paragraph keeps its single run of text. Selection in a settled turn covers its final answer; `reply_texts` now records that same text so the pieces line up.
