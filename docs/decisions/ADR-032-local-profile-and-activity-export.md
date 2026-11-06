# ADR-032: Local identity, retained activity and bounded image export

**Status:** Superseded — the Profile page, `profile_image_action` and transcript prompt activity were removed at the owner's request (2026-10-05). A retained `AppSettings.profile` is ignored.

## Context

The owner requested a Profile and activity-sharing experience within
Sirus's settings. Historical token and deletion-preserving
activity ledgers; Sirus retains transcripts and current session choices.
Quota snapshots cannot supply lifetime token totals. Browser clipboard/download
and arbitrary social URL bridges would cross the existing native trust boundary.

## Decision

`AppSettings.profile` stores local display name, handle, one of five avatar colors
and an optional square JPEG. It is separate from provider accounts and login.
Native save/load bounds names to 80 characters, ASCII handles to 32 and encoded
avatars to 200,000 characters. Images must fully decode within 160×160 and a
1 MiB allocation limit. Legacy or malformed preferences normalize safely.
`HostInfo.profileDefaultName` exposes only the home-directory basename.

`profile-stats.ts` derives activity from owned, retained sessions. Whole imported
sessions are excluded because they lack per-message app provenance; fork prefixes
are excluded using `inheritedMessageCount`. Archived sessions remain retained.
Deletion reduces counts. The rolling 274-day calendar uses local dates, prompt
counts and four ranked positive intensity levels. A single midnight timer and
focus/visibility events refresh calendar boundaries without polling.

Provider/model/reasoning shares describe current choices of sessions with
activity, not historical turn or token attribution. Lifetime tokens and peak
token day remain unavailable; plugin runs are explicitly untracked. A future
native admission ledger requires its own persistence/provenance design.

The shared Arc dialogs edit local preferences and preview an aggregate-only
white activity card. The exact same locally rendered PNG feeds the preview,
Copy, Save and social actions. It includes identity and counts, never transcript
text, project paths or credentials. Edit cannot be dismissed while processing
or saving, preventing a pending save from closing a later edit.

`profile_image_action` is one closed native command: Copy, Save, X, LinkedIn or
Reddit. It accepts no destination path or URL. One process-wide action is admitted
at a time. PNG bytes are bounded to 4 MiB, 2048×2048, unanimated 8-bit RGB/RGBA;
chunk lengths/CRCs and full bounded pixel decoding are checked before an action.
The `image` crate enables only PNG/JPEG, replacing incomplete header-only checks;
`crc32fast` verifies every PNG chunk, including ancillary chunks.

Copy writes PNG through AppKit on the main thread. Save uses the existing native
PNG picker; writes require an absolute regular PNG destination, reject final
symlinks, use `O_NOFOLLOW`, verify an existing file's inode/device before truncation
and create new files with mode 0600. The renderer cannot select a filesystem path.
No stronger ancestor-directory race isolation is claimed. Non-macOS Copy reports
unsupported; Save remains the portable action.

Social actions copy the image and open exactly three native constant HTTPS
composer URLs through the existing opener. Posting/pasting is the user's action.
This exception does not extend `open_external_url`'s host allowlist, add browser
fallbacks, credential reads, network publishing or Tauri permissions. Native
shutdown admission is checked before side effects.

## Consequences

- **Positive:** Familiar local Profile and sharing without invented telemetry,
  an app account, remote image rendering or a generic filesystem/URL bridge.
- **Negative:** Retained counts change after deletion; imported-session follow-ups
  are also excluded. Token totals and historical model attribution are unavailable.
- **Accepted trade-off:** One small codec dependency validates actual pixels on
  every host; image clipboard support remains macOS-specific.

## Alternatives considered

- Reusing vendor identity/quota: confuses local preferences with authentication
  and does not provide historical totals.
- A lifetime ledger in this change: requires native turn provenance and durable
  removal semantics beyond the requested settings surface.
- Browser downloads, clipboard plugins or arbitrary social URLs: unnecessary
  extra authority for a single bounded export operation.
- Header-only validation or AppKit-only decoding: admits corrupted saved files
  or leaves validation inconsistent across supported desktop hosts.
