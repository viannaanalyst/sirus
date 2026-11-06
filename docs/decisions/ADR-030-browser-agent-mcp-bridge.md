# ADR-030: Browser tools for agents over a local MCP bridge

**Status:** Accepted

## Context

ADR-029 introduced the native user-facing browser and reserved Phase 3 for
agent control. An Electron app can expose `browser_*` tools over MCP
because Chromium/CDP already runs the pages. Sirus drives WKWebView on
the main thread and has no CDP; it also has no MCP gateway.

Alternatives evaluated:

- **`rmcp` + streamable HTTP inside the app**: official SDK, but brings an
  HTTP server stack and per-session auth plumbing for a feature that only
  needs to talk to child processes we already spawn.
- **HTTP on localhost with a token in the URL/header**: same weight, and the
  token would live in provider argv/config files anyway.
- **Directly exposing Tauri IPC to providers**: would put renderer-adjacent
  IPC in a CLI process and break the Client/Transport boundary.

## Decision

- The app runs a private **Unix socket bridge** under app data
  (`browser-mcp/bridge.sock`, directory `0700`, socket `0600`, stale socket
  removed on start). Each browser-enabled session gets a random token; the
  socket accepts newline-framed JSON `{id, token, tool, arguments}` and
  answers `{id, ok, result | error}`. The token is compared in constant time
  and the session must still exist in `AppState`.
- The same binary runs in **MCP stdio child mode** with `--mcp-browser`: it
  reads JSON-RPC from stdin, serves `initialize`, `notifications/initialized`,
  `ping`, `tools/list` and `tools/call`, and forwards each call over the
  bridge. No window is created; `main.rs` exits before Tauri starts.
- **Injection** happens where the adapters spawn CLIs: Claude gets
  `--mcp-config <file>` (0600 file in app data, command = this binary);
  Codex gets `-c mcp_servers.sirus_browser.*` overrides before
  `app-server` (command/args/env, token passed by env, never argv); OpenCode
  gets an `mcp` entry merged into the inline `OPENCODE_CONFIG_CONTENT` it
  already receives. Other adapters keep no browser tools.
- **Tools** (fixed, bounded): `browser_status`, `browser_tabs`,
  `browser_open`, `browser_navigate`, `browser_back`, `browser_forward`,
  `browser_reload`, `browser_screenshot` (PNG image content, 8 MiB cap; only
  the model gets it, nothing is added to the composer),
  `browser_snapshot` (≤200 interactive elements with `data-sirus-ref`
  refs), `browser_click`, `browser_type`, `browser_scroll`, `browser_logs`
  (≤200 entries), `browser_close`. Navigation stays on the http(s) allowlist;
  `file:` and downloads remain unavailable; arguments are length-checked and
  passed to the page through `callAsyncJavaScript` arguments, never
  interpolated into script source.
- A document-start user script records human input (`pointerdown`, `keydown`,
  `wheel`) and a bounded console buffer. Mutating tools return an error when
  the person used the page in the last 1.5 s ("interrupted by human input").

## Consequences

- **Positive:** agents drive the same visible browser the owner uses, with no
  new runtime dependency and no renderer IPC; tokens never appear in argv;
  every value crossing the page boundary is bounded or passed as data.
- **Negative:** stdio MCP children are spawned per provider turn (Codex
  spawns one app-server per turn), so each turn starts a small extra process;
  the socket bridge is ours to maintain; only Codex, Claude and OpenCode get
  tools; `browser_logs`/human detection are page-level (not native input).
- **Accepted trade-off:** no uploads, downloads, OAuth automation, cookie
  import or `browser_run`-style scripting; those can be later ADRs.

## Alternatives considered

- `rmcp`: revisit if the bridge grows beyond tools (resources, prompts,
  subscriptions) or if more providers need HTTP MCP.
- Exposing the browser over the app's own IPC: breaks the untrusted-webview
  boundary and the Client/Transport architecture.
