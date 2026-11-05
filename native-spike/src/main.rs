//! Part 1 of the native spike: tabs, pinning, sidebar and motion on GPUI with sample data,
//! to compare by feel against the Tauri app. GPUI is vendored from Monocode (Apache-2.0);
//! a few presentation files are ported from Monocode (MIT, LICENSE-MONOCODE-MIT).
mod data;
mod marks;
mod motion;
mod shell;
mod tab_label;
mod tab_tile;
mod theme;

use gpui::{App, AssetSource, Bounds, Result, SharedString, TitlebarOptions, WindowBounds, WindowOptions, point, prelude::*, px, size};
use std::borrow::Cow;

struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let bytes: &'static [u8] = match path {
            "plus" => include_bytes!("../assets/plus.svg"),
            "close" => include_bytes!("../assets/close.svg"),
            "sidebar" => include_bytes!("../assets/sidebar.svg"),
            "sidebar-right" => include_bytes!("../assets/sidebar-right.svg"),
            "chevron-right" => include_bytes!("../assets/chevron-right.svg"),
            "search" => include_bytes!("../assets/search.svg"),
            "pencil-line" => include_bytes!("../assets/pencil-line.svg"),
            "bot" => include_bytes!("../assets/bot.svg"),
            "refresh" => include_bytes!("../assets/refresh.svg"),
            "mic" => include_bytes!("../assets/mic.svg"),
            "arrow-up" => include_bytes!("../assets/arrow-up.svg"),
            "zap" => include_bytes!("../assets/zap.svg"),
            "window" => include_bytes!("../assets/window.svg"),
            "square-terminal" => include_bytes!("../assets/square-terminal.svg"),
            "files" => include_bytes!("../assets/files.svg"),
            "git-compare" => include_bytes!("../assets/git-compare.svg"),
            "globe" => include_bytes!("../assets/globe.svg"),
            "messages" => include_bytes!("../assets/messages.svg"),
            "smartphone" => include_bytes!("../assets/smartphone.svg"),
            "expand" => include_bytes!("../assets/expand.svg"),
            "settings" => include_bytes!("../assets/settings.svg"),
            "laptop" => include_bytes!("../assets/laptop.svg"),
            "branch" => include_bytes!("../assets/branch.svg"),
            "upload" => include_bytes!("../assets/upload.svg"),
            "server" => include_bytes!("../assets/server.svg"),
            "folder" => include_bytes!("../assets/folder.svg"),
            "arrow-left" => include_bytes!("../assets/arrow-left.svg"),
            "bell" => include_bytes!("../assets/bell.svg"),
            "box" => include_bytes!("../assets/box.svg"),
            "folder-git" => include_bytes!("../assets/folder-git.svg"),
            "keyboard" => include_bytes!("../assets/keyboard.svg"),
            "message-square" => include_bytes!("../assets/message-square.svg"),
            "monitor" => include_bytes!("../assets/monitor.svg"),
            "pointer" => include_bytes!("../assets/pointer.svg"),
            "plug" => include_bytes!("../assets/plug.svg"),
            "puzzle" => include_bytes!("../assets/puzzle.svg"),
            "tuning" => include_bytes!("../assets/tuning.svg"),
            "sliders" => include_bytes!("../assets/sliders.svg"),
            "terminal-square" => include_bytes!("../assets/terminal-square.svg"),
            "tool-branch" => include_bytes!("../assets/tool-branch.svg"),
            "claude.svg" => include_bytes!("../assets/claude.svg"),
            "codex-glyph.svg" => include_bytes!("../assets/codex-glyph.svg"),
            "opencode.svg" => include_bytes!("../assets/opencode.svg"),
            "prompt.svg" => include_bytes!("../assets/prompt.svg"),
            _ => return Ok(None),
        };
        Ok(Some(Cow::Borrowed(bytes)))
    }
    fn list(&self, _: &str) -> Result<Vec<SharedString>> {
        Ok(Vec::new())
    }
}

fn main() {
    let launched = std::time::Instant::now();
    gpui::Application::with_platform(gpui_platform::current_platform(false))
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            cx.on_window_closed(|cx, _| cx.quit()).detach();
            let bounds = Bounds::centered(None, size(px(1320.), px(860.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(820.), px(520.))),
                    app_id: Some("com.siruscode.native-spike".into()),
                    window_background: gpui::WindowBackgroundAppearance::Blurred,
                    app_owns_titlebar_drag: true,
                    titlebar: Some(TitlebarOptions {
                        title: Some("Sirus Native (teste)".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(px(12.), px((theme::TITLE_BAR - 14.) / 2.))),
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    window.on_next_frame(move |_, cx| {
                        // Measuring launch: print the time to the first frame and leave.
                        if std::env::var_os("SPIKE_MEASURE").is_some() {
                            println!("first frame after {} ms in main", launched.elapsed().as_millis());
                            cx.quit();
                        }
                    });
                    cx.new(|cx| shell::Shell::new(window, cx))
                },
            )
            .expect("open window");
            cx.activate(true);
        });
}
