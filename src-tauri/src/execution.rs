//! Bounded per-turn preferences, never arbitrary protocol/config passthrough.
use crate::{
    error::{Error, Result},
    models::{AgentProviderId, ApprovalMode, ExecutionOptions},
    provider_models::ProviderModelList,
};
use serde_json::{json, Value};
pub const EFFORTS: &[&str] = &[
    "none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra",
];

/// A queued request cannot silently adopt a different account/model/workspace
/// or follow a different turn. These values restrict admission, never grant it.
pub fn validate_queued_context(
    session: &crate::models::Session,
    expected: Option<&crate::models::QueuedPromptContext>,
) -> Result<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let message_id = session
        .messages
        .iter()
        .rev()
        .find(|message| message.role == crate::models::MessageRole::Agent)
        .map(|message| message.id.as_str());
    if expected.agent != session.agent
        || expected.model != session.model
        || expected.provider_account_id != session.provider_account_id
        || expected.worktree_path != session.worktree.path
        || expected.message_id.as_deref() != message_id
        || session.status.is_active()
    {
        return Err(Error::new(
            "stale",
            "Queued request context changed. Review the queue before sending again.",
        ));
    }
    Ok(())
}

/// How many of a session's latest user messages a send's id is checked against.
const RECENT_SENDS: usize = 100;

/// A send's client id, if any: a hyphenated UUID, kept lowercase. Anything else is refused,
/// so the stored value stays small and comparable.
pub fn client_request_id(raw: Option<&str>) -> Result<Option<String>> {
    let Some(raw) = raw else {
        return Ok(None);
    };
    match uuid::Uuid::parse_str(raw) {
        Ok(id) if raw.len() == 36 => Ok(Some(id.hyphenated().to_string())),
        _ => Err(Error::new(
            "invalid",
            "clientRequestId must be a hyphenated UUID",
        )),
    }
}

/// True when one of the session's recent user messages came from this send already: a
/// retried or doubled delivery, which must not start a second turn (idempotent sends).
pub fn already_sent(session: &crate::models::Session, client_request_id: &str) -> bool {
    session
        .messages
        .iter()
        .rev()
        .filter(|message| message.role == crate::models::MessageRole::User)
        .take(RECENT_SENDS)
        .any(|message| message.client_request_id.as_deref() == Some(client_request_id))
}

/// The `@model:<provider>/<id>` mentions the composer's `@` menu writes, in order and once
/// each. Sentence punctuation after a mention is not part of it; unknown providers are prose.
pub fn model_mentions(text: &str) -> Vec<(AgentProviderId, String)> {
    let mut found: Vec<(AgentProviderId, String)> = Vec::new();
    for word in text.split_whitespace() {
        let Some(rest) = word
            .trim_start_matches(['(', '[', '"', '\''])
            .strip_prefix("@model:")
        else {
            continue;
        };
        let Some((provider, model)) = rest.split_once('/') else {
            continue;
        };
        let model = model.trim_end_matches(['.', ',', ';', ':', '!', '?', ')', ']', '"', '\'']);
        let Ok(provider) = serde_json::from_value::<AgentProviderId>(json!(provider)) else {
            continue;
        };
        if model.is_empty() || found.iter().any(|(p, m)| *p == provider && m == model) {
            continue;
        }
        found.push((provider, model.to_owned()));
    }
    found
}

/// A request that @-mentions a model asks for that model to do the work: the agent is told to
/// hand it over with `sirus_delegate` (ADR-111), so it runs as a linked subagent (ADR-198).
pub fn model_mention_prompt(request: String) -> String {
    let mentions = model_mentions(&request);
    if mentions.is_empty() {
        return request;
    }
    let targets = mentions
        .iter()
        .map(|(provider, model)| {
            format!(
                "- @model:{key}/{model} → sirus_delegate with provider \"{key}\" and model \"{model}\" ({title})",
                key = provider.key(),
                title = provider.title()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{request}\n\nSirus model mention: the person named the agent that should do this work. Delegate the request to it with sirus_delegate instead of doing it yourself, writing a self-contained task (objective, context, constraints, expected result) without the mention token; with several mentions, split the work between them as the request says.\n{targets}\nIf delegation fails, say why and ask before doing the work yourself.")
}

pub fn debug_prompt(debugging: bool, request: String) -> String {
    if !debugging {
        return request;
    }
    format!("{request}\n\nSirus Debug mode: Observe the real state, reproduce the defect, investigate testable hypotheses, fix the smallest root cause and verify the original symptom. Collect relevant logs/errors when accessible. Add a regression test when practical and run appropriate checks before claiming success. Keep the current permission policy. When reproduction requires the user or evidence is inaccessible, give precise steps and ask for that evidence; never claim to observe external actions you cannot access.")
}

/// The approval modes a provider accepts; empty when it takes none (its own policy applies).
pub fn approval_modes(provider: &AgentProviderId) -> &'static [ApprovalMode] {
    match provider {
        AgentProviderId::Grok => &[],
        AgentProviderId::Droid | AgentProviderId::Pi => &[ApprovalMode::Auto],
        _ => &[ApprovalMode::Ask, ApprovalMode::Auto, ApprovalMode::Full],
    }
}

pub(crate) fn approval_rank(mode: ApprovalMode) -> u8 {
    match mode {
        ApprovalMode::Ask => 0,
        ApprovalMode::Auto => 1,
        ApprovalMode::Full => 2,
    }
}

/// The approval work started on someone's behalf runs with: the inherited
/// mode when the target provider offers it, else the broadest mode it offers that is no
/// broader than the inherited one, and none for a provider that takes no mode. Never broader
/// than what the caller had.
pub fn approval_for(
    provider: &AgentProviderId,
    inherited: ApprovalMode,
) -> Result<Option<ApprovalMode>> {
    let modes = approval_modes(provider);
    if modes.is_empty() {
        return Ok(None);
    }
    if modes.contains(&inherited) {
        return Ok(Some(inherited));
    }
    modes
        .iter()
        .copied()
        .filter(|mode| approval_rank(*mode) <= approval_rank(inherited))
        .max_by_key(|mode| approval_rank(*mode))
        .map(Some)
        .ok_or_else(|| {
            Error::agent(
                "That provider only runs with broader access than this conversation allows; pick another provider or change the access mode.",
            )
        })
}

/// Providers with a native planning (read-only) mode.
pub fn supports_planning(provider: &AgentProviderId) -> bool {
    matches!(
        provider,
        AgentProviderId::Codex
            | AgentProviderId::Claude
            | AgentProviderId::Cursor
            | AgentProviderId::OpenCode
            | AgentProviderId::Droid
            | AgentProviderId::Pi
            | AgentProviderId::Devin
            | AgentProviderId::Hermes
            | AgentProviderId::Antigravity
    )
}

pub fn validate(
    provider: &AgentProviderId,
    model: Option<&str>,
    options: &ExecutionOptions,
    catalog: Option<&ProviderModelList>,
) -> Result<()> {
    if options
        .approval
        .is_some_and(|mode| !approval_modes(provider).contains(&mode))
    {
        return Err(Error::agent(
            "This approval mode is not supported by this adapter.",
        ));
    }
    if options.planning && options.approval == Some(ApprovalMode::Full) {
        return Err(Error::agent(
            "Full access cannot be combined with planning mode.",
        ));
    }
    if options.planning && !supports_planning(provider) {
        return Err(Error::agent(
            "Planning mode is not supported by this adapter.",
        ));
    }
    if options.planning && *provider == AgentProviderId::Codex && model.is_none() {
        return Err(Error::agent(
            "Select a model before enabling planning mode.",
        ));
    }
    if options.effort.is_none() && !options.fast {
        return Ok(());
    }
    let entry = catalog
        .filter(|list| &list.provider == provider)
        .and_then(|list| {
            list.models.iter().find(|row| {
                Some(row.id.as_str()) == model
                    && row.availability == crate::provider_models::ModelAvailability::Available
            })
        })
        .ok_or_else(|| {
            Error::agent("Refresh the model catalog before changing execution options.")
        })?;
    if let Some(effort) = &options.effort {
        if !EFFORTS.contains(&effort.as_str())
            || !entry.effort_levels.contains(effort)
            || !matches!(
                provider,
                AgentProviderId::Codex
                    | AgentProviderId::Claude
                    | AgentProviderId::OpenCode
                    | AgentProviderId::Grok
                    | AgentProviderId::Cursor
                    | AgentProviderId::Devin
            )
            || (*provider == AgentProviderId::Cursor && !entry.parameterized)
        {
            return Err(Error::agent(
                "This model does not support the selected effort.",
            ));
        }
    }
    if options.fast
        && (!matches!(
            provider,
            AgentProviderId::Codex | AgentProviderId::Claude | AgentProviderId::Cursor
        ) || !entry.fast_mode
            || (*provider == AgentProviderId::Cursor && !entry.parameterized))
    {
        return Err(Error::agent(
            "Fast mode is not supported by this adapter/model.",
        ));
    }
    Ok(())
}

/// Build only known Cursor bracket parameters from native catalog-retained IDs/values.
pub fn cursor_model(
    model: Option<&str>,
    options: &ExecutionOptions,
    catalog: Option<&ProviderModelList>,
) -> Result<Option<String>> {
    let Some(model) = model else {
        return Ok(None);
    };
    // The renderer supplies a catalog ID, never bracket parameters or option-like values.
    if model.is_empty()
        || model.len() > 200
        || !model
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphanumeric())
        || !model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    {
        return Err(Error::agent("Invalid Cursor model ID"));
    }
    let entry = catalog
        .filter(|list| list.provider == AgentProviderId::Cursor)
        .and_then(|list| list.models.iter().find(|row| row.id == model));
    let Some(entry) = entry.filter(|row| row.parameterized) else {
        return Ok(Some(model.into()));
    };
    let mut parameters = Vec::new();
    if let Some(effort) = options.effort.as_ref() {
        let key = entry
            .effort_parameter
            .as_deref()
            .filter(|id| ["effort", "reasoning", "reasoning_effort"].contains(id))
            .ok_or_else(|| Error::agent("Cursor effort parameter is unavailable"))?;
        let value = entry
            .effort_values
            .get(effort)
            .filter(|value| EFFORTS.contains(&value.as_str()) || value.as_str() == "extra-high")
            .ok_or_else(|| Error::agent("Cursor effort value is unavailable"))?;
        parameters.push(format!("{key}={value}"));
    }
    if entry.fast_mode {
        parameters.push(format!("fast={}", options.fast));
    }
    Ok(Some(if parameters.is_empty() {
        model.into()
    } else {
        format!("{model}[{}]", parameters.join(","))
    }))
}

pub fn codex_policy(options: &ExecutionOptions) -> (&'static str, &'static str, &'static str) {
    match options.approval {
        Some(ApprovalMode::Full) => ("danger-full-access", "never", "user"),
        Some(ApprovalMode::Auto) => ("workspace-write", "on-request", "auto_review"),
        _ => ("workspace-write", "on-request", "user"),
    }
}

pub fn claude_permission_mode(options: &ExecutionOptions) -> &'static str {
    if options.planning {
        return "plan";
    }
    match options.approval {
        Some(ApprovalMode::Full) => "bypassPermissions",
        Some(ApprovalMode::Auto) => "auto",
        _ => "manual",
    }
}

pub fn codex_turn(
    params: &mut Value,
    options: &ExecutionOptions,
    model: Option<&str>,
) -> Result<()> {
    let (_, approval, reviewer) = codex_policy(options);
    params["approvalPolicy"] = json!(approval);
    params["approvalsReviewer"] = json!(reviewer);
    params["sandboxPolicy"] = if options.approval == Some(ApprovalMode::Full) {
        json!({"type":"dangerFullAccess"})
    } else {
        json!({"type":"workspaceWrite","writableRoots":params["cwd"].as_str().into_iter().collect::<Vec<_>>(),"networkAccess":false,"excludeTmpdirEnvVar":true,"excludeSlashTmp":true})
    };
    // Explicit default tier clears Fast on an exact resumed native thread.
    params["serviceTier"] = json!(if options.fast { "priority" } else { "default" });
    params["effort"] = json!(options.effort);
    if options.planning {
        let model = model.ok_or_else(|| Error::agent("Planning requires a selected model."))?;
        params["collaborationMode"] = json!({"mode":"plan","settings":{"model":model,"reasoning_effort":options.effort,"developer_instructions":null}});
        params["sandboxPolicy"] = json!({"type":"readOnly"});
    } else if let Some(model) = model {
        // Clear a previous plan preset; never leave resumed turns in a hidden mode.
        params["collaborationMode"] = json!({"mode":"default","settings":{"model":model,"reasoning_effort":options.effort,"developer_instructions":null}});
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_mentions_become_a_delegation_instruction() {
        assert_eq!(
            model_mentions(
                "@model:claude/sonnet revise (@model:codex/gpt-5.5). @model:claude/sonnet again \
                 @model:nobody/x @model:codex/ email@model:codex/y"
            ),
            vec![
                (AgentProviderId::Claude, "sonnet".to_owned()),
                (AgentProviderId::Codex, "gpt-5.5".to_owned()),
            ]
        );
        assert_eq!(model_mention_prompt("plain".into()), "plain");
        let prompt = model_mention_prompt("@model:opencode/anthropic/opus fix it".into());
        assert!(prompt.starts_with("@model:opencode/anthropic/opus fix it\n\n"));
        assert!(prompt.contains("provider \"opencode\" and model \"anthropic/opus\""));
    }

    #[test]
    fn queued_context_rejects_changed_identity_history_and_active_turns() {
        let session: crate::models::Session = serde_json::from_value(json!({
            "id":"s", "projectId":"p", "title":"Task", "agent":"codex", "model":"model",
            "providerAccountId":"profile", "status":"completed", "createdAt":"time", "lastActivityAt":"time",
            "worktree":{"path":"/fixture", "branch":"main", "isolated":true}, "lastError":null,
            "messages":[{"id":"answer", "sessionId":"s", "role":"agent", "content":"Done", "createdAt":"time", "streaming":false}]
        })).unwrap();
        let expected: crate::models::QueuedPromptContext = serde_json::from_value(json!({
            "messageId":"answer", "agent":"codex", "model":"model", "providerAccountId":"profile", "worktreePath":"/fixture"
        })).unwrap();
        assert!(validate_queued_context(&session, Some(&expected)).is_ok());
        assert!(validate_queued_context(&session, None).is_ok());
        for field in [
            "messageId",
            "model",
            "providerAccountId",
            "worktreePath",
            "agent",
        ] {
            let mut value = serde_json::to_value(&expected).unwrap();
            value[field] = json!(if field == "agent" {
                "claude"
            } else {
                "changed"
            });
            let changed = serde_json::from_value(value).unwrap();
            assert!(
                validate_queued_context(&session, Some(&changed)).is_err(),
                "{field}"
            );
        }
        let mut active = session.clone();
        active.status = crate::models::SessionStatus::Running;
        assert!(validate_queued_context(&active, Some(&expected)).is_err());
        // Resume after failure is explicit in the queue; native admission still
        // enforces the same binding and settled history.
        active.status = crate::models::SessionStatus::Failed;
        assert!(validate_queued_context(&active, Some(&expected)).is_ok());
        let legacy: crate::models::SendPromptRequest =
            serde_json::from_value(json!({"sessionId":"s", "prompt":"Next"})).unwrap();
        assert!(legacy.queued_after.is_none());
    }
    #[test]
    fn debug_is_native_prompt_guidance_and_legacy_requests_default_off() {
        let request: crate::models::SendPromptRequest = serde_json::from_value(json!({
            "sessionId":"session", "prompt":"Reproduce the error"
        }))
        .unwrap();
        assert!(!request.debugging);
        assert_eq!(debug_prompt(false, request.prompt.clone()), request.prompt);
        let output = debug_prompt(true, request.prompt.clone());
        assert!(output.starts_with(&request.prompt));
        assert!(output.contains("reproduce the defect"));
        assert!(output.contains("current permission policy"));
        assert!(output.contains("never claim to observe external actions"));
        assert_eq!(request.prompt, "Reproduce the error");
    }
    #[test]
    fn devin_acp_offers_every_access_mode_and_planning() {
        for mode in [ApprovalMode::Ask, ApprovalMode::Auto, ApprovalMode::Full] {
            let options = ExecutionOptions {
                approval: Some(mode),
                ..Default::default()
            };
            assert!(validate(&AgentProviderId::Devin, None, &options, None).is_ok());
        }
        let planning = ExecutionOptions {
            planning: true,
            ..Default::default()
        };
        assert!(validate(&AgentProviderId::Devin, None, &planning, None).is_ok());
    }
    #[test]
    fn additional_adapters_reject_unimplemented_permission_profiles() {
        for provider in [AgentProviderId::Droid, AgentProviderId::Pi] {
            for mode in [ApprovalMode::Ask, ApprovalMode::Full] {
                assert!(validate(
                    &provider,
                    None,
                    &ExecutionOptions {
                        approval: Some(mode),
                        ..Default::default()
                    },
                    None
                )
                .is_err());
            }
            let auto = ExecutionOptions {
                approval: Some(ApprovalMode::Auto),
                ..Default::default()
            };
            assert!(validate(&provider, None, &auto, None).is_ok());
        }
        for provider in [AgentProviderId::Droid, AgentProviderId::Pi] {
            assert!(validate(
                &provider,
                None,
                &ExecutionOptions {
                    planning: true,
                    ..Default::default()
                },
                None
            )
            .is_ok());
        }
    }
    #[test]
    fn approval_profiles_reach_codex_and_clear_full_access_on_the_next_turn() {
        let auto: ExecutionOptions = serde_json::from_value(json!({"approval":"auto"})).unwrap();
        let full: ExecutionOptions = serde_json::from_value(json!({"approval":"full"})).unwrap();
        let ask: ExecutionOptions = serde_json::from_value(json!({"approval":"ask"})).unwrap();
        let mut params = json!({"cwd":"/fixture"});
        codex_turn(&mut params, &auto, None).unwrap();
        assert_eq!(params["approvalsReviewer"], "auto_review");
        assert_eq!(params["approvalPolicy"], "on-request");
        assert_eq!(params["sandboxPolicy"]["type"], "workspaceWrite");
        codex_turn(&mut params, &full, None).unwrap();
        assert_eq!(params["approvalPolicy"], "never");
        assert_eq!(params["sandboxPolicy"]["type"], "dangerFullAccess");
        codex_turn(&mut params, &ask, None).unwrap();
        assert_eq!(params["approvalsReviewer"], "user");
        assert_eq!(params["sandboxPolicy"]["type"], "workspaceWrite");
        assert_eq!(params["sandboxPolicy"]["networkAccess"], false);
    }

    #[test]
    fn delegated_work_steps_down_to_a_mode_the_target_offers() {
        use ApprovalMode::*;
        assert_eq!(
            approval_for(&AgentProviderId::Codex, Auto).unwrap(),
            Some(Auto)
        );
        assert_eq!(approval_for(&AgentProviderId::Grok, Full).unwrap(), None);
        assert_eq!(
            approval_for(&AgentProviderId::Droid, Full).unwrap(),
            Some(Auto)
        );
        assert_eq!(
            approval_for(&AgentProviderId::Pi, Auto).unwrap(),
            Some(Auto)
        );
        assert!(
            approval_for(&AgentProviderId::Droid, Ask).is_err(),
            "never broader"
        );
    }

    #[test]
    fn unsupported_approval_modes_and_full_access_planning_are_rejected() {
        for mode in ["ask", "auto", "full"] {
            let options: ExecutionOptions =
                serde_json::from_value(json!({"approval":mode})).unwrap();
            assert!(validate(&AgentProviderId::Grok, None, &options, None).is_err());
            // Cursor runs through ACP (ADR-107): every access mode, answered by Sirus.
            assert!(validate(&AgentProviderId::Cursor, None, &options, None).is_ok());
        }
        let options: ExecutionOptions =
            serde_json::from_value(json!({"approval":"full","planning":true})).unwrap();
        assert!(validate(&AgentProviderId::Claude, None, &options, None).is_err());
        assert!(serde_json::from_value::<ExecutionOptions>(json!({"approval":"--force"})).is_err());
    }
    #[test]
    fn cursor_arguments_use_native_parameter_values_and_clear_fast_without_generic_overrides() {
        let mut catalog:ProviderModelList=serde_json::from_value(json!({"provider":"cursor","source":"cli","note":"","models":[{"id":"composer-2.5","displayName":"Composer","availability":"available","parameterized":true,"fastMode":true},{"id":"gpt-5.5","displayName":"GPT","availability":"available","parameterized":true,"effortLevels":["xhigh"],"effortParameter":"reasoning","effortValues":{"xhigh":"extra-high"}}]})).unwrap();
        let fast = ExecutionOptions {
            fast: true,
            ..Default::default()
        };
        assert!(validate(
            &AgentProviderId::Cursor,
            Some("composer-2.5"),
            &fast,
            Some(&catalog)
        )
        .is_ok());
        assert_eq!(
            cursor_model(Some("composer-2.5"), &fast, Some(&catalog))
                .unwrap()
                .as_deref(),
            Some("composer-2.5[fast=true]")
        );
        assert_eq!(
            cursor_model(
                Some("composer-2.5"),
                &ExecutionOptions::default(),
                Some(&catalog)
            )
            .unwrap()
            .as_deref(),
            Some("composer-2.5[fast=false]")
        );
        let effort = ExecutionOptions {
            effort: Some("xhigh".into()),
            ..Default::default()
        };
        assert_eq!(
            cursor_model(Some("gpt-5.5"), &effort, Some(&catalog))
                .unwrap()
                .as_deref(),
            Some("gpt-5.5[reasoning=extra-high]")
        );
        catalog.models[1]
            .effort_values
            .insert("xhigh".into(), "xhigh,fast=true".into());
        assert!(cursor_model(Some("gpt-5.5"), &effort, Some(&catalog)).is_err());
        assert!(validate(
            &AgentProviderId::Cursor,
            Some("composer-2.5"),
            &effort,
            Some(&catalog)
        )
        .is_err());
        for model in [
            "composer-2.5[fast=true]",
            "--force",
            "model\n",
            "",
            "m,fast=true",
        ] {
            assert!(cursor_model(Some(model), &ExecutionOptions::default(), None).is_err());
        }
    }
    #[test]
    fn unsupported_options_fail_closed_and_default_requires_no_catalog() {
        let default = ExecutionOptions::default();
        assert!(validate(&AgentProviderId::Grok, None, &default, None).is_ok());
        assert!(validate(
            &AgentProviderId::Grok,
            None,
            &ExecutionOptions {
                planning: true,
                ..default.clone()
            },
            None
        )
        .is_err());
        assert!(validate(
            &AgentProviderId::Claude,
            Some("opus"),
            &ExecutionOptions {
                fast: true,
                ..default.clone()
            },
            None
        )
        .is_err());
        let catalog:ProviderModelList = serde_json::from_value(json!({"provider":"codex","source":"cli","note":"","models":[{"id":"m","displayName":"M","availability":"available","effortLevels":["low","high"],"fastMode":true}]})).unwrap();
        for effort in ["ultra", "--dangerously", "high\n"] {
            assert!(validate(
                &AgentProviderId::Codex,
                Some("m"),
                &ExecutionOptions {
                    effort: Some(effort.into()),
                    ..default.clone()
                },
                Some(&catalog)
            )
            .is_err());
        }
        assert!(validate(
            &AgentProviderId::Codex,
            Some("m"),
            &ExecutionOptions {
                effort: Some("high".into()),
                fast: true,
                planning: false,
                ..Default::default()
            },
            Some(&catalog)
        )
        .is_ok());
        assert!(validate(
            &AgentProviderId::Codex,
            Some("foreign"),
            &ExecutionOptions {
                fast: true,
                ..default
            },
            Some(&catalog)
        )
        .is_err());
    }
    #[test]
    fn codex_options_clear_fast_and_planning_without_broadening_permissions() {
        let mut params = json!({"approvalPolicy":"on-request","sandboxPolicy":{"type":"workspaceWrite","networkAccess":false}});
        codex_turn(
            &mut params,
            &ExecutionOptions {
                effort: Some("high".into()),
                fast: true,
                planning: true,
                ..Default::default()
            },
            Some("m"),
        )
        .unwrap();
        assert_eq!(params["serviceTier"], "priority");
        assert_eq!(params["sandboxPolicy"]["type"], "readOnly");
        assert_eq!(params["collaborationMode"]["mode"], "plan");
        assert_eq!(params["approvalPolicy"], "on-request");
        codex_turn(&mut params, &ExecutionOptions::default(), Some("m")).unwrap();
        assert_eq!(params["serviceTier"], "default");
        assert_eq!(params["collaborationMode"]["mode"], "default");
    }
}
