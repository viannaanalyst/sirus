# ADR-082: Phone alerts and HTTPS

**Status:** Accepted Its HTTPS through `tailscale serve` was removed by ADR-198.

## Context

Stage 4 of the mobile plan: the phone should tell the person when an agent needs an approval or an answer, or has finished, even while the app is closed. On iPhone, web push works only for a home-screen web app (iOS 16.4+) served over HTTPS with a service worker. ADR-080 serves plain HTTP inside Tailscale. The person already uses Tailscale, which can put a real certificate for `<mac>.<tailnet>.ts.net` in front of a local port (`tailscale serve`).

## Decision

**HTTPS through the person's Tailscale.** Settings → Connections gains a "Secure address (HTTPS)" row. `remote_action { enableHttps }` runs `tailscale serve --bg --https=443 http://127.0.0.1:<port>`:

- It uses a fixed argv and the Tailscale app's own binary first. `/usr/local/bin/tailscale` is a shell wrapper, and stopping it would leave a waiting `serve` behind.
- It sets `TAILSCALE_BE_CLI=1`. Started from an app, without a terminal's environment, the macOS binary otherwise tries to open its GUI and fails with `CLIError error 3`.
- When the tailnet has not allowed HTTPS yet, the CLI prints a `login.tailscale.com` link and waits. The app stops it as soon as the link appears and offers "Open Tailscale" (`openHttpsSetup`). It opens only links of that exact form.
- The status reads the MagicDNS name and whether `serve` proxies the port. `disableHttps` turns `serve` off.
- When HTTPS is on, it comes first in the address list and the QR code. Requests from `serve` arrive from loopback with the `https://<mac>.ts.net` origin, which the guard now accepts.

**Push from the Mac, straight to the vendor.**

- `remote/push.rs` keeps a VAPID key pair in `remote.json` (`0600`).
- A paired device sends its subscription to `PUT /api/push` and can remove it with `DELETE`. The device's bearer token is required; `GET` returns the public key.
- Only `https` endpoints on Apple's, Google's, Mozilla's or Windows' push services are accepted, without a custom port. The subscription is bounded and must parse.
- The payload is title, body and session ID, bounded. It is encrypted for the device (RFC 8291) by `web-push-native` (pure Rust) and posted with `reqwest` (no redirects, 15 s timeout).
- A `404` or `410` drops the subscription.

**Same events as the Mac.** `notifications::publish` and `announce` also call `remote::notify` for permission requests, questions and finished tasks. They respect the person's per-kind switches but not the Mac's window focus: the phone is useful precisely when the person is away.

**Phone side.**

- `public/sw.js` shows the alert, with the session as its tag. A tap focuses the app and posts `open-session`, or launches `/?session=<id>`.
- `MobileApp` registers the worker on secure pages and opens that conversation. An alert from a side chat opens its parent.
- Settings → Alerts explains what is missing, in order: the secure address, the home-screen install, browser support, then permission. Otherwise it offers a switch.
- On the Mac, each device shows "alerts on" and has "Send a test alert" (`testPush`).

**Also in this change: the Astros tab** (ADR-081). It lists the person's Astros with their activity and unread count. A tap opens the Astro's conversation (`openAstro`). An Astro without a project says to set one on the Mac, because it cannot start without one. Creating and editing Astros stay on the Mac.

**Serving fixes.**

- Tauri does not type `.webmanifest`; it is served as `application/manifest+json`.
- A missing static file (scripts, styles, images, fonts) answers 404 when the resolver falls back to the app shell. Client-side routes still get the shell.

## Consequences

- Alerts need a one-time switch in the person's Tailscale account. Devices paired over plain HTTP pair again at the HTTPS address, because storage is per origin.
- The Mac contacts Apple's (or another vendor's) push service with each alert. The text is end-to-end encrypted for the device; the vendor sees only the timing and size.
- There is no offline cache: the service worker exists for alerts only.

## Alert text and grouping (2026-10-13)

In phone alerts the title is the conversation (or the project when the conversation is untitled or named like it). The body is the status, then the project, then the detail after a dash: "✓ Concluída · newlex — De nada! 👍". iOS joins the lines of a web alert, so the body no longer relies on a line break. The status words are "✓ Concluída", "⚠︎ Precisa de aprovação", "? Pergunta para você", "✕ Falhou" and "⏰ De volta", with matching English text.

The payload carries `group: { kind, label }`. When alerts of the same kind from other conversations are still on screen (within 30 minutes), the service worker folds them into one summary, such as "3 conversas concluídas", with the conversation titles listed under it. Tapping the summary opens Home. A conversation's own alerts still share its tag, so a newer one replaces the older one quietly. The phone cannot skip an alert for the conversation already open, because iOS requires every web push to show a notification.
