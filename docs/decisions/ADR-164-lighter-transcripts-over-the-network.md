# ADR-164: Lighter transcripts over the network

**Status:** Accepted (2026-10-25).

## Context

Another open-source client cut its load times on slow links (cold connect 16.0 s → 3.6 s on 3G). Measured against
the real `state.db`, most of another client's changes would not move Sirus: `load_state` is about
27 KB for 14 sessions, and the phone app already pages transcripts (`limit`, `lite`). The weight is
elsewhere:

- The web UI on the phone and a desktop window showing another computer (ADR-132/133) load a
  transcript whole: 4.15 MB for the largest one.
- Turn review diffs are 19 % of a transcript and most of a large end-of-turn `session-updated`
  event (90–265 KB), though only the review pane reads them.
- `/api/socket` frames are never compressed (`axum::ws` does not negotiate permessage-deflate);
  only the HTTP bundle is.

## Decision

- **Diffs on request.** Transcript loads, search candidates and session events carry each turn
  review file's path, kind and counts but not its diff text; a file whose text was left out has
  `diffOmitted: true`. `transcript_action { turnReview }` returns one reply's whole review, which
  the review pane fetches once per turn and merges into the live review (undo and commit marks
  still come from it). Undo treats an omitted diff as present, since the native side has it.
  Stored reviews never carry the flag.
- **Gzipped frames.** A client whose runtime has `DecompressionStream` says `gzip: true` in its
  `hello`; frames of 32 KB or more then go as gzipped binary frames (`flate2`, fast level).
  Older clients and the native phone app omit it and keep text frames; older servers ignore the
  field. The client inflates in order: while a frame inflates, later frames wait behind it.
- **First page first.** Over the network (`client.overNetwork`), opening a transcript asks for its
  newest 60 messages, shows them when the transcript is at least twice that, and then loads the
  rest. `ensureTranscript` settles only when the whole transcript is in, so search, jumps and
  forks still see all of it. The page is shown only when it is well short of the whole, so a live
  event's window always starts past what it holds and reloads instead of splicing.

Not done, measured as not worth it: skipping settled sessions, side chats or idle subagents at
connect (17 KB), an ID table (under 1 %), deduplicating message bodies (none repeated), and a
reconnect delta (27 KB, about 7 KB gzipped).

## Consequences

- The largest transcript opened from the phone's web app: first paint from the newest 60
  messages, and about a quarter of the bytes for the whole once gzipped and without diffs.
- Opening a turn review costs one request; its diffs are not in memory until then.
