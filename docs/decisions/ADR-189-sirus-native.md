# ADR-189: Sirus Native, a GPUI window over the headless host

- Status: accepted
- Date: 2026-11-05

## Context

The Tauri and React window has the features people use, but scrolling long conversations and animating tabs cost more than a native renderer would. A throwaway test of the Monocode tab strip in GPUI (Rust and Metal) felt faster, so the window is being rewritten in stages. The functionality stays the same as Sirus Code; design and layout are what change.

## Decision

- **Separate app.** `native-spike/` builds "Sirus Native" with its own app id (`com.siruscode.native`) and data folder, beside Sirus Code. The first run copies the data with `sqlite3 .backup`, with automatic work off.
- **Same back end.** The window starts `sirus-host` (ADR-131) on `127.0.0.1:7811` and speaks the phone's WebSocket protocol (ADR-080). It adds no commands of its own, apart from routing the visual tools (`visual_docs`, `visual_check`, `visual_show`) in `browser_mcp.rs` and storing a visual's native source.
- **Its own secrets.** With `SIRUS_KEYCHAIN_APP` set, the host keeps connector tokens and browser logins under that app's Keychain items. It copies a Sirus Code item only when macOS allows it without asking, and never writes back. Sharing items made each app rewrite the other's secrets, and macOS then asked for the password.
- **Vendored GPUI.** `vendor/gpui-pre*` comes from Monocode's upstream, for its frosted backdrop and rounded clips. Licences are kept in `LICENSE-MONOCODE-MIT` and `ICONS-NOTICE.md`.
- **Virtual transcript.** A conversation is a `gpui::list` that builds only the turns on screen and follows the tail until the reader scrolls up, when a button brings the latest message back.
- **One project at a time.** The sidebar shows one project, chosen from a popup under its name, with conversations grouped by date and a filter for status, period and provider.
- **Deletions always ask.** Every destructive action goes through one confirmation popup (`native_confirm`), including the Settings pages.

## Consequences

- Two windows exist until the rewrite ends. Anything new in Sirus Code has to be ported.
- Local work that the host doesn't offer (a commit's files, whole-project file search) runs `git` from the window, so it only works on this machine.
- Syntax highlighting (tree-sitter) adds about 8 MB to the app.
- Not done yet: the Simulator pane, Create PR and Watch PR in the Environment panel, pairing a phone with the native app (its host listens on loopback only), and per-project colour themes.
- The app is signed with a stable local identity, so macOS remembers "Always allow". The code is not notarized or released.
