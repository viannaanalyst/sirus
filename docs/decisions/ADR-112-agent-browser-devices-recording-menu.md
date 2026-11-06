# ADR-112: Agents use device mode and recording; browser menu; recording overlays

**Status:** Accepted (2026-10-11)

## Context

ADR-111 gave the browser device mode and recording, but only through the pane's buttons. Agents should resize the preview and record it themselves, the preview needs a "⋮" menu, and recordings should show key presses and clicks.

## Decision

- **Device mode lives with the native browser.** `BrowserSessionState` carries `device` (CSS width, height, preset), so the pane and agents share one state through `browser-state` events. The pane writes it with `browser_set_device`. When the pane is closed, the hidden page keeps the device's layout: it is sized to the device times the current scale.
- **Agent tools** on the browser MCP bridge (ADR-030):
  - `browser_device` takes a preset (one of 17), or width and height (240–3840), an orientation, or `fill: true`. It changes CSS breakpoints, not the user agent.
  - `browser_record_start` and `browser_record_stop` ask the renderer, which owns the recorder, through a `browser-record-request` event. The renderer answers with `browser_record_reply` within 30 s. Stop returns the file's path and does not open Finder.
- **Recordings outlive the pane.** A per-session registry (`src/lib/browser-recordings.ts`) holds them, so the pane's button and an agent end the same recording. Snapshots of a hidden page ask WebKit for a fresh paint (`afterScreenUpdates`).
- **Overlays and frame rate** (Settings → Chat behavior → Browser recordings):
  - 30 or 60 fps (default 30); the canvas redraws the latest snapshot at that rate.
  - Key presses: a caption at the bottom; letters run together as typed, shortcuts read like "⌘K". A key typed in a password field shows only as "•".
  - Clicks: an expanding ring.
  - The page bridge script records the last 60 events, and the recorder reads them every 120 ms.
- **The "⋮" menu**:
  - reload without cache (`reloadFromOrigin`);
  - Web Inspector (WebKit's `_inspector`, with `developerExtrasEnabled` on new tabs, as Tauri does);
  - open in the default browser;
  - page colour scheme (system, light, dark) through the view's `NSAppearance`;
  - zoom from 50% to 200%, multiplied with the device scale;
  - clear cookies, or clear the disk, memory and fetch caches.

## Consequences

- The appearance and zoom settings are kept only while the app runs.
- Clearing cookies or the cache affects every session's browser. All of them share the app's default WebKit data store, so another Sirus instance with the same bundle id is affected too.
- Overlay positions follow the page's viewport. Clicks inside cross-origin frames are not seen.
