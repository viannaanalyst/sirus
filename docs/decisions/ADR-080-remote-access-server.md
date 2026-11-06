# ADR-080: Remote access server

**Status:** Accepted

## Context

The owner wants to follow and drive sessions from a phone while the Mac does the work: a server on the person's machine, reached over Tailscale, with one-time pairing. A native app would need the paid Apple developer program, so the first client is the existing web UI, served by the Mac and opened in the phone's browser (later a PWA). This ADR covers stages 1 and 2 of that plan: the Mac becomes reachable, and Settings → Connections pairs devices with a QR code. The mobile screens and push notifications are later stages.

The UI already talks to the Mac only through `Transport` (`src/client/transport.ts`). Only `LocalTransport` touches Tauri, so a second transport can carry the same commands over the network.

## Decision

**Off by default.** `remote.rs` keeps its own `remote.json` in the app data folder (`0600`): `enabled`, `port` (default 7710) and the paired devices. It is not part of `AppSettings`, so `save_settings` (which a remote device can call) cannot change it. Two commands manage it and are refused to remote devices: `remote_action` (`status`, `setEnabled`, `revoke`) and `remote_pair`.

**Settings → Connections.** The Mac manages remote access on its own Settings page (the section is hidden in the UI served to other devices):

- a switch for remote access;
- the Tailscale address, with a copy button, or a hint to install Tailscale and sign in;
- three steps and a QR code made natively (`qrcode` crate, SVG shown as an image), with a countdown. The code closes by itself when a device pairs, and the page names the device;
- the device list (name, added, last used) with Remove.

`remote-changed` is emitted when a device pairs or connects. Development builds accept `SIRUS_REMOTE_LOOPBACK=1` to pair over `127.0.0.1` without Tailscale.

**Who can connect.** When on, an axum server listens on `0.0.0.0:<port>`. Every request passes a guard:

- The peer must be loopback or Tailscale (`100.64.0.0/10`, `fd7a:115c:a1e0::/48`); LAN and internet peers get 403. Tailscale already encrypts the traffic between devices.
- When the browser sends `Origin`, it must name the requested host. This blocks other websites and cross-site WebSocket hijacking.

**Pairing.** `remote_pair` creates a six-digit code (a 16-byte token before ADR-081) that is used once and expires after five minutes. It replaces any earlier code and is withdrawn after five wrong guesses. It returns `http://<tailscale address>:<port>/?pair=<code>` for each Tailscale address of the Mac and a QR code of the first one. Without a Tailscale address it refuses. The page exchanges the code at `POST /api/pair` for the device's own 32-byte token. The Mac keeps only the token's SHA-256, the device name and when it was created and last seen, for at most ten devices. Revoking a device closes its open sockets.

**Serving the UI.** Other paths are served from the bundled frontend through Tauri's asset resolver. Client-side routes fall back to `index.html`; missing files return 404. The HTML gets its own CSP (`connect-src 'self'`, no frames).

**Commands and events.** `GET /api/socket` upgrades to a WebSocket.

- The first message must be `hello` with a valid token within ten seconds.
- `invoke` is handed to the main webview's own IPC entry point (`Webview::on_message`) on the main thread, with the webview's URL and the app's invoke key. Command lookup, argument parsing and capability checks are exactly the ones the window uses, and no command list is duplicated.
- These are refused: `plugin:` commands and anything that acts on the Mac itself or manages remote access (`remote_*`, the file pickers, window capture and pasteboard, dictation, computer use, simulator streaming (the simulator was removed on 2026-10-08), window snapping).
- `listen` and `unlisten` subscribe to app events by name through `listen_any`, at most 64 per socket.
- Replies are JSON. Raw IPC replies are base64.
- A device whose queue reaches 4,096 messages is disconnected. It reconnects and reloads, so the queue never grows without bound.

**Client.** `RemoteTransport` is used when the page is not running inside Tauri (`isRemoteUi`).

- It opens the socket on first use, rejects pending calls when the connection drops, and reconnects with backoff (1 s doubling to 30 s) while anything listens. Subscriptions are renewed after a reconnect.
- `main.tsx` pairs from `?pair=` and stores the token in local storage.
- Without a token it shows how to pair. After a lost connection comes back, it reloads so the state is fresh. A revoked token is forgotten.
- Simulator frames (Tauri channels) are not available remotely yet. (The simulator was removed on 2026-10-08; since ADR-128 the Simulator pane's streams are served at `/api/device-hub/*` behind the device token.)

## Consequences

- From a paired device, the person can do anything the Mac window can, except the refused commands. That includes running agents with full access, so a device token is as sensitive as the Mac session itself. Tokens stay on the device, the Mac stores only hashes, and access ends when the device is revoked or remote access is turned off.
- `tauri::webview::InvokeRequest` and `Webview::on_message` are public but marked not stable. A Tauri upgrade must re-check them; the lockfile pins Tauri 2.12.
- Plain HTTP is used inside Tailscale. Service workers and web push (later stages) need HTTPS, which `tailscale serve` can provide in front of the same port.
- macOS asks once whether Sirus may accept incoming connections.
- No relay or cloud service: the Mac must be on and reachable over Tailscale. A headless host (VPS or always-on Mac) is a later, optional stage.

## Addendum (2026-10-16): the pairing QR code

The pairing link is drawn in the page (`PairingQrCode`, `uqr`, medium error correction): round modules, rounded finder squares and the Sirus mark in a cleared square at the centre, dark on a white tile whatever the theme, since not every phone camera reads an inverted code. Modules are dots of 0.96 module and the code is 192 px, sizes at which a software decoder read every case checked (four addresses, 1x and 2x screens). The native `qr_svg` stays for the host's command line.

## Addendum (2026-10-16): the "Connect a device" dialog and device names

- The pairing code moved from the Connections page into a dialog. An illustration band shows what happens on the other device; a switch separates the iPhone app (QR with the Sirus mark, the six digits beside it and three steps) from a browser (the address to copy and the code in large digits). A live code, the device that just joined and an expired code each have their own view; a new code takes the old one off screen before it is asked for, so a failure never shows a dead code.
- A device that joins while the dialog is open is named there, with suggestions. Paired devices can also be renamed in place in the device list (`remote_action` `rename`: one trimmed line, at most 60 characters).
