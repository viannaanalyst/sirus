# ADR-205: One notification column, compact cards

- Status: accepted
- Date: 2026-11-11
- Amends: ADR-198 (Toasts), ADR-192 (cards on the Mac), ADR-090 (the ⌘Q notice)

## Context

The window had notifications in four places: failures and Git results at the top centre (`toast.rs`), provider CLI updates and the cards for other conversations (`activity_cards.rs`) in two separate stacks at the top right that could overlap, "Aperte ⌘Q de novo" at the top centre, and the Sirus app update as a small link in the footer. The cards were 330 px wide and cut their words to one line with an ellipsis, so a long error, a command or a file path could not be read.

## Decision

- **One column.** Every notification of the window sits in one column at the top right, under the title bar (`top = TITLE_BAR + 6`, `right 12`, 5 px between cards), built by `notice_column` in `toast.rs`. From the top: the ⌘Q notice, the failure or Git result (a conflict included), the Sirus app update, the provider CLI updates, then the cards for other conversations (newest first, three and a "+N avisos" line). The column reports its bounds to `browser_holes.rs` so it shows over the browser's page.
- **One card shape** (`toast::Card`, `toast::compose`): 320 px wide, growing downward; corner radius `RADIUS_SM` (7 px); padding 7/8/8/9; the floating surface, a 1 px border and a shadow. On the left a 12 px dithered pixel square in the card's tone (Bayer thresholds, 2 px cells, 0.4 px gap, solid at the top left), in place of the tinted icon square and the project avatar. A 12 px semibold title, an 11 px dim detail, and optional monospaced lines (a conflict's files, a permission's command).
- **Wrap, never truncate.** Titles, details and monospaced lines wrap; no ellipsis, no line limit. Long unbroken paths wrap at any character (GPUI's line wrapper does this when the width is fixed).
- **Buttons.** Compact (11 px, 3/6 px padding, radius 5). The primary one is filled with the accent colour (`theme::accent()`, text `on_accent()`) and sits rightmost; secondary ones are a 1 px outline just left of it; a tertiary one (a conflict's "Desfazer") goes to the far left of the button row. Permission cards use the accent too, no longer amber.
- **Beside or below.** GPUI cannot measure and lay out again, so `buttons_beside` decides from character counts: the words get 281 px (the card less border, padding and icon); the buttons take about 6.2 px per label character plus 14 px each and 4 px between; the buttons sit beside the words, vertically centred, only when the title (6.6 px per character) and at most one line under it (6.2 px per character) each fit on one line in what the buttons leave. Otherwise the buttons take a row of their own under the words, right-aligned, and the words take the full width. A tertiary action always gets the row. Unit-tested.
- **Clock.** A card that leaves by itself draws its time left as a 2 px pixel bar along its foot in its tone (`pixel_bar::pixel_fill`, the fill cells of `pixel_bar` without the track), shrinking to the left. Hover pauses it as before. Something running (a provider update, the app download) shows a short pixel stripe travelling along the foot; with reduced motion it stays still. The ✕ shows on hover. Cards keep their slide-in, and reduced motion shows them at once.
- **What moved.** The failure card left the top centre. "Aperte ⌘Q de novo para fechar o Sirus" is a card whose clock runs out with the two-second window; "Aperte esc de novo" stays above the composer. The app update is a card: "Sirus X disponível" with "Agora não" and "Instalar", "Baixando o Sirus X…" with the running stripe, "Atualização pronta" with "Depois" and "Reiniciar"; "Agora não", "Depois" and ✕ put that phase of that version away for the session. The footer notice is gone.
- **Unchanged.** The secret-saved card in the transcript, the script run notice above the composer, the failure ledge above the composer (`account_card.rs`), and macOS notifications (`native_notify.rs`).

## Consequences

- Nothing overlaps at the top right any more, and every notification reads in full.
- The heuristic can misjudge a title made of very wide or very narrow letters; at worst short words get their buttons on a row below, or a title beside the buttons wraps to a second line.
- An activity card no longer shows the project's avatar; the project's name leads the line under the title.
