# ADR-029: Embedded browser via native WKWebView, agent tools over local MCP

**Status:** Accepted

## Context

The product owner wants a full embedded browser in Sirus:
a per-session browser pane with tabs and an address bar, plus agent-facing
browser tools over MCP (navigate, snapshot, click, type, screenshot, …).
In an Electron app each tab would be a Chromium `WebContentsView` and agents
drive it over CDP through a private pipe. Sirus is Tauri 2 on macOS
(WKWebView/WebKit), which has no `WebContentsView` and no CDP.

Alternatives evaluated (October 2026):

- **Tauri child webviews (`unstable` feature)**: multiple webviews in one
  window still require `tauri = { features = ["unstable"] }` plus a dedicated
  capability, and the track record for exactly this use is poor: white pages
  on load ([#10011](https://github.com/tauri-apps/tauri/issues/10011)),
  broken positioning ([#10420](https://github.com/tauri-apps/tauri/issues/10420))
  and child webviews that always paint above the DOM
  ([#15682](https://github.com/tauri-apps/tauri/issues/15682)).
- **`webkit` crate (webkit-rs 0.4)**: complete safe bindings, but the
  `WebView` is offscreen-only and exposes no NSView handle, so it cannot be
  embedded into the existing window without contributing an accessor first.
- **External/system browser**: no in-app pane and no agent control.
- **Bundling Chromium/CDP**: contradicts the app's size and dependency rules.

## Decision

- The browser is a **native `WKWebView`** created with `objc2-web-kit` 0.3.2
  (same objc2 family already used for AppKit) and added as a subview of the
  Tauri window's `contentView` (`WindowExtMacOS::ns_window()`), the direct
  analog of Electron's `WebContentsView`. Rust owns tabs, navigation and
  lifecycle; the React pane measures its viewport and sends logical bounds
  over IPC; Rust converts to AppKit coordinates and calls `setFrame`.
  Every WebKit call is marshalled to the main thread with
  `AppHandle::run_on_main_thread`.
- **Occlusion** works by hiding/parking the native view (`setHidden`,
  bounds off-window, or removal from the hierarchy) whenever the pane is
  unmounted, collapsed, on a hidden session, or covered by app overlays.
  WebKit views paint above the DOM; the renderer never tries to z-index over
  them. Visible `data-appearance-floating` rectangles are checked for any
  intersection with the browser viewport, alongside existing DOM hit tests.
  Body portal insertion triggers a bounded positioning burst and portal size
  observation; narrow sidebar overlays must not fall between sample points.
- **Security boundary**: only `http(s)` and `about:blank` may load; `file://`
  is refused; navigation is decided in a `WKNavigationDelegate`; no Tauri IPC
  is reachable from page content (the page webview is not registered with
  Tauri); injected scripts live in a named `WKContentWorld`; permissions such
  as camera/microphone stay denied. **Downloads are out of scope**: no
  `WKDownload` handling, and navigation downloads are cancelled.
- **Agent tools** are exposed through a local **MCP server** implemented with
  the official `rmcp` crate (streamable HTTP on `127.0.0.1`, per-session
  bearer token, active-turn gated). It is injected per provider using each
  CLI's own MCP configuration (Claude `--mcp-config` + `--strict-mcp-config`;
  Codex `mcp_servers` with `bearer_token_env_var` in the bound profile; OpenCode
  remote `mcp` entry via its per-session config). Without CDP, tools are a mix
  of native WKWebView APIs (`load`, `goBack`, `takeSnapshot`) and bounded
  injected JavaScript (`snapshot`, `click`, `type`, `scroll`, `logs`) in our
  content world, with human-input interruption and output redaction.
- The work is phased: **Phase 1** user browser pane; **Phase 2** rich
  interaction (annotations, copy link, prompt screenshot, storage/cookies,
  in-page find, inspectable, popups); **Phase 3** MCP server, provider
  injection, agent tools, human takeover. Phases 2–3 get their own plans.

## Consequences

- **Positive:** one native browser stack already shipped by the OS; no new
  rendering dependency; complete control over bounds, occlusion, policy and
  focus; the MCP surface stays provider-agnostic and testable without CDP.
- **Negative:** macOS-only (already true for the product); WebKit is not
  Chromium, so a few sites render differently and Playwright/CDP-style
  automation does not exist; `browser_run`-style scripting must be
  reimplemented over injected JS later; `WKWebView` keyboard focus interacts
  with the app's `before-input-event`-less WebKit (chords like ⌘B/⌘\ are
  handled by `lib/shortcuts.ts` only when the app webview is focused).
- **Accepted trade-off:** no downloads and no OAuth popups in Phase 1; no
  password vault or cookie import from other browsers unless requested later.

## Alternatives considered

- Electron/Chromium: rejected — it would replace the whole
  desktop stack for one pane.
- Tauri `unstable` multiwebview: rejected on the open bugs above; revisit if
  Tauri stabilizes and fixes compositing.
- `webkit` crate direct: rejected for Phase 1 because it cannot attach to the
  window; it remains a useful API reference.
