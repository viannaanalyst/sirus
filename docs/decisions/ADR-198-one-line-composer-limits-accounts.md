# ADR-198: A one-line composer, limits that offer another account, and removing accounts

- Status: accepted
- Date: 2026-11-06

## Context

A conversation stays on the account that started it, so one on the default account stopped at that account's limit with nothing to do but wait, and an account could be added but not removed. The failure card sat over the composer as a second box, and the notification settings offered six system sounds that no longer played.

## Decision

- **Composer.** One box with no divider, as in Monocode: attach, access and the context meter on the left, then the model, microphone and send on the right, on a single line.
- **Failure ledge (`account_card.rs`).** One line on the composer's top edge, like the queue. A usage limit says when it returns, read from the provider's words, and offers each other account next to "Try again".
- **Removing an account.** `remove_provider_account` deletes an app-managed account and its folder. It refuses the default account, which is the Mac's own CLI login, and any account a conversation still uses.
- **Sounds.** The host names the cue for each event (`cue`): permissions and questions take "input", pull requests "linked", the rest "finished". Settings keep each event's switch and "Listen"; the sound picker is gone. The stored sound fields remain unused by the window.
- **Caret.** The composer's caret blinks as the Mac's does, 0.58 s on in a 1.06 s cycle, restarting lit when it moves. The idle window redraws only at each flip while the composer has the keys and the window is active. Monocode's caret does not blink; this differs by the owner's choice.
- **Instructions.** A steer glides to the top of the view like a sent message: its stretch fills the view while the turn is anchored.
- **Color.** Dark glass now matches Monocode's tokens: the shell at 81% and the conversation under a near-black coat at 40%, where it had none. The sidebar opened over the workspace dims it by 40% and blurs only its own panel.
- **Project rail.** The footer, under the sidebar, shows five projects' robots with arrows that slide by one, and a swipe or a sideways wheel moves them too, settling on a whole tile, as Monocode's; the tooltip gives the name and the keys. Usage and Settings in the footer are icons with a tooltip. ⌘⇧← and ⌘⇧→ step between projects, unless the composer holds text, where they keep selecting to the line's ends. ⌥Tab goes back to the last project, and held it shows them by last use, which the pointer can also pick. ⌃Tab was already the conversation tabs' key.
- **Files.** The path bar over an opened file is gone; only Markdown keeps a bar, for Preview and Source.
- **Terminal tabs.** A terminal tab dragged from the panel onto the title bar becomes a title tab in any placement, with its shell still running; the panel opens a fresh shell if it would be left empty. A right click pins a title terminal as a small tile at the bar's far left, or closes it.
- **Terminal moves.** A terminal tab can be dragged onto the side panel or the bottom strip, where its shells rejoin the Terminal pane, or onto the conversation area, which splits with a pane for it, as Monocode's title terminals do. Its middle splits too: a terminal never replaces a pane and never becomes the active pane, because the composer and the shortcuts follow that one. It keeps its tab while it sits in a pane, and closing the pane only returns it there. A pinned terminal is a card in the deck of pinned tabs, no longer a separate tile.
- **Selection menu.** "Ask in the side chat" gives way to "Add to chat", which appends the selection to the composer's draft on its own paragraph, as Monocode does.
- **Second opinion.** The turn menu's action and its code are gone, which ends ADR-056 here. The host keeps `handoff_session`, which the handoff uses.
- **Composer height.** The box slides to the height of its text in 0.16 s, per conversation, with reduced motion cutting it. The caret's scroll aims at the height it is heading to, so the text does not jump while the box grows.
- **Tooltips.** They open 150 ms after the pointer arrives, centered above their element, or below it at the window's top, kept inside the window. The vendored gpui takes the element's bounds for that, in place of the mouse's position.
- **Title tabs.** A tab with an unsent draft is no longer dashed; this one departs from Monocode by choice. The close button shows only under the pointer, and the plus button has Monocode's plate, which brightens on hover.
- **Listening.** While the microphone records, the project's robot stands over the composer and hops with the voice, with fading bars on both sides. The window asks the host for the loudness (`dictation_level`, Mac only, as the other dictation commands) every 50 ms during a recording; a poll keeps these numbers away from the event stream that phones also receive.
- **Turn diffs.** A file row of a closed card now fetches the diff texts the transcript left out; only opening the whole card used to. Without them the row wrongly said the diff exceeded the storage limit. That message now shows only for a diff that was counted but too long to keep.
- **Turn actions.** The `⋯` menu is gone: side chat, fork and handoff are icons beside copy.
- **Edge fades.** The transcript's fades are now an alpha mask around the list. The gradients drawn in the conversation's color showed as dark bands once the conversation became translucent black.
