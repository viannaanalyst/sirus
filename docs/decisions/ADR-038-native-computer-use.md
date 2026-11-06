# ADR-038: App-owned computer use with per-app approval

**Status:** Accepted

## Context

Agents that can see and operate Mac apps unlock tasks outside the repository. One approach embeds a patched upstream driver (Cua `cua-driver`, about 1 MB of patches) behind roughly 40 MCP tools. It is slow (Calculator turn 26.6 s versus 17.2 s for Codex's own plugin), has no persistent "agent is in control" indicator, and grants a whole chat access to the whole computer. Sirus already hosts the embedded browser as an app-owned MCP server (`--mcp-browser`, private socket, per-session token; ADR-030 browser bridge), which is the right shape for this too.

## Decision

`computer.rs` is a native driver in the app binary — no extra executable, no Swift build step. It uses the official `objc2` bindings:

- **Read:** Accessibility (`objc2-application-services`). One bounded breadth-first pass per observe — at most 6,000 visited nodes, 40 levels and 20–400 recorded elements. Interactive roles and labelled static text receive refs (`e1…`) with window-relative frames. Secure text fields never expose their value. All AX calls run on one dedicated worker thread (AX elements are not `Send`) with a 2 s messaging timeout.
- **Act:** semantic `AXPress`/`AXValue`/`AXFocused` first. Otherwise keyboard, mouse and scroll events go to the target process with `CGEventPostToPid`. The person's pointer and focus are not moved. Bringing an app forward is the only foreground action and requires 1.5 s without human HID input.
- **See:** ScreenCaptureKit `SCScreenshotManager` (the CoreGraphics window-capture API is obsolete since macOS 15). It captures the matching window as a JPEG with its longest side at most 1,280 px.
- **Launch:** `NSWorkspace` by bundle id, without activation.

`computer_mcp.rs` exposes nine tools: `computer_apps`, `_observe`, `_screenshot`, `_click`, `_type`, `_key`, `_scroll`, `_launch` and `_focus`. Actions return the settled observation by default (act-then-see), cutting round trips. The stdio child mode is shared with the browser through `mcp_stdio.rs`.

**Policy is native and closed:**

- **Off by default.** `AppSettings.computerUseEnabled` gates whether Codex, Claude and OpenCode turns receive the server. Turning it off revokes everything.
- **Per-app approval** per session, memory-only. The tool call waits up to 120 s for an in-session card ("Codex wants to use Calculator"), and the Dock bounces. Grants are never persisted.
- **Fixed blocklist:** password managers, Keychain, System Settings, SecurityAgent, terminals, Script Editor/Automator/Shortcuts and Sirus itself. Typing into a shell would bypass provider sandboxes and approvals.
- **Planning is read-only:** only observe and screenshot are allowed.
- **Stop:**
  - A physical Escape (global key monitor, ignoring the agent's own Escape) revokes every grant and declines pending approvals.
  - So does the native indicator's Stop button.
  - Both stop control only while an agent acted in the last 15 s.
- **Indicator:** a floating pill ("Codex is using Calculator · Esc to stop", native Liquid Glass when available) plus a border around the controlled window. It is a non-activating panel that hides after 6 s of inactivity.
- **IPC:**
  - One closed command, `computer_action`: status, fixed permission prompts, fixed Privacy panes, respond, revoke, stop.
  - One event, `computer-state`.
  - The renderer never supplies apps, paths, URLs or input.
- **History:** memory-only, 200 entries, without typed text.

Claude receives the server in the same `--mcp-config` file and `--allowedTools mcp__sirus_computer`. Codex receives `-c` overrides with `default_tools_approval_mode="approve"`. OpenCode receives an ACP MCP server. The per-app approval replaces the per-call vendor prompt.

## Consequences

- **Positive:** Fast semantic actions in the background, one visible kill switch, least privilege per app and session, and no third-party binary to sign or patch.
- **Negative:** Apps with poor Accessibility trees (some Electron/Chromium canvases) fall back to coordinate clicks, which background delivery may not reach. Ad hoc-signed local builds can lose the Accessibility/Screen Recording grants after each rebuild; a stable signing identity is needed for daily use. macOS only.
- **Not provided:** live streaming preview, per-app always-allow, menu-bar traversal and multi-display certification.
