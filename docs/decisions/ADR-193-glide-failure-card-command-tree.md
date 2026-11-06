# ADR-193: The transcript glides, a failed turn says why, and commands read as their description

- Status: accepted
- Date: 2026-11-06

## Context

Three things in the window were worse than in the reference app's native app. A sent message and a growing reply jumped, because `FollowMode::Tail` snaps the list to its end in the very layout that grew it. A turn that ended on an error (for example "OAuth session expired" on the shared `default` login) showed an empty reply, "trabalhou por 13s", with the host's `lastError` stored but never shown. And a command in the opened tree read as its raw command line, while the reference app shows what the model said it was for, with the program and the files it names as chips.

## Decision

- **Glide (`glide.rs`).** The list stays paused (`pause_following_tail`) and the window moves it a little each frame toward its end: fast first, easing as it arrives, at most 1,600 px/s, straight to the end with reduced motion. Scrolling up leaves the end; reaching it again, sending, or the jump button follows again. A message sent from the window anchors its turn at the top of the view (the last turn is as tall as the view), so it slides up and stays there while the reply grows below it.
- **Failure card (`account_card.rs`).** When a session is `failed` with a `lastError`, a card above the composer says why. A sign-in failure gets a plain sentence, "Try again", and one "Continue on <account>" per other account of the provider, through `move_session_account`, which carries the native transcript (ADR-063 amendment); both then send "Continue where you left off.".
- **Command captions.** The host keeps a Bash call's `description` as `caption` on its activity row (bounded, masked by `redact`); the tree shows it as the row's title with the program (`git status`) and up to two file or folder chips (with their file icons) and a guide line, as in the reference app.

## Consequences

- The activity item gains an optional `caption`; the phone ignores it until it is shown there.
- An anchored last turn leaves empty room under a short reply until the next message.
- Sessions that stay on the `default` account still share its credential with every other Claude Code process; the card is the way out, not a fix for the race.
