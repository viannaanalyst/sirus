// The headless host (ADR-131) builds a subset of the app: code only the window uses is
// unused there. Dead code is judged by the desktop build, which keeps `-D warnings`.
#![cfg_attr(not(feature = "desktop"), allow(dead_code))]

#[cfg(feature = "desktop")]
use crate::error::Result;
#[cfg(feature = "desktop")]
use std::sync::Arc;
#[cfg(feature = "desktop")]
use tauri::Manager;

mod account_move;
mod acp_cli;
mod activity;
mod agent;
mod agent_output;
mod agent_pool;
mod agent_resources;
#[cfg(feature = "desktop")]
mod app_update;
mod appearance;
mod archive_removal;
mod attachment_platform;
mod attachments;
mod auto_pull;
mod browser;
mod browser_agent;
mod browser_bridge;
mod browser_cookies;
mod browser_login;
mod browser_mcp;
mod browser_profiles;
#[cfg(feature = "desktop")]
mod browser_recording;
mod chat_background;
mod ci_autofix;
mod claude;
mod cli_output;
mod close;
mod codex;
mod commands;
mod commit_title;
mod computer;
mod computer_mcp;
mod connectors;
mod context_text;
mod crash_resume;
mod delegation;
mod detect;
#[cfg(feature = "desktop")]
mod devices;
mod diagnostics;
mod dictation;
mod document_preview;
mod drafts;
mod editor;
mod error;
mod execution;
mod fs_tree;
mod gh_gate;
mod git;
mod git_incoming;
mod git_workspace;
mod github_accounts;
mod github_inbox;
mod goals;
mod html_preview;
mod idle_compaction;
mod keep_awake;
#[cfg(target_os = "macos")]
mod keychain_scope;
mod launch;
mod local_servers;
mod login_vault;
mod mcp_config;
mod mcp_stdio;
mod media_preview;
mod memory;
mod models;
mod new_project;
mod notifications;
mod opencode;
mod output_batch;
mod paths;
mod persist;
mod pr_watch;
mod process_priority;
mod process_tree;
mod project_look;
mod project_scripts;
mod prompt_link;
mod provider_accounts;
mod provider_models;
mod provider_updates;
mod provider_usage;
mod provider_usage_http;
mod pty_term;
mod pull_ask;
mod pull_requests;
mod redact;
mod remote;
mod reply_image;
mod reply_preview;
mod review_guides;
mod rotation;
mod secrets;
mod session_export;
mod session_move;
mod side_chat;
mod sidebar;
mod sirus_tools;
mod site_favicon;
mod skills;
mod snooze;
mod store;
mod subagent_feed;
mod team;
mod text_search;
mod thumbnails;
mod token_budget;
mod tool_output;
mod transcript;
mod transcript_store;
mod transcript_view;
mod turn_review;
mod turn_undo;
mod usage_history;
mod usage_prices;
mod visuals;
mod vscode_theme;
mod window_attachment;
#[cfg(feature = "desktop")]
mod window_reveal;
mod window_snap;
mod workflow;
mod workspace_entries;
mod workspace_watch;
mod worktree;
mod worktree_cleanup;

/// The headless host: CLI, command table and server (ADR-131).
#[cfg(all(feature = "host", not(feature = "desktop")))]
mod headless;
/// Tauri's items in the desktop app, stand-ins in the headless host (ADR-131).
mod runtime;
#[cfg(all(feature = "host", not(feature = "desktop")))]
pub use headless::main as run_host;

#[cfg(feature = "desktop")]
use commands::AppState;

/// Child mode used by provider CLIs: MCP over stdio, no window.
pub fn run_browser_mcp() -> i32 {
    browser_mcp::run_stdio()
}

/// Child mode used by provider CLIs for computer use: MCP over stdio, no window.
pub fn run_computer_mcp() -> i32 {
    computer_mcp::run_stdio()
}

/// The composer gets macOS spell checking (red underlines, suggestions in its context
/// menu). WebKit reads this from the app's defaults before its first web view; a choice
/// the person already made in Edit → Spelling and Grammar is kept.
#[cfg(all(feature = "desktop", target_os = "macos"))]
fn enable_spell_checking() {
    use objc2_foundation::{NSString, NSUserDefaults};
    let defaults = NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str("WebContinuousSpellCheckingEnabled");
    if defaults.objectForKey(&key).is_none() {
        defaults.setBool_forKey(true, &key);
    }
}

#[cfg(feature = "desktop")]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("sirus_code=info")),
        )
        .init();
    #[cfg(target_os = "macos")]
    enable_spell_checking();

    tauri::Builder::default()
        .manage(html_preview::HtmlPreviews::default())
        // Agent HTML rendered in the transcript, isolated on its own origin (ADR-072).
        .register_uri_scheme_protocol("sirus-preview", |ctx, request| {
            use tauri::Manager;
            html_preview::serve(
                &ctx.app_handle().state::<html_preview::HtmlPreviews>(),
                &request,
            )
        })
        // Sent image thumbnails, read from files instead of travelling in transcripts.
        .register_asynchronous_uri_scheme_protocol(thumbnails::SCHEME, |ctx, request, responder| {
            let data_path = ctx
                .app_handle()
                .try_state::<Arc<AppState>>()
                .map(|state| state.data_path.clone());
            tauri::async_runtime::spawn_blocking(move || {
                responder.respond(thumbnails::serve(data_path.as_deref(), &request));
            });
        })
        // Videos in the file pane, served by byte range for seeking (ADR-108).
        .register_asynchronous_uri_scheme_protocol(
            media_preview::SCHEME,
            |_ctx, request, responder| {
                tauri::async_runtime::spawn_blocking(move || {
                    responder.respond(media_preview::serve(&request));
                });
            },
        )
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        // `sirus://new?…` prompt links (ADR-122); handled natively in `prompt_link`.
        .plugin(tauri_plugin_deep_link::init())
        // In-app updates (ADR-140): driven from Rust only, no renderer permission.
        .plugin(tauri_plugin_updater::Builder::new().build())
        // The main window reopens where and how big it was left. Visibility
        // stays ours: the window shows only after its glass is applied (ADR-098).
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_state_flags(
                    tauri_plugin_window_state::StateFlags::all()
                        - tauri_plugin_window_state::StateFlags::VISIBLE,
                )
                .build(),
        )
        .menu(native_menu)
        .on_menu_event(|app, event| {
            // Custom native Quit goes through RunEvent::ExitRequested. macOS
            // predefined Quit calls NSApp terminate: and bypasses that event.
            if event.id().as_ref() == "sirus-quit" && close::quit_pressed(app) {
                app.exit(0);
            }
        })
        .setup(|app| {
            let resolver = app.path();
            let app_dir = resolver.app_data_dir()?;
            #[cfg(debug_assertions)]
            let app_dir = if let Some(path) = std::env::var_os("SIRUS_DATA_DIR") {
                let path = std::path::PathBuf::from(path);
                if !path.is_absolute() {
                    return Err("SIRUS_DATA_DIR must be absolute".into());
                }
                path
            } else {
                app_dir
            };
            std::fs::create_dir_all(&app_dir)?;
            {
                // Keep a small crash breadcrumb without persisting panic payloads
                // or source paths, which may contain private user data.
                let log = app_dir.join("panic.log");
                let previous = std::panic::take_hook();
                std::panic::set_hook(Box::new(move |info| {
                    use std::io::Write;
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log)
                    {
                        let line = info.location().map(|at| at.line()).unwrap_or_default();
                        let thread = std::thread::current()
                            .name()
                            .unwrap_or("unnamed")
                            .to_string();
                        let _ = writeln!(
                            file,
                            "{} [{thread}] panic at source line {line}",
                            chrono::Utc::now().to_rfc3339(),
                        );
                    }
                    previous(info);
                }));
            }
            let data_path = app_dir.join("state.json");
            browser_mcp::init(app.handle().clone(), app_dir.clone());
            secrets::init(&app_dir);
            remote::init(app.handle().clone(), &app_dir);
            devices::init(app.handle().clone(), &app_dir);
            let worktree_root = app_dir.join("worktrees");
            std::fs::create_dir_all(&worktree_root)?;
            let mut data = persist::load_or_create(&data_path)?;
            sirus_tools::set_enabled(data.settings.agents_manage_sessions);
            browser_mcp::set_agent_access(data.settings.agent_browser_access);
            agent_pool::set_limit(data.settings.keep_idle_agents as usize);
            connectors::start(&app_dir, &data_path, &mut data);
            computer_mcp::init(
                app.handle().clone(),
                app_dir.clone(),
                data.settings.computer_use_enabled,
            );
            app.manage(Arc::new(AppState::new(data_path, worktree_root, data)));
            archive_removal::delete_archived(app.state::<Arc<AppState>>().inner());
            appearance::schedule(app.handle(), app.state::<Arc<AppState>>().inner().clone());
            notifications::install(app.handle());
            memory::watch_pressure(app.handle());
            // Conversations and delegated tasks a crash cut mid-turn continue (ADR-114, ADR-118).
            crash_resume::resume_interrupted(app.handle());
            snooze::start(app.handle().clone());
            idle_compaction::start(app.handle().clone());
            auto_pull::start(app.handle().clone());
            ci_autofix::start(app.handle().clone());
            pr_watch::start(app.handle().clone());
            prompt_link::install(app.handle());
            app_update::start(app.handle());
            {
                let state = app.state::<Arc<AppState>>().inner().clone();
                let settings = state.data.lock().settings.clone();
                window_snap::apply(app.handle(), &settings);
            }
            attachment_platform::install_paste_listener(
                app.handle(),
                app.state::<Arc<AppState>>().inner().clone(),
            );
            // The main window starts hidden and shows at launch with its glass; the content reveals on the first ready paint.
            window_reveal::show_at_launch(app.handle());
            #[cfg(debug_assertions)]
            if std::env::var("SIRUS_DEVTOOLS").as_deref() == Ok("1") {
                if let Some(window) = app.get_webview_window("main") {
                    window.open_devtools();
                }
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            html_preview::html_preview,
            team::team_action,
            side_chat::side_chat_action,
            pull_ask::pull_ask_action,
            github_inbox::pull_request_action,
            github_accounts::github_account_action,
            ci_autofix::ci_autofix_action,
            pr_watch::pr_watch_action,
            local_servers::local_server_action,
            local_servers::local_servers_list,
            agent_resources::agent_resources,
            agent_resources::agent_resource_action,
            devices::device_action,
            review_guides::review_guide_action,
            window_reveal::window_ready,
            commands::retry_last_turn,
            attachments::attachment_image,
            chat_background::chat_background_action,
            chat_background::chat_background_image,
            vscode_theme::vscode_themes_search,
            vscode_theme::vscode_themes_fetch,
            vscode_theme::vscode_themes_import_local,
            site_favicon::site_favicon,
            window_snap::window_snap_action,
            app_update::app_update_action,
            project_look::pick_look_image,
            media_preview::media_preview,
            media_preview::media_release,
            media_preview::reply_media,
            workspace_watch::workspace_watch,
            project_scripts::project_scripts_action,
            worktree_cleanup::worktree_cleanup_action,
            session_export::export_conversation,
            session_move::prepare_session_move,
            session_move::find_project_by_remote,
            session_move::import_session,
            session_move::mark_session_moved,
            turn_undo::undo_turn_changes,
            turn_undo::check_turn_undo,
            commands::trash_workspace_entry,
            skills::skill_action,
            mcp_config::mcp_action,
            connectors::connector_action,
            notifications::notification_action,
            computer_mcp::computer_action,
            secrets::secret_action,
            login_vault::login_vault_list,
            login_vault::login_vault_save,
            login_vault::login_vault_delete,
            login_vault::login_vault_set_agent_use,
            login_vault::login_vault_matches,
            login_vault::login_vault_prompts,
            login_vault::login_vault_answer_offer,
            login_vault::login_vault_answer_approval,
            browser_login::login_vault_autofill,
            remote::remote_action,
            remote::remote_pair,
            remote::share_push_identity,
            reply_image::reply_image,
            commands::load_state,
            provider_accounts::create_provider_account,
            provider_accounts::select_provider_account,
            provider_accounts::rename_provider_account,
            provider_accounts::login_provider_account,
            provider_accounts::cancel_provider_account_login,
            provider_usage::provider_usage,
            provider_usage::consume_codex_reset,
            usage_history::usage_history,
            usage_prices::usage_prices,
            provider_updates::provider_updates,
            provider_updates::update_providers,
            dictation::dictation_status,
            dictation::start_dictation,
            dictation::stop_dictation,
            dictation::transcription_status,
            dictation::transcribe_audio,
            browser::browser_open,
            browser::browser_close,
            browser::browser_state,
            browser::browser_new_tab,
            browser::browser_close_tab,
            browser::browser_select_tab,
            browser::browser_navigate,
            browser::browser_reload,
            browser::browser_back,
            browser::browser_forward,
            browser::browser_set_bounds,
            browser::browser_annotate_start,
            browser::browser_annotate_finish,
            browser::browser_annotate_cancel,
            browser::browser_annotate_capture,
            browser::browser_copy_link,
            browser::browser_capture,
            browser::browser_set_zoom,
            browser::browser_picture_in_picture,
            browser::browser_frame,
            browser::browser_set_device,
            browser::browser_set_user_zoom,
            browser::browser_set_appearance,
            browser::browser_hard_reload,
            browser::browser_devtools,
            browser::browser_clear_data,
            browser::browser_input_events,
            browser_agent::browser_dialog_reply,
            browser_agent::browser_profile_delete,
            browser_agent::browser_profiles_supported,
            browser_agent::browser_cookie_sources,
            browser_agent::browser_cookie_domains,
            browser_agent::browser_cookie_import,
            browser_agent::browser_open_full_disk_access,
            browser_recording::browser_recording_start,
            browser_recording::browser_recording_append,
            browser_recording::browser_recording_finish,
            browser_recording::browser_record_reply,
            commands::save_settings,
            commands::save_composer_draft,
            commands::save_context_text,
            commands::session_pull_request,
            commands::add_project,
            commands::create_project,
            commands::clone_project,
            commands::remove_project,
            commands::open_project,
            commands::rename_project,
            commands::set_project_appearance,
            commands::git_identity,
            commands::git_status,
            commands::project_diff_stats,
            commands::git_diff,
            commands::list_branches,
            commands::checkout_branch,
            commands::create_branch,
            commands::git_commit,
            commands::git_workspace_action,
            git_incoming::git_incoming,
            git_incoming::git_pull_ff,
            commands::transcript_action,
            commit_title::commit_title_action,
            commands::git_push,
            commands::project_remote_url,
            prompt_link::take_prompt_link,
            prompt_link::build_prompt_link,
            commands::list_worktrees,
            commands::list_dir,
            commands::create_workspace_entry,
            commands::workspace_files,
            commands::search_workspace_text,
            commands::detect_agents,
            commands::create_session,
            commands::fork_session,
            commands::handoff_session,
            commands::dismiss_handoff,
            commands::set_message_pinned,
            commands::set_session_agent,
            commands::rename_session,
            snooze::snooze_session,
            commands::delete_session,
            commands::send_prompt,
            commands::stop_agent,
            commands::respond_agent_request,
            commands::steer_turn,
            commands::stop_background_task,
            commands::background_subagent,
            commands::steer_subagent,
            subagent_feed::subagent_feed,
            commands::start_terminal,
            commands::write_terminal,
            commands::resize_terminal,
            commands::stop_terminal,
            commands::host_info,
            commands::probe_provider,
            commands::list_provider_models,
            commands::set_session_model,
            account_move::move_session_account,
            pick_folder,
            attachments::pick_prompt_attachments,
            attachments::capture_prompt_window,
            document_preview::attachment_preview,
            document_preview::workspace_document,
            turn_review::keep_turn_changes,
            attachments::paste_prompt_attachments,
            attachments::drop_prompt_attachments,
            attachments::release_prompt_attachments,
            pick_executable,
            open_path,
            open_external_url,
            editor::detect_editors,
            editor::editor_app_icons,
            editor::open_in_editor,
            editor::read_text_file,
            editor::write_text_file,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Sirus Code")
        .run(|app, event| {
            match &event {
                tauri::RunEvent::Ready => {
                    // Tauri assigns its default Dock image during launch.
                    appearance::ready(app, app.state::<Arc<AppState>>().inner().clone());
                }
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::Focused(true),
                    ..
                } if label == "main" => {
                    // At most one update check every few hours (ADR-140).
                    app_update::focused(app);
                }
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::ThemeChanged(_),
                    ..
                } if label == "main" => {
                    // OS palette changes can select different glass preferences.
                    appearance::schedule(app, app.state::<Arc<AppState>>().inner().clone());
                }
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::DragDrop(drop),
                    ..
                } if label == "main" => {
                    // Finder drops (ADR-073): paths stay native; the UI only learns where it is.
                    attachments::window_drop(app, drop);
                }
                tauri::RunEvent::WindowEvent {
                    label,
                    event: tauri::WindowEvent::CloseRequested { api, .. },
                    ..
                } if label == "main" => {
                    request_close(app, || api.prevent_close());
                }
                #[cfg(target_os = "macos")]
                tauri::RunEvent::Reopen {
                    has_visible_windows: false,
                    ..
                } => {
                    // Dock click or a second launch while the window is still hidden.
                    window_reveal::reopen(app);
                }
                tauri::RunEvent::ExitRequested { api, .. } => {
                    request_close(app, || api.prevent_exit());
                }
                _ => {}
            }
            if matches!(event, tauri::RunEvent::Exit) {
                attachment_platform::remove_paste_listener();
                secrets::clear_all();
                login_vault::clear_all();
                remote::shutdown();
                devices::shutdown();
                let state = app.state::<Arc<AppState>>();
                if let Err(error) = state.shutdown() {
                    tracing::error!(%error, "cannot persist application shutdown");
                }
                // Escaped agent descendants get their SIGKILL before the app exits,
                // and the keep-awake assertion ends with the app (ADR-100).
                process_tree::finish_pending();
                keep_awake::release_all();
            }
        });
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn pick_folder(app: tauri::AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder.map(|path| path.to_string()));
    });
    Ok(rx.await.ok().flatten())
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn pick_executable(app: tauri::AppHandle) -> Result<Option<String>> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_file(move |file| {
        let _ = tx.send(file.map(|path| path.to_string()));
    });
    Ok(rx.await.ok().flatten())
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn open_path(
    app: tauri::AppHandle,
    state: tauri::State<'_, Arc<AppState>>,
    path: String,
) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let canonical = crate::paths::ensure_dir(std::path::Path::new(&path))?;
    let allowed = {
        let data = state.data.lock();
        let mut roots = data
            .projects
            .iter()
            .map(|project| std::path::PathBuf::from(&project.path))
            .collect::<Vec<_>>();
        roots.push(state.worktree_root.clone());
        if let Some(parent) = state.data_path.parent() {
            roots.push(parent.to_path_buf());
        }
        if let Some(path) = &data.settings.worktree_base_path {
            roots.push(path.into());
        }
        roots.iter().any(|root| {
            root.canonicalize()
                .is_ok_and(|root| canonical.starts_with(root))
        })
    };
    if !allowed {
        return Err(crate::error::Error::invalid_path(
            "directory is outside registered projects and Sirus Code data",
        ));
    }
    app.opener()
        .open_path(canonical.display().to_string(), None::<&str>)
        .map_err(|err| crate::error::Error::new("opener", err.to_string()))?;
    Ok(())
}

/// Strict external link policy: http(s) only, no credentials in the URL, and a
/// fixed host allowlist. Local development servers are always allowed.
#[cfg_attr(not(feature = "desktop"), allow(dead_code))]
fn validate_external_url(raw: &str) -> crate::error::Result<String> {
    let value = raw.trim();
    if value.is_empty()
        || value.len() > 2048
        || value
            .chars()
            .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return Err(crate::error::Error::invalid_path("URL is not allowed"));
    }
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .ok_or_else(|| crate::error::Error::invalid_path("URL is not allowed"))?;
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    if authority.is_empty() {
        return Err(crate::error::Error::invalid_path("URL is not allowed"));
    }
    // Reject any userinfo so a host-looking prefix cannot hide the real target.
    let (host_port, _) = match authority.split_once('@') {
        Some(_) => {
            return Err(crate::error::Error::invalid_path("URL is not allowed"));
        }
        None => (authority, ()),
    };
    // Links the person clicks in replies open in their browser on any http(s) host
    // (2026-10-08: an allowlist of github.com and localhost silently turned every other
    // link into a clipboard copy). Userinfo, control characters and empty hosts stay out.
    let host = if let Some(stripped) = host_port.strip_prefix('[') {
        stripped
            .split(']')
            .next()
            .ok_or_else(|| crate::error::Error::invalid_path("URL is not allowed"))?
    } else {
        host_port.split(':').next().unwrap_or("")
    };
    if host.is_empty()
        || !host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':') || !ch.is_ascii())
    {
        return Err(crate::error::Error::invalid_path("URL host is not allowed"));
    }
    Ok(value.to_string())
}

#[cfg(feature = "desktop")]
#[tauri::command]
async fn open_external_url(app: tauri::AppHandle, url: String) -> Result<()> {
    use tauri_plugin_opener::OpenerExt;
    let url = validate_external_url(&url)?;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|err| crate::error::Error::new("opener", err.to_string()))?;
    Ok(())
}

#[cfg(feature = "desktop")]
fn native_menu(app: &tauri::AppHandle) -> tauri::Result<tauri::menu::Menu<tauri::Wry>> {
    let menu = tauri::menu::Menu::default(app)?;
    #[cfg(target_os = "macos")]
    {
        use tauri::menu::{AboutMetadata, MenuItem, PredefinedMenuItem, Submenu};
        let metadata = AboutMetadata {
            name: Some(app.package_info().name.clone()),
            version: Some(app.package_info().version.to_string()),
            ..Default::default()
        };
        let application = Submenu::with_items(
            app,
            app.package_info().name.clone(),
            true,
            &[
                &PredefinedMenuItem::about(app, None, Some(metadata))?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::services(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::hide(app, None)?,
                &PredefinedMenuItem::hide_others(app, None)?,
                &PredefinedMenuItem::separator(app)?,
                &MenuItem::with_id(
                    app,
                    "sirus-quit",
                    "Quit Sirus Code",
                    true,
                    Some("CmdOrCtrl+Q"),
                )?,
            ],
        )?;
        // Tauri's default macOS first submenu is the application menu, whose
        // predefined Quit must be replaced before the menu becomes active.
        menu.remove_at(0)?;
        menu.insert(&application, 0)?;
    }
    Ok(menu)
}

#[cfg(feature = "desktop")]
// Native memory, never renderer state,
// decides whether a process or its spawn admission needs confirmation.
fn request_close(app: &tauri::AppHandle, prevent: impl FnOnce()) {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    let state = app.state::<Arc<AppState>>();
    let (decision, portuguese) = {
        // Same data → process lock order as admission/shutdown. Do not mutate
        // execution state while the user is deciding.
        let data = state.data.lock();
        let agents = !state.agents.lock().is_empty();
        let starting = data
            .sessions
            .iter()
            .any(|s| s.status == models::SessionStatus::Starting);
        let decision =
            state
                .close_guard
                .lock()
                .request(data.settings.confirm_close_running, agents, starting);
        (decision, data.settings.locale == "pt-BR")
    };
    match decision {
        close::Decision::Allow => {}
        close::Decision::Pending => prevent(),
        close::Decision::Ask => {
            prevent();
            let (title, message, accept, cancel) = if portuguese {
                (
                    "Fechar Sirus Code?",
                    "Há sessões em execução. Fechar interromperá os agentes. As sessões e suas mensagens serão mantidas.",
                    "Fechar e interromper",
                    "Continuar executando",
                )
            } else {
                (
                    "Close Sirus Code?",
                    "Sessions are running. Closing will interrupt agents. Sessions and their messages will be kept.",
                    "Close and interrupt",
                    "Keep running",
                )
            };
            let app = app.clone();
            let callback_app = app.clone();
            let mut dialog = app.dialog().message(message);
            if let Some(window) = app.get_webview_window("main") {
                dialog = dialog.parent(&window);
            }
            dialog
                .title(title)
                .kind(MessageDialogKind::Warning)
                .buttons(MessageDialogButtons::OkCancelCustom(
                    accept.into(),
                    cancel.into(),
                ))
                .show(move |accepted| {
                    callback_app
                        .state::<Arc<AppState>>()
                        .close_guard
                        .lock()
                        .answer(accepted);
                    if accepted {
                        callback_app.exit(0);
                    }
                });
        }
    }
}

#[cfg(test)]
mod external_url_tests {
    use super::validate_external_url;

    #[test]
    fn any_http_host_opens_but_malformed_urls_do_not() {
        assert!(validate_external_url(
            "https://docs.cnpj.ws/referencia-de-api/api-publica/consultando-cnpj"
        )
        .is_ok());
        assert!(validate_external_url("https://www.gov.br/pt-br/servicos?x=1#y").is_ok());
        assert!(validate_external_url("http://localhost:3000/").is_ok());
        assert!(validate_external_url("https://github.com/a/b/pull/1").is_ok());
        assert!(validate_external_url("https://user@evil.com/").is_err());
        assert!(validate_external_url("javascript:alert(1)").is_err());
        assert!(validate_external_url("https:///path").is_err());
        assert!(validate_external_url("https://exa mple.com").is_err());
        assert!(validate_external_url("file:///etc/passwd").is_err());
    }
}
