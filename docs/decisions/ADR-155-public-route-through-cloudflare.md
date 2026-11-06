# ADR-155: A public route through Cloudflare, paired by one scan

**Status:** Accepted (2026-10-20).

## Context

The phone reached the Mac and the VPS host (ADR-131) only over Tailscale, so the Tailscale app had to be on. Another open-source client solves "reach my machines from anywhere" with its own relay: an account (Clerk), a central broker that hands out tunnel addresses and one-time credentials. The owner wanted the same experience without running a central service or paying for one.

## Decision

- **Cloudflare Tunnel plus Access, per computer.** Each computer gets a hostname on the owner's domain (`mac.siruscode.com`, `vps.siruscode.com`) served by `cloudflared` pointing at `127.0.0.1:7710`. Cloudflare Access sits in front: people sign in with an email code (One-time PIN) restricted to the owner, and the iPhone app presents an Access service token (`CF-Access-Client-Id` / `CF-Access-Client-Secret`). Sirus pairing and device tokens stay mandatory behind it, so Access is an outer layer, not the only one.
- **`remote-public.json`.** Next to `remote.json`, the owner's file names the public `url` (an `https://` origin) and, optionally, the service token. `remote.rs` reads it when a pairing code is made; nothing else changes in the server (requests arrive from loopback with a matching `Host`, so the peer and origin checks already pass).
- **One scan.** With a public route, `remote_pair` adds `appUrl`: `sirus://pair?address=…&code=…&cfid=…&cfsecret=…`, and the QR code shows it instead of the tailnet link. The host's `sirus-host pair` prints that QR first. Scanning with the iPhone camera opens the app, which pairs over the public address and keeps the token in the Keychain with the computer.
- **The app sends the token everywhere.** `pairComputer` sends it with the pairing request, and the socket passes it as WebSocket headers (React Native's third constructor argument). Re-pairing a computer at its public address keeps its previous tailnet address and token in `previous`, because the app's updates are fetched from the tailnet address fixed in the build.

## Consequences

- The phone reaches both computers from anywhere without Tailscale; the VPS keeps agents running while the Mac is closed.
- The QR code carries the service token, so it is shown only on the owner's own screen, like the pairing code. Revoking it is one action in Cloudflare (delete or rotate the service token); device tokens still revoke per device.
- Not done: one phone web app (PWA) listing both computers across origins (ADR-132 only accepts `*.ts.net`, and Access cookies are per hostname), app updates over the public route (needs a native rebuild for `setUpdateURLAndRequestHeadersOverride`), and a Settings field to write `remote-public.json` from the UI.
