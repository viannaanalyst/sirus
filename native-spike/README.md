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

Quitting with `pkill` leaves the host running. Kill it too after rebuilding it, or the app reuses the old one.

## Layout

- `src/shell.rs`: the window: title bar, tabs, sidebar, transcript, composer, popups and keys.
- `src/backend.rs`: starts and pairs the host, and the WebSocket client.
- `src/data.rs`: sessions and messages as conversations and turns.
- `src/settings_pages.rs`: the Settings pages.
- `src/panel_*.rs`: the side panel: Terminal, Files, Changes, Browser and side chat.
- `src/i18n.rs`: English and Portuguese, with the Portuguese text as the key.
- `src/visuals/`: native visuals, run from a JavaScript program.

## Tests

```sh
cargo test --features gpui/test-support
```
