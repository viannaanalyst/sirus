# ADR-066: iOS Simulator pane

> **2026-10-13:** Superseded by [ADR-128](ADR-128-device-hub-simulator-pane.md). The Simulator pane is back, built on Expo's `expo-device-hub` instead of the native helper described here.

> **2026-10-08:** Superseded/removed at the owner's request. The Simulator dock pane, the `simulator_action` command and `simulator_*` agent tools, the native helper (`src-tauri/native/simulator-helper`), the H.264 frame path and the `autoOpenSimulator` setting are gone; a saved `autoOpenSimulator` key is ignored on load. Files the helper left in the app data folder (`simulator/`) are not deleted. The text below is kept as the historical record.

**Status:** Superseded by ADR-128 (removed 2026-10-08)

## Context

The owner uses Xcode and asked for a live, touchable iOS Simulator in the right dock, which agents can drive to test the apps they build.

## Decision

**Native helper.** `src-tauri/native/simulator-helper` holds the device helper (MIT, see `THIRD-PARTY-NOTICES.md`).

- The helper drives a booted simulator headless through CoreSimulator/SimulatorKit, which it `dlopen`s from the selected Xcode. It needs no Screen Recording or Accessibility permission.
- `simulator.rs` embeds the sources and compiles them on first use with the person's own `xcrun clang`/`swiftc`, into app data keyed by Xcode build and source hash.
- The helper speaks newline-delimited JSON-RPC on stdio: list, attach, stream, tap, swipe, key, text, button, screenshot and describe-ui.
- It writes H.264 (Annex B) frames to a private 0600 Unix socket that `simulator.rs` listens on. The renderer and `simulator.rs` handle the stream as described under **Video** below.

**Video.** These are the measured causes of a stalled or choppy picture, and their fixes:

- **Copying the framebuffer.** The simulator draws every frame into one shared IOSurface. Any read waits until that drawing ends: 20–40 ms by CPU lock, VTPixelTransfer or CoreImage alike. The helper therefore copies the surface with an asynchronous Metal blit into pooled buffers it owns (at most three copies in flight) and encodes each copy when it completes.
- **Encoder.** The VideoToolbox session uses a bitrate scaled to the screen area (4–24 Mbps), zero frame delay and speed over quality. Before this, the pane got about 15 fps from the ~54 frames per second the simulator draws. It now gets about 44, at about 50 KB per frame instead of up to 1 MB.
- **Settle pass.** About 60 ms after the screen stops changing, the helper encodes it once more. A change dropped while the pipeline was busy is never lost.
- **Decoder buffering.** VideoToolbox's SPS has no VUI `bitstream_restriction`, so WebKit's WebCodecs decoder held every picture until a flush. `simulator_h264.rs` rewrites each SPS to declare `max_num_reorder_frames = 0`, which is true because the stream has no B-frames. Each frame now paints within about 10 ms of arriving.
- **Transport.** Frames go to each mounted pane as raw bytes over its own Tauri `Channel` (an `ArrayBuffer` in the webview), with no JSON or base64. The stream runs only while a pane is subscribed (`watch` / `unwatch`, at most four subscriptions).
- **Keyframes.** A late subscriber, a sequence gap or a decoder error asks for a keyframe (`resync`). The helper re-encodes the current screen as a keyframe (`stream.keyframe`) instead of restarting the stream. Unchanged codec parameters do not recreate the decoder.

**Pane.** The dock gains a Simulator pane:

- a picker listing the available iOS simulators (name, runtime, booted state);
- the drawn device: an SVG squircle chassis with clickable side buttons;
- a control rail: Home, rotate view, save screenshot, record (saved through the native save dialog), shut down (confirmed) and disconnect.

Input:

- Pointer down/move/up become live touch phases (one move per animation frame), so drags, sliders and long presses behave as on a device.
- Coordinates come from pre-transform offsets, so they stay right in the rotated view.
- Touches and keys go through as HID usages or text, sent in order.
- Home is the HID Home button on every device. A synthetic edge swipe was dropped because apps took it as a scroll.

**Agent tools.** The existing per-session MCP bridge (`browser_mcp.rs`) also serves `simulator_list`, `simulator_boot`, `simulator_install`, `simulator_launch`, `simulator_screenshot`, `simulator_describe_ui`, `simulator_tap`, `simulator_swipe`, `simulator_type` and `simulator_button`.

- `simulator_install` accepts only an `.app` bundle inside the session workspace.
- Bundle IDs and UDIDs are validated.
- Coordinates must be normalized 0..1.
- When an agent boots a simulator, `simulator-open` opens the pane if Chat behavior's `autoOpenSimulator` is on (the default).

**Lifecycle.**

- There is one attached device app-wide.
- Sirus boots at most 3 simulators. One it booted shuts down 10 minutes after it is detached, when the person switches to another device, and on quit. Simulators the person booted are never shut down automatically.
- After a fresh boot, attach waits briefly: `bootstatus` returns before SpringBoard accepts touches. A device that has no framebuffer yet is retried for up to 45 s.
- Attach, detach and stream changes run one at a time. A helper that exits is restarted, the device re-attached and the stream resumed (at most three automatic recoveries a minute). A failed helper build is remembered until restart.
- A recording ends, unsaved, on detach, device switch and quit.
- Every command is a fixed argv: `/usr/bin/xcrun simctl …` or the helper.

## Consequences

- **Positive:** the person and the agent can see and drive an iOS app without leaving Sirus, and without granting extra macOS permissions.
- **Negative:**
  - The helper relies on private Xcode frameworks, so a new Xcode can break it. Rebuilding per Xcode build limits this, and failures are reported in the pane.
  - The first open spends a few seconds compiling.
  - The simulated device cannot really rotate; rotate only turns the view.
  - Text input covers printable ASCII.
  - Apps inside a simulator crash as macOS processes, so their crash dialogs (for example "MobileCal quit unexpectedly" on a first boot) come from macOS, not Sirus.

## Alternatives considered

- **Mirroring the Simulator.app window with ScreenCaptureKit.** Rejected: it needs Screen Recording, shows window chrome and cannot inject touches reliably.
- **A prebuilt helper binary.** Rejected: the private API moves with each Xcode release, so the helper must be built against the installed toolchain.
