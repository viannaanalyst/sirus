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
- `src/i18n.rs`: English and Portuguese, with the Portuguese text as the key.
- `src/visuals/`: native visuals, run from a JavaScript program.

## Tests

```sh
cargo test --features gpui/test-support
```
