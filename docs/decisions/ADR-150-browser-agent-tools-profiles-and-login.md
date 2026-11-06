# ADR-150: More browser tools for agents, browser profiles, and no login shortcuts yet

**Status:** Accepted (2026-10-19). Browser profiles and the new agent tools are built; passkey login is not, and why is below. Cookie import from Chrome and Safari came later (ADR-152).

## Context

Agents drive the embedded WKWebView through `browser_*` tools (ADR-029, ADR-030, ADR-087). That covered navigation, snapshots and basic input, but not hover menus, `<select>` elements, drag and drop, file uploads, JavaScript dialogs, the page's network activity, viewport size, or a colour scheme. Every session also shared one cookie store, so signing in to one account signed in every conversation's browser, and there was no way to keep a work account apart from a personal one or to browse without leaving data behind.

## Decision

### Agent tools

All new tools sit behind the same switch as the existing ones, Settings → Agent browser access (`agentBrowserAccess`, ADR-130). Each argument is checked in `browser_agent.rs` before anything reaches a page; page scripts are fixed and receive their inputs as JSON, never spliced into source.

| Tool | Behaviour and limits |
| --- | --- |
| `browser_hover {ref?, selector?}` | Pointer and mouse events on the element, to reveal menus. |
| `browser_select {ref?, selector?, values[1–20]}` | A native `<select>`, or a listbox or combobox with `role=option` items, by text or value. A miss lists the options. |
| `browser_drag {from, to}` | Each end is a ref, a selector, or `{x, y}`. Synthetic pointer and mouse events, plus HTML5 drag events when the source is `draggable`. |
| `browser_upload {paths[1–5], ref?, selector?}` | Files only from the session's workspace or its attachment snapshots, resolved on disk (symlinks resolving outside are refused), at most 10 MB in total. Read with `O_NOFOLLOW`. A file input receives the files; other targets receive a drop. |
| `browser_dialog {action: accept\|dismiss, text?}` | Answers the page's alert, confirm or prompt. WebKit's UI delegate queues it on the session; `browser_status` shows it under `dialog`. The person can answer it in the pane. An unanswered dialog is dismissed when its browser is released, so the page never stays frozen. |
| `browser_evaluate {expression}` | Runs JavaScript in the page's main world, with the page's own permissions, and returns the JSON value bounded to 32 KB. Tried first as an expression, then as statements ending in `return`. It is gated only by Agent browser access, the level that already allows full control of pages; there is no separate switch. |
| `browser_network {limit?, clear?}` | The current page's fetch and XHR calls, plus resource timings. Method, URL with query values replaced by `…`, status, type, duration. No bodies or headers. Requests made before the page's document-start script ran can be missing, and the log resets on each navigation. |
| `browser_resize {width, height}` | Device mode at a custom size (240–3840 CSS px). `browser_device {fill: true}` returns to the pane. |
| `browser_color_scheme {scheme}` | `light`, `dark` or `system`, as the appearance menu sets it. |
| `browser_open {port}` | Opens `http://localhost:<port>` (1–65535). |

Page-facing scripts use `callAsyncJavaScript` with arguments, the same path as the existing tools.

### Browser profiles

- **Default** is WebKit's default store, the cookies every tab used before this ADR.
- **Private** uses one non-persistent store per session browser. It is dropped with the browser, and its tabs share it so a sign-in popup keeps its cookies.
- **Custom profiles** use `WKWebsiteDataStore.dataStoreForIdentifier` with a UUID, on macOS 14 and later. On earlier systems they are refused (explicit requests) or fall back to Default (a stale default). The renderer can see support through `browser_profiles_supported`.
- The list is `AppSettings.browserProfiles` (at most 10, names up to 40 characters, unique, not Default or Private, ids canonical lowercase UUIDs) and `defaultBrowserProfile`, both validated natively in `browser_profiles::validate`.
- A tab keeps the profile it opened with, and popups inherit their opener's profile. Agents' `browser_open` uses the Settings default.
- The **+** button of the tab strip opens a menu: Default, Private, then each custom profile.
- Deleting a profile closes its tabs in every session, then removes its WebKit data store with `removeDataStoreForIdentifier`, and only then does the renderer drop it from settings. If the removal fails, the profile stays listed.

### Login helpers

- **Passkeys and WebAuthn in WKWebView: not built.** Passkeys in a browser app need Apple's restricted `com.apple.developer.web-browser.public-key-credential` entitlement, which Apple grants on request, and which needs a provisioning profile that lists it. Sirus is signed with a local identity or a Developer ID and has neither. The Electron reference (another open-source client) relies on the same managed entitlement. We could not verify on this machine whether a non-browser WKWebView gets platform passkeys without it, so nothing is built and the question stays open until Apple approval or a test on a signed build.
- **Cookie import from Chrome and Safari: built later, with its own consent flow (ADR-152).** The original reasoning, kept for context: Safari's `Cookies.binarycookies` is behind Full Disk Access, a broad permission for a feature that only copies cookies. Chrome's cookies are AES-encrypted with a key that sits in the Keychain item "Chrome Safe Storage". Reading it means a Keychain prompt for another application's secret, decrypting cookies, and writing them into the target profile's `WKHTTPCookieStore`. That is feasible, but it adds crypto and a consent flow that deserve their own decision. Until then, a profile keeps whatever the person signs in to in it, which covers the common case with one sign-in per profile.

## Consequences

- The `browser_*` tool list grows from 19 to 28; the bridge still refuses unknown tools and the Settings switch still hides them all.
- `browser_evaluate` runs whatever an agent writes in a page the person may be signed in to. That is the same exposure the Settings switch already accepts for clicks and typing; turning the switch off removes it.
- Dialogs freeze a page until someone answers, so an agent that never answers would stall the page; the pane answers too, and a closed browser dismisses.
- Network capture is a document-level shim and a resource-timing observer, not a WebKit inspector hook, so it shows what the page's own scripts issued, not every request.
- Profiles add up to ten WebKit data stores on disk for custom profiles. Deleting one removes its data; there is no undo.
- Verified by unit tests on argument validation, the upload path rules, URL redaction, dialog queueing types and profile validation, and by the two clippy builds and `cargo test --lib browser`. Not yet exercised in the running app.
