# Sirus Native

The Sirus Code window rewritten in GPUI (Rust and Metal). It runs beside Sirus Code with its own app id and data, and drives the headless `sirus-host` over the phone protocol. See [ADR-189](../docs/decisions/ADR-189-sirus-native.md).

## Build and run

```sh
# The host, from src-tauri
CARGO_TARGET_DIR=../native-spike/target/host cargo build --release --no-default-features --features host --bin sirus-host

# The window, from native-spike
cargo build --release
./bundle.sh
open "Sirus Native.app"
```

`bundle.sh` signs with the local identity "Sirus Code Local Signing", so macOS keeps "Always allow" between builds.

`bundle.sh` also builds the phone's web UI (`npm run build` at the repository root, once; `BUILD_UI=1` to redo it) and puts it in `Contents/Resources/ui`, about 97 MB. Other devices reach this Mac only while Settings → Conexões allows it (port 7711, HTTPS on 8443; Sirus Code keeps 7710 and 443).

Quitting with `pkill` leaves the host running. Kill it too after rebuilding it, or the app reuses the old one.

## Layout

- `src/shell.rs`: the window: title bar, tabs, sidebar, transcript, composer, popups and keys.
- `src/backend.rs`: starts and pairs the host, and the WebSocket client.
- `src/data.rs`: sessions and messages as conversations and turns.
- `src/settings_pages.rs`: the Settings pages. `src/settings_pair.rs`: the pairing popup (QR code) of Settings → Conexões.
- `src/panel_*.rs`: the side panel: Terminal, Files, Changes, Browser and side chat.
- `src/browser_bridge.rs`, `browser_tab.rs`, `browser_js/`: agent control of the browser pane and the sign-in and computer-use approval cards (see ADR-189).
- `src/attachment_view.rs`, `attachment_doc.rs`, `attachment_pdf.rs`: the composer's attachment chips and the viewer (image gallery, PDF, Word, slides, sheets, web pages, text). `attachment_ooxml.rs` (zip and XML of Office files) and `attachment_html.rs` (DOCX, PPTX and web pages as safe HTML) are pure and tested; `attachment_web.rs` lays a WKWebView or a PDFKit `PDFView` over the viewer's body.
- `src/theme.rs`: the look (Ink, accent, glass) and its file. `src/send_morph.rs`: the composer's send button.
- `src/pull_requests_page.rs`: the Pull requests page, a title-bar tab (footer button or palette); its list logic and parsing are pure and tested. Create PR lives in the Changes panel (`panel_changes.rs`).
- `src/usage_page.rs`: the Usage popup. `src/sidebar_card.rs`: the hover card of a conversation. `src/split_panes.rs`: conversations side by side. `src/hover_track.rs`: the sliding hover.
- `src/message_actions.rs`, `src/handoff.rs`, `src/transcript_select.rs`, `src/transcript_find.rs`, `src/turn_review.rs`: what a reply offers. `src/reply_rich.rs`, `src/reply_render.rs`, `src/reply_view.rs`: images, video, Mermaid, KaTeX and HTML in replies.
- `src/file_editor.rs`: the editor in the Files panel. `src/git_sync.rs`, `src/project_scripts.rs`, `src/cli_diagnostics.rs`, `src/delegated_tasks.rs`, `src/agent_resources.rs`, `src/conversation_export.rs`, `src/chat_background.rs`, `src/simulator.rs` (hidden for now).
- `src/first_run.rs`: the welcome of the very first launch (agents, project, look) over the letters robot of `new_tab_field.rs`; once seen it is remembered in `native-first-run.json`, and Settings → Geral plays it again. `src/global_search.rs`: ⌘⇧F and the palette's message results, from the host's `transcript_action` search (3 characters, debounced). ⌃⇧F toggles the frame-time overlay.
- `src/workspace_follow.rs`: follows a session's folder on disk for the Files and Changes panels. `src/prompt_link.rs`: `sirus://new` prompt links and "Copiar link do prompt".
- `src/i18n.rs`: English and Portuguese, with the Portuguese text as the key.
- `src/visuals/`: native visuals, run from a JavaScript program.

## Tests

```sh
cargo test --features gpui/test-support
```
