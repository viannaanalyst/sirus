# ADR-034: Native session notifications and closed sound controls

**Status:** Accepted

## Context

Background sessions can finish or wait for an approval/question while another
session is visible. A Notifications page can offer Activity toasts, Desktop
notifications and Test. Sirus additionally needs independently selectable
event sounds without letting its untrusted webview manufacture activity alerts.

## Decision

`notifications.rs` owns closed persisted `AppSettings.notifications`: app
toasts, system banners, sounds, foreground behavior, three event toggles and
three allowlisted sound IDs. Missing fields receive defaults. Restoration is
page-wide and preserves unrelated settings. Foreground system alerts/sounds
are off by default; app toasts are for sessions outside the visible conversation.

Native agent request admission and successful process settlement alone call
the notification publisher. Codex, Claude and OpenCode share the typed native
request path; other providers can generate completion alerts but only verified
interactive callbacks can generate permission/question alerts. Transcript prose
is never parsed for imagined requests. No bootstrap/history, failed or cancelled
turns produce alerts. Pending identity includes generation, turn and request;
completion identity includes the last admitted user message. A 256-entry native
cache suppresses duplicates. Native ownership/status/preferences/focus are
checked again after OS authorization lookup, while holding retained data during
submission. No permission prompt occurs automatically.

macOS uses [UserNotifications](https://developer.apple.com/documentation/usernotifications)
directly, with real authorization/alert state and a retained delegate. Clicking
a default banner action opens only its still-owned Session and never answers
an agent request. The runtime tracks at most 64 notification identifiers and
removes older delivered banners. Deleted sessions cannot be opened. System
Focus/notification settings still determine whether a scheduled banner appears.

Six fixed [NSSound](https://developer.apple.com/documentation/appkit/nssound)
names provide Glass, Ping, Pop, Submarine, Tink and Hero. Playback occurs on the
main thread and stops the prior cue. System banners carry no separate default
sound, avoiding double audio. Sound and system-alert preferences are independent;
AppKit playback uses system output volume. No sound paths or uploaded audio.

## IPC security review

One new allowlisted `notification_action` command accepts a closed tagged
action: Status, Request, Test, Settings or Preview with a closed Sound enum.
Unknown fields are rejected. It accepts no session ID, lifecycle, text, URL,
file path, destination, tool response or notification payload. Test copy comes
from native localized fixed text and retained settings, with a three-second
gate; previews have a 300ms gate. Only one explicit permission request can be
pending. Settings opens one fixed Sirus macOS notification-settings URL.
Status/request callbacks have bounded waits. UI uses Client/Transport, with
no frontend notification/shell/fs plugin or capability addition.

Native `notification-activity` events contain bounded display labels and an
arrival timestamp, never transcript/tool contents. `notification-open` contains
an owned Session ID. The existing Zustand store holds at most eight app notices;
timestamps anchor expiry across renders/pages, and disabling preferences prunes
queued notices. Existing Arc toasts, Selects and compact Switches render them.

## Consequences

- Desktop system delivery and sound previews are macOS-only in this implementation;
  other hosts report Unsupported and retain app toasts.
- The OS authorization prompt requires an explicit Allow action. Denial directs
  the user to System Settings; Test reports submission rather than guaranteed
  visibility or ignoring Focus mode.
- Native authorization/banner appearance and audible playback require a later
  interactive check in the signed app. Unit/SSR/build checks do not prove them.

## Alternatives considered

- Renderer notification text/session/sound-path commands were rejected because
  they let a compromised webview impersonate lifecycle events.
- Snapshot comparison in React was rejected because reloads/replays and browser
  throttling must not determine native background delivery.
- Per-row reset controls were rejected in favor of the owner's
  existing page-wide reset design.
