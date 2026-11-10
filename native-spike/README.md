# Sirus

Sirus: the window, written in GPUI (Rust and Metal), formerly "Sirus Native". Its app id is `com.siruscode.native` (unchanged by the rename, so its data folder and macOS permissions stay). It drives the headless `sirus-host` over the phone protocol; the old Tauri and React app is gone (ADR-191). See [ADR-189](../docs/decisions/ADR-189-sirus-native.md).

## Build and run

```sh
# The host, from src-tauri
CARGO_TARGET_DIR=../native-spike/target/host cargo build --release --no-default-features --features host --bin sirus-host

# The window, from native-spike
cargo build --release
./bundle.sh
open Sirus.app
```

`/Applications/Sirus.app` replaces `/Applications/Sirus Native.app` (an in-app update does that swap itself). `bundle.sh` signs with this Mac's Apple Development identity, so the Keychain ties its items to the team and no new build asks for the password; "Sirus Code Local Signing" remains the fallback, and with it every build asks again, because the Keychain then ties an item to that build's exact hash.

`bundle.sh` also copies `assets/reply-render` to `Contents/Resources/reply-render` (about 6 MB): the committed Mermaid and KaTeX files that the hidden page of `src/reply_render.rs` loads to draw diagrams and formulas, served by the host at `/assets/` (`--assets-dir`). Nothing is built at bundle time; `assets/reply-render/build/build.sh` updates them, and a test checks that every file they reach is there. Other devices reach this Mac only while Settings → Conexões allows it (port 7711, HTTPS on 8443).

Quitting with `pkill` leaves the host running. Kill it too after rebuilding it, or the app reuses the old one.

## Layout

- `src/shell.rs`: the window: title bar, tabs, sidebar, transcript, composer, popups and keys.
- `src/backend.rs`: starts and pairs the host, and the WebSocket client.
- `src/data.rs`: sessions and messages as conversations and turns.
- `src/settings_pages.rs`: the Settings pages. `src/settings_pair.rs`: the pairing popup (QR code) of Settings → Conexões.
- `src/panel_*.rs`: the side panel: Terminal, Files, Changes, Browser and side chat.
- `src/terminal_engine.rs`: the terminal engines behind the Terminal panel (alacritty by default, Ghostty's `libghostty-vt` in `src/ghostty_vt.rs` with the `ghostty` feature; `scripts/build-libghostty-vt.sh` builds the library, ADR-199).
- `src/browser_bridge.rs`, `browser_tab.rs`, `browser_js/`: agent control of the browser pane and the sign-in and computer-use approval cards (see ADR-189).
- `src/remote_pty.rs`: on a remote computer, a terminal's shell runs on its host (`start_terminal` and friends over the connection) and its output feeds the same engine (ADR-204).
- `src/tunnel.rs`: on a remote computer, pages on its `localhost:PORT` open through a listener here that tunnels each connection over the host's `/api/tunnel` (ADR-201).
- `src/attachment_view.rs`, `attachment_doc.rs`, `attachment_pdf.rs`: the composer's attachment chips and the viewer (image gallery, PDF, Word, slides, sheets, web pages, text). `attachment_ooxml.rs` (zip and XML of Office files) and `attachment_html.rs` (DOCX, PPTX and web pages as safe HTML) are pure and tested; `attachment_web.rs` lays a WKWebView or a PDFKit `PDFView` over the viewer's body. `image_markup.rs` keeps the strokes drawn on a draft picture and burns them into a PNG. `disclosure.rs` is the reveal behind folds; `sound.rs` the notification sounds. The sidebar's project rail and the ⌥Tab switcher live in `shell.rs`.
- `src/theme.rs`: the look (Ink, accent, glass) and its file. `src/send_morph.rs`: the composer's send button.
- `src/pull_requests_page.rs`: the Pull requests page, a title-bar tab (footer button or palette); its list logic and parsing are pure and tested. Create PR lives in the Changes panel (`panel_changes.rs`).
- `src/usage_page.rs`: the Usage popup. `src/tasks_page.rs`: Tarefas, a tab with the calendar of scheduled prompts and the "Nova tarefa" modal (ADR-200). `src/sidebar_card.rs`: the hover card of a conversation. `src/split_panes.rs`: conversations side by side. `src/hover_track.rs`: the sliding hover.
- `src/message_actions.rs`, `src/handoff.rs`, `src/transcript_select.rs`, `src/transcript_find.rs`, `src/turn_review.rs`: what a reply offers (the changes card, ⌘F marks through `markdown::set_search`). `src/reply_rich.rs`, `src/reply_render.rs`, `src/reply_view.rs`: images, video, Mermaid, KaTeX and HTML in replies.
- `src/file_editor.rs`: the editor in the Files panel. `src/git_sync.rs`, `src/git_split.rs`, `src/git_button.rs`, `src/project_scripts.rs`, `src/cli_diagnostics.rs`, `src/delegated_tasks.rs`, `src/agent_resources.rs`, `src/conversation_export.rs`, `src/chat_background.rs`, `src/simulator.rs` (hidden for now).
- `src/first_run.rs`: the welcome of the very first launch (agents, project, look) over the letters robot of `new_tab_field.rs`; once seen it is remembered in `native-first-run.json`, and Settings → Geral plays it again. `src/global_search.rs`: ⌘⇧F and the palette's message results, from the host's `transcript_action` search (3 characters, debounced). ⌃⇧F toggles the frame-time overlay.
- `src/workspace_follow.rs`: follows a session's folder on disk for the Files and Changes panels. `src/prompt_link.rs`: `sirus://new` prompt links and "Copiar link do prompt".
- `src/glide.rs`: the transcript's follow of its end as a glide (the list stays paused and is moved each frame; a sent message slides up and stays at the top). `src/account_card.rs`: the card above the composer when a turn ended on an error, with Try again and Continue on another account.
- `src/activity_cards.rs`: cards and project badges for what other conversations need (ADR-192).
- `src/app_update.rs`: in-app updates. It reads `sirus-latest.json` from the fixed `sirus-channel` release, verifies the archive's minisign signature against the key in `updater.pub` (never changed: installed apps trust it), swaps the bundle and relaunches only when asked, after the running-agents confirmation. Releases: `node scripts/release-sirus.ts` (dry run) or `--publish`.
- `src/i18n.rs`: English and Portuguese, with the Portuguese text as the key.
- `src/visuals/`: native visuals, run from a JavaScript program.

## Tests

```sh
cargo test --features gpui/test-support
```
