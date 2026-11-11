# ADR-087: Browser keys and waiting for agents

**Status:** Accepted

## Context

Sirus's browser tools (ADR-030) could not press keys or wait for the page. An agent could type into a field but could not press Escape, move with Tab or wait for a page to finish rendering. Separately, a tab an agent opened while the dock pane had never been shown was 1×1, so its screenshots were useless.

## Decision

1. **`browser_press`** presses one key from a fixed list on the focused element: Enter, Escape, Tab, Backspace, Delete, the arrows, PageUp, PageDown, Home, End and space (`BROWSER_KEYS`). Enter submits the field's form, Backspace edits fields, Tab moves focus, and the scrolling keys scroll the page. Like the other mutating tools, it stops if the person used the page in the last 1.5 s.
2. **`browser_wait_for`** waits until a text appears on the page or a selector matches. It polls every 150 ms for at most 8 s, below the 10 s limit of the page call.
3. **Tab size.** Tabs are created 1280×800 instead of 1×1, so a tab opened by an agent lays out and captures like a window even before the pane is shown. The pane's measured bounds take over once it is visible.

There is still no tool that runs arbitrary JavaScript. The phone app has no browser screen; the browser stays on the Mac.

## Consequences

- Agents can drive keyboard-only flows (menus, dialogs, search boxes) and wait for slow pages instead of guessing with retries.
- Keys outside the list are refused with the list of accepted keys.

## Addendum: one keyboard owner

When the owner clicks into the page the web view becomes the first responder while GPUI keeps its focus, so the composer kept blinking and received the letters the page ignored, while space scrolled the page. Each frame now judges it (`panel_browser::keys_decision`): keys moving from GPUI's view to a page with no agent acting means the owner clicked, and GPUI blurs; GPUI focus with the page holding the keys and no such move hands the keys back. An agent action never moves either (`agent_acting`; the bridge returns the keys when they were home).
