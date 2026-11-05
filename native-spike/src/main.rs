//! Part 1 of the native spike: tabs, pinning, sidebar and motion on GPUI with sample data,
//! to compare by feel against the Tauri app. GPUI is vendored from Monocode (Apache-2.0);
//! a few presentation files are ported from Monocode (MIT, LICENSE-MONOCODE-MIT).
mod attachment_doc;
mod attachment_html;
mod attachment_ooxml;
mod attachment_pdf;
mod attachment_web;
mod backend;
mod chat_background;
mod browser_address;
mod browser_bridge;
mod browser_crop;
mod browser_data;
mod browser_device;
mod browser_js;
mod browser_pip;
mod browser_profiles;
mod browser_record;
mod browser_tab;
mod cli_diagnostics;
mod connector_marks;
mod data;
mod effort_energy;
mod file_editor;
mod file_icons;
mod folder_icon_data;
mod git_sync;
mod hover_track;
mod i18n;
mod local_git;
mod markdown;
mod marks;
mod mascots;
mod motion;
mod send_morph;
mod native_actions;
mod panel_browser;
mod panel_changes;
mod panel_files;
mod panel_sidechat;
mod panel_simulator;
mod panel_terminal;
mod panels;
mod project_avatar;
mod project_files;
mod project_scripts;
mod project_scripts_view;
mod provider_masks;
mod reply_render;
mod reply_rich;
mod reply_view;
mod settings_connectors;
mod settings_pair;
mod simulator;
mod settings_pages;
mod shell;
mod subagent_team;
mod syntax;
mod tab_label;
mod tab_tile;
mod terminal_place;
mod theme;
mod visuals;

use gpui::{
    App, AssetSource, Bounds, Result, SharedString, TitlebarOptions, WindowBounds, WindowOptions,
    point, prelude::*, px, size,
};
use std::borrow::Cow;

struct Assets;
impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        if let Some(bytes) = settings_connectors::asset(path) {
            return Ok(Some(bytes));
        }
        if let Some(bytes) = settings_pair::asset(path) {
            return Ok(Some(bytes));
        }
        let bytes: &'static [u8] = match path {
            "plus" => include_bytes!("../assets/plus.svg"),
            "cursor" => include_bytes!("../assets/cursor.svg"),
            "grok" => include_bytes!("../assets/grok.svg"),
            "antigravity" => include_bytes!("../assets/antigravity.svg"),
            "droid" => include_bytes!("../assets/droid.svg"),
            "pi" => include_bytes!("../assets/pi.svg"),
            "devin" => include_bytes!("../assets/devin.svg"),
            "hermes" => include_bytes!("../assets/hermes.svg"),
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
            "palette" => include_bytes!("../assets/palette.svg"),
            "image-plus" => include_bytes!("../assets/image-plus.svg"),
            "chevrons-up-down" => include_bytes!("../assets/chevrons-up-down.svg"),
            "folder-open" => include_bytes!("../assets/folder-open.svg"),
            "app-window" => include_bytes!("../assets/app-window.svg"),
            "trash" => include_bytes!("../assets/trash.svg"),
            "layout-grid" => include_bytes!("../assets/layout-grid.svg"),
            "filter" => include_bytes!("../assets/filter.svg"),
            "pin" => include_bytes!("../assets/pin.svg"),
            "copy" => include_bytes!("../assets/copy.svg"),
            "pencil" => include_bytes!("../assets/pencil.svg"),
            "square-pen" => include_bytes!("../assets/square-pen.svg"),
            "calendar-range" => include_bytes!("../assets/calendar-range.svg"),
            "calendar-today" => include_bytes!("../assets/calendar-today.svg"),
            "calendar-week" => include_bytes!("../assets/calendar-week.svg"),
            "calendar-month" => include_bytes!("../assets/calendar-month.svg"),
            "tab-close" => include_bytes!("../assets/tab-close.svg"),
            "tabs-others" => include_bytes!("../assets/tabs-others.svg"),
            "tabs-right" => include_bytes!("../assets/tabs-right.svg"),
            "tabs-left" => include_bytes!("../assets/tabs-left.svg"),
            "tabs-all" => include_bytes!("../assets/tabs-all.svg"),
            "zap" => include_bytes!("../assets/zap.svg"),
            "zap-fill" => include_bytes!("../assets/zap-fill.svg"),
            "star" => include_bytes!("../assets/star.svg"),
            "star-fill" => include_bytes!("../assets/star-fill.svg"),
            "chevron-left" => include_bytes!("../assets/chevron-left.svg"),
            "folder-plus" => include_bytes!("../assets/folder-plus.svg"),
            "chevrons-down-up" => include_bytes!("../assets/chevrons-down-up.svg"),
            "paperclip" => include_bytes!("../assets/paperclip.svg"),
            "goal" => include_bytes!("../assets/goal.svg"),
            "bulb" => include_bytes!("../assets/bulb.svg"),
            "bug" => include_bytes!("../assets/bug.svg"),
            "hand" => include_bytes!("../assets/hand.svg"),
            "undo" => include_bytes!("../assets/undo.svg"),
            "key" => include_bytes!("../assets/key.svg"),
            "check" => include_bytes!("../assets/check.svg"),
            "users" => include_bytes!("../assets/users.svg"),
            "square" => include_bytes!("../assets/square.svg"),
            "chevron-up" => include_bytes!("../assets/chevron-up.svg"),
            "chevron-down" => include_bytes!("../assets/chevron-down.svg"),
            "list-ordered" => include_bytes!("../assets/list-ordered.svg"),
            "pause" => include_bytes!("../assets/pause.svg"),
            "play" => include_bytes!("../assets/play.svg"),
            "square-fill" => include_bytes!("../assets/square-fill.svg"),
            "window" => include_bytes!("../assets/window.svg"),
            "square-terminal" => include_bytes!("../assets/square-terminal.svg"),
            "files" => include_bytes!("../assets/files.svg"),
            "git-compare" => include_bytes!("../assets/git-compare.svg"),
            "globe" => include_bytes!("../assets/globe.svg"),
            "messages" => include_bytes!("../assets/messages.svg"),
            "handoff" => include_bytes!("../assets/handoff.svg"),
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
            other => match file_icons::asset(other).or_else(|| panel_browser::asset(other)) {
                Some(bytes) => bytes,
                None => return Ok(None),
            },
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
            cx.on_app_quit(|_| async { backend::stop_host() }).detach();
            let bounds = Bounds::centered(None, size(px(1320.), px(860.)), cx);
            cx.open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(820.), px(520.))),
                    app_id: Some("com.siruscode.native".into()),
                    window_background: gpui::WindowBackgroundAppearance::Blurred,
                    app_owns_titlebar_drag: true,
                    titlebar: Some(TitlebarOptions {
                        title: Some("Sirus Native".into()),
                        appears_transparent: true,
                        traffic_light_position: Some(point(
                            px(12.),
                            px((theme::TITLE_BAR - 14.) / 2.),
                        )),
                    }),
                    ..Default::default()
                },
                |window, cx| {
                    window.on_next_frame(move |_, cx| {
                        // Measuring launch: print the time to the first frame and leave.
                        if std::env::var_os("SPIKE_MEASURE").is_some() {
                            println!(
                                "first frame after {} ms in main",
                                launched.elapsed().as_millis()
                            );
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
