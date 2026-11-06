# ADR-054: Global window snap

**Status:** Accepted

## Context

A global hotkey can capture another app's frontmost window into a task. Sirus already had Attach window (ADR-041), but only through the composer's "+" and the system picker. The owner asked for the shortcut.

## Decision

**Setting.**

- **Storage:** `AppSettings.windowSnapEnabled` (off by default) and `windowSnapShortcut`.
- **Shortcut choices:** a closed set: ⌃⌥⌘S (default), ⌥⇧S or ⌃⇧S.
- **Where:** Settings → Computer, with the existing Screen Recording status and request buttons.

**Shortcut.**

- **Registration:** the native side registers the shortcut through Carbon `RegisterEventHotKey`, using the `global-hotkey` crate (Tauri project, macOS-only dependency).
- **Permissions:** it needs no Input Monitoring or Accessibility. If another app already owns the combination, registration fails and the shortcut stays off.

**Capture.**

- **What is captured:** when pressed, native code reads the frontmost app (`NSWorkspace`). If that is Sirus, nothing is captured. Otherwise it takes that app's front normal window from the on-screen window list (front to back, layer 0) and captures it with the existing ScreenCaptureKit helper (`computer::screenshot`, longest side 1800 px). Screen Recording must already be granted.
- **Pending slot:** the JPEG is held in one native slot with a random nonce. Only the app name and nonce cross IPC, in the `window-snap` event; the event also reports `needsPermission`, `ownWindow` and `failed`.
- **Where it goes:** the renderer claims the capture for the composer that is open: the selected session, or the selected project's new-chat landing. With neither, it discards the capture.
- **Admission:** claiming goes through the existing bounded attachment admission and private cache (ADR-018), and a toast confirms it.

**Closed command.** `window_snap_action` accepts only `claim { nonce, owner }` and `discard { nonce }`.

## Security review

- **Inputs:** the renderer supplies only a nonce and a draft owner. It cannot choose a window, app, process, path or image bytes.
- **Claiming:** each capture can be claimed once. It expires after 60 seconds, and a new capture replaces an unclaimed one. Owner validation and size limits reuse attachment admission.
- **Sending:** nothing is ever sent automatically. The image stays an unsent draft attachment until the person sends.
- **Permissions:** the hotkey is opt-in and requires no new capability. It uses no event tap and no new OS permission beyond Screen Recording, which computer use already requests through fixed prompts and panes.
- **Scope of capture:** only the frontmost app's front window is captured at the moment the person presses the shortcut. This can include sensitive content the person chose to have in front; the capture still only reaches a draft.

## Consequences

- **Positive:** a browser, design or simulator window becomes context in one keystroke without switching apps.
- **Negative:** the shortcut works only while Sirus is running, and macOS only.
- **Accepted trade-off:** the capture joins the conversation that is open, not "the one used in the last minute".

## Alternatives considered

- **Both Option keys.** Rejected: it needs a CGEventTap and the Input Monitoring permission.
- **Letting the renderer pick the target window.** Rejected: window identities stay native, as in ADR-041.
