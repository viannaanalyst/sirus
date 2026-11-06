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
