# ADR-015: Isolated provider accounts with session profile binding

**Status:** Accepted

## Context

The owner clarified that the usage popup must add another account of the same provider, rather than another monitored provider. Separate Codex/Claude profiles can pin conversations to their initial profile. A global logout/login switch would also change existing sessions and the user's external CLI. This authorizes a narrow new-profile vendor-login exception to ADR-014's account-management prohibition; its Cursor/Go read-only credential policy and ADR-013's earned-reset rules remain.

## Decision

- `provider_accounts.rs` owns a bounded registry of native-generated profile UUIDs, labels and selected defaults. The renderer supplies only a provider, registered ID or bounded label through five typed create/select/rename/login/cancel commands. No auth token, arbitrary directory, URL, env or process arguments are accepted.
- Codex and Claude named profiles live in private, owner-validated AppData directories. Their fixed child environment selects `CODEX_HOME`, or `CLAUDE_CONFIG_DIR` plus `CLAUDE_SECURESTORAGE_CONFIG_DIR`, and removes inherited API/OAuth token variables. The default profile continues using the user's existing environment. Missing or unsafe named profiles fail without fallback.
- Only an explicit user action starts `codex login` or `claude auth login --claudeai` in a new/unused profile. The vendor owns the browser flow and credential storage. The host never imports, copies, logs or returns credentials. Login has null stdin, bounded discarded output, a ten-minute deadline and cancellation/shutdown cleanup of the owned process group. Default login and re-login to a profile used by sessions are rejected. Profile deletion is not offered.
- A session persists its selected profile at creation and retains a per-provider binding when changing providers. Exact native continuation validates profile identity along with the existing project/cwd/model/session identity. Changing the selected account applies to new sessions only. Legacy sessions retain `default`.
- Process execution, quota cache keys and Codex reset offers use the same profile. Reset consumption retains fresh email validation, confirmation, once-only reservation and idempotency. Quota/account snapshots remain memory-only; no polling is introduced.
- Account rows display verified CLI email/plan/quota; missing identity is unverified. Unverified named profiles cannot be selected through the UI; the existing default remains selectable for API-key accounts without email. Management currently means renaming. Cursor/OpenCode have working default-account quotas but no verified isolated multi-account login in these adapters.

## Consequences

Multiple Codex/Claude accounts can coexist without replacing the system login. A session binds a profile rather than a cryptographically immutable vendor identity: external vendor tools can still replace credentials in that directory, especially the shared default. This is an explicit limitation, not a guarantee against external reauthentication. No cloud account or Sirus login is introduced.

Native regressions cover private paths, foreign IDs, environment isolation, fixed login/cancel, session binding and profile-scoped reset offers. Empty-profile probes against both installed CLIs verified absence of inherited default identity without logging in or running inference. Automated tests exercise login with a disposable process fixture; a real browser login requires the user's participation. Operational details and sources: [usage guide](../development/provider-usage.md).

## Addendum (2026-11-11): account names and per-project accounts

- **Names.** Any account can be named from the usage popup's account list (a pencil on hover turns the row into an inline field; Enter saves, Esc cancels). An added account's name is its `label` (never empty). The terminal login (`default`) can now be named too: its name lives in `AppData.defaultAccountLabels`, and an empty name clears it back to the e-mail. The window shows the name when there is one and the e-mail otherwise; with a name, the e-mail moves to the row's second line (picker) or sits beside it, smaller (account row).
- **Per-project account.** `Project.providerAccounts` remembers the account chosen per provider for that project. Picking an account in the popup calls `select_provider_account` with the conversation's `projectId`, which sets only that project's entry, and moves the open conversation onto it (an empty conversation included). `provider_accounts::selected_for(data, project, provider)` is what new sessions, handoffs, provider switches and commit titles in a known project use: the project's entry while that account exists, else `default` — a project never inherits another project's choice. The global `selectedProviderAccounts` stays only as the fallback for callers with no project (model catalogs, usage without an account, `select_provider_account` without `projectId`). Removing an account clears it from the global choice and every project. The popup shows the open conversation's bound account when it runs on that provider, else its project's choice, and asks usage for that account, so switching conversations or projects shows the right limits.

## Alternatives considered

Changing global authentication, copying existing credentials and accepting token input would expand authority and break session ownership. A cosmetic “Add account” button opening provider visibility would repeat the original product error. Offering unverified isolation for Cursor/OpenCode would falsely imply account separation.
