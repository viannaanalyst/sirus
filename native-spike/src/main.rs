//! Part 1 of the native spike: tabs, pinning, sidebar and motion on GPUI with sample data,
//! to compare by feel against the Tauri app. GPUI is vendored (Apache-2.0);
//! a few presentation files are ported from an MIT-licensed project (MIT, LICENSE-THIRD-PARTY-MIT).
mod agent_pointer;
mod app_icon;
mod app_update;
mod attachment_doc;
mod attachment_html;
mod attachment_ooxml;
mod attachment_pdf;
mod attachment_web;
mod backend;
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
mod chat_background;
mod cli_diagnostics;
mod computer_move;
mod computers;
mod connector_marks;
mod conversation_settle;
mod data;
mod disclosure;
mod edit_menu;
mod effort_energy;
mod file_chips;
mod file_editor;
mod file_icons;
mod folder_icon_data;
mod fonts;
mod freeze_watch;
#[cfg(feature = "ghostty")]
mod ghostty_vt;
mod git_graph;
mod git_split;
mod git_sync;
mod hotkey_conflicts;
mod hover_track;
mod i18n;
mod image_markup;
mod inline_badges;
mod local_git;
mod markdown;
mod marks;
mod memory_trim;
mod motion;
mod native_actions;
mod native_notify;
mod new_tab_field;
mod panel_browser;
mod panel_changes;
mod panel_files;
mod panel_sidechat;
mod panel_simulator;
mod panel_terminal;
mod panels;
mod pointer_overlay;
mod prefs;
mod project_actions;
mod project_avatar;
mod project_files;
mod project_scripts;
mod project_scripts_view;
mod provider_masks;
mod pull_requests_page;
mod quick_composer;
mod remote_tls;
mod reply_render;
mod reply_rich;
mod reply_view;
mod segment_pill;
mod send_morph;
mod settings_connectors;
mod settings_infisical;
mod settings_pages;
mod settings_pair;
mod settings_search;
mod shell;
mod sidebar_items;
mod simulator;
mod sound;
mod step_lines;
mod subagent_team;
mod syntax;
mod tab_label;
mod tab_tile;
mod terminal_engine;
mod terminal_place;
mod theme;
mod tool_images;
mod visuals;
mod window_place;
mod workspace_choice;
mod workspace_follow;

use gpui::{
    App, AssetSource, Result, SharedString, TitlebarOptions, WindowOptions, point, prelude::*, px,
    size,
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
            "sparkles" => include_bytes!("../assets/sparkles.svg"),
            "minus" => include_bytes!("../assets/minus.svg"),
            "comment-add" => include_bytes!("../assets/comment-add.svg"),
            "cursor" => include_bytes!("../assets/cursor.svg"),
            "grok" => include_bytes!("../assets/grok.svg"),
            "antigravity" => include_bytes!("../assets/antigravity.svg"),
            "droid" => include_bytes!("../assets/droid.svg"),
            "pi" => include_bytes!("../assets/pi.svg"),
            "devin" => include_bytes!("../assets/devin.svg"),
            "hermes" => include_bytes!("../assets/hermes.svg"),
            "close" => include_bytes!("../assets/close.svg"),
            "more-horizontal" => include_bytes!("../assets/more-horizontal.svg"),
            "sidebar" => include_bytes!("../assets/sidebar.svg"),
            "sidebar-right" => include_bytes!("../assets/sidebar-right.svg"),
            "chevron-right" => include_bytes!("../assets/chevron-right.svg"),
            "search" => include_bytes!("../assets/search.svg"),
            "pencil-line" => include_bytes!("../assets/pencil-line.svg"),
            "bot" => include_bytes!("../assets/bot.svg"),
            "refresh" => include_bytes!("../assets/refresh.svg"),
            "task-clock" => include_bytes!("../assets/task-clock.svg"),
            "cloud" => include_bytes!("../assets/cloud.svg"),
            "cloud-download" => include_bytes!("../assets/cloud-download.svg"),
            "cloud-upload" => include_bytes!("../assets/cloud-upload.svg"),
            "cloud-sync" => include_bytes!("../assets/cloud-sync.svg"),
            "commit" => include_bytes!("../assets/commit.svg"),
            "chart-column" => include_bytes!("../assets/chart-column.svg"),
            "mic" => include_bytes!("../assets/mic.svg"),
            "arrow-up" => include_bytes!("../assets/arrow-up.svg"),
            "palette" => include_bytes!("../assets/palette.svg"),
            "image" => include_bytes!("../assets/image.svg"),
            "wrap-text" => include_bytes!("../assets/wrap-text.svg"),
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
            "square-plus" => include_bytes!("../assets/square-plus.svg"),
            "alert-circle" => include_bytes!("../assets/alert-circle.svg"),
            "queue-list" => include_bytes!("../assets/queue-list.svg"),
            "queue-steer" => include_bytes!("../assets/queue-steer.svg"),
            "queue-pause" => include_bytes!("../assets/queue-pause.svg"),
            "queue-play" => include_bytes!("../assets/queue-play.svg"),
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
            "test-tube" => include_bytes!("../assets/test-tube.svg"),
            "hammer" => include_bytes!("../assets/hammer.svg"),
            "rocket" => include_bytes!("../assets/rocket.svg"),
            "package" => include_bytes!("../assets/package.svg"),
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
            "git-pull-request" => include_bytes!("../assets/git-pull-request.svg"),
            "git-merge" => include_bytes!("../assets/git-merge.svg"),
            "circle-dot" => include_bytes!("../assets/circle-dot.svg"),
            "upload" => include_bytes!("../assets/upload.svg"),
            "server" => include_bytes!("../assets/server.svg"),
            "folder" => include_bytes!("../assets/folder.svg"),
            "sun" => include_bytes!("../assets/sun.svg"),
            "moon" => include_bytes!("../assets/moon.svg"),
            "eye" => include_bytes!("../assets/eye.svg"),
            "eye-off" => include_bytes!("../assets/eye-off.svg"),
            "clipboard" => include_bytes!("../assets/clipboard.svg"),
            "lock" => include_bytes!("../assets/lock.svg"),
            "shield" => include_bytes!("../assets/shield.svg"),
            "file-text" => include_bytes!("../assets/file-text.svg"),
            "wrench" => include_bytes!("../assets/wrench.svg"),
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
            "codex.svg" => include_bytes!("../assets/codex.svg"),
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
    let app = gpui::Application::with_platform(gpui_platform::current_platform(false))
        .with_assets(Assets);
    // `sirus://new?…` prompt links (ADR-122): macOS hands them over as an Apple Event, and the
    // window takes them from this channel (shell/prompt_link.rs).
    let (links, link_events) = async_channel::unbounded::<String>();
    app.on_open_urls(move |urls| {
        if let Some(url) = shell::prompt_link::link_in(&urls) {
            let _ = links.try_send(url);
        }
    });
    app.run(move |cx: &mut App| {
        cx.on_window_closed(|cx, _| cx.quit()).detach();
        cx.on_app_quit(|_| async {
            window_place::flush();
            backend::stop_host()
        })
        .detach();
        freeze_watch::start(cx);
        let bounds = window_place::opening(cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(bounds),
                    window_min_size: Some(size(px(window_place::MIN.0), px(window_place::MIN.1))),
                    app_id: Some("com.siruscode.native".into()),
                    window_background: gpui::WindowBackgroundAppearance::Blurred,
                    app_owns_titlebar_drag: true,
                    titlebar: Some(TitlebarOptions {
                        title: Some("Sirus".into()),
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
                    cx.new(|cx| {
                        cx.observe_window_bounds(window, |_, window, _| {
                            window_place::remember(window.window_bounds())
                        })
                        .detach();
                        shell::Shell::new(window, cx)
                    })
                },
            )
            .expect("open window");
        memory_trim::start(window.into(), cx);
        quick_composer::start(window, cx);
        cx.spawn(async move |cx| {
            while let Ok(url) = link_events.recv().await {
                let _ = window.update(cx, |shell, window, cx| {
                    window.activate_window();
                    shell.prompt_link_received(url, cx);
                });
            }
        })
        .detach();
        cx.activate(true);
    });
}
