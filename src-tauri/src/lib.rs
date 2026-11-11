// The back end of the headless host (ADR-131). Some code was written for the retired Tauri
// window and is no longer reached; it stays until it is deleted module by module.
#![allow(dead_code)]

mod account_move;
mod acp_cli;
mod activity;
mod agent;
mod agent_output;
mod agent_pool;
mod agent_resources;
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
mod diagnostics;
mod dictation;
mod document_preview;
mod drafts;
mod editor;
mod error;
mod execution;
mod forge;
mod fs_tree;
mod gh_gate;
mod git;
mod git_incoming;
mod git_workspace;
mod github_accounts;
mod github_inbox;
mod goals;
mod host_update;
mod html_preview;
mod idle_compaction;
mod infisical;
mod infisical_setup;
mod issue_ref;
mod keep_awake;
#[cfg(target_os = "macos")]
mod keychain_scope;
mod launch;
mod local_servers;
mod login_vault;
mod mcp_config;
mod mcp_external;
#[cfg(unix)]
mod mcp_http;
mod mcp_stdio;
mod media_preview;
mod memory;
mod models;
mod new_project;
mod notifications;
mod opencode;
mod output_batch;
mod paths;
mod peer_tools;
mod peers;
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
mod scheduled_prompts;
mod secrets;
mod session_export;
mod share_links;
mod session_move;
mod side_chat;
mod sidebar;
mod sirus_tools;
mod site_favicon;
mod skill_scope;
mod skills;
mod snooze;
mod store;
mod subagent_feed;
mod team;
mod text_search;
mod thumbnails;
mod token_budget;
mod tool_images;
mod tool_output;
mod transcript;
mod transcript_store;
mod transcript_view;
mod tunnel;
mod turn_edit;
mod turn_review;
mod turn_undo;
mod usage_history;
mod usage_prices;
mod usage_summary;
mod visuals;
mod vscode_theme;
mod window_attachment;
mod window_snap;
mod workflow;
mod workspace_entries;
mod workspace_watch;
mod worktree;
mod worktree_cleanup;

/// The headless host: CLI, command table and server (ADR-131).
#[cfg(feature = "host")]
mod headless;
/// The stand-ins for Tauri's runtime that the shared modules use (ADR-131).
mod runtime;
#[cfg(feature = "host")]
pub use headless::main as run_host;

/// Child mode used by provider CLIs: MCP over stdio, no window.
pub fn run_browser_mcp() -> i32 {
    browser_mcp::run_stdio()
}

/// Child mode used by provider CLIs for computer use: MCP over stdio, no window.
pub fn run_computer_mcp() -> i32 {
    computer_mcp::run_stdio()
}

/// Strict external link policy: http(s) only, no credentials in the URL, and a
/// fixed host allowlist. Local development servers are always allowed.
#[allow(dead_code)]
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
