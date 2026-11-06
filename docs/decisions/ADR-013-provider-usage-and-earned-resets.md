# ADR-013: Native usage probes and explicit earned-reset redemption

**Status:** Superseded by [ADR-014](ADR-014-scoped-authenticated-quota-reads.md) for credential-source and account-metadata policy. Reset lifecycle and visibility rules are retained.

## Context

The composer footer needs account quota snapshots and available Codex resets. Renderer-owned subprocesses, credential readers and polling are outside Sirus's authority model. Redeeming an earned reset consumes a finite account credit.

## Decision

- `provider_usage` is a fixed, typed Client/Transport operation. Rust launches only the detected Codex or Claude CLI, with bounded input/output, deadlines, owned-group cleanup and shutdown cancellation. No inference, credential readers, login or generic RPC is exposed. Native cache entries are executable-bound, last at most 60 seconds for ordinary reads, and explicit refreshes have a two-second minimum interval. Frontend requests coalesce per executable and queue a replacement after a path change.
- Codex uses `account/rateLimits/read`. Claude uses its experimental `get_usage` control with `skip_behaviors: true`; its child disables hooks and unrelated MCP configuration and does not persist a session. Only normalized quota percentages, window IDs/durations, reset timestamps and reset availability cross IPC. Unsupported providers and incompatible CLIs return an explicit unavailable/error state, never invented percentages.
- Available Codex credits remain native-owned. The renderer receives one opaque UUID offer instead of a vendor credit ID. `consume_codex_reset` requires explicit confirmation, a current executable-bound offer younger than ten minutes, and a freshly revalidated, named, available, unexpired `codexRateLimits` credit. Count-only availability cannot authorize redemption. The offer is reserved once before any side effect and supplies the UUID idempotency key. Duplicate, stale, foreign or unconfirmed offers are rejected. No automatic retry or redemption occurs.
- Redemption uses only `account/rateLimitResetCredit/consume` with the retained native credit ID; it does not buy credits, enable overage, change plans or modify authentication/configuration. Native limits are read afterward; the UI never infers fresh zero-percent windows from a reset outcome. An uncertain operation requires a read-only refresh before another deliberate attempt.
- *Amended (ADR-049 era): the composer footer was removed; up to two quota rings in the sidebar rail (`AppSettings.sidebarUsageProviders`) now host this panel and refresh on mount, visibility restoration and the end of a followed provider's turn.* Provider visibility is persisted in `AppSettings.usageProviders`; quota snapshots and offers remain memory-only. The current provider is always shown. The footer refreshes on mount/provider changes, explicit actions, visibility restoration subject to cache, and completion of a monitored agent turn. There are no polling loops or countdown timers. Relative reset labels update with renders; absolute timestamps and last-read times are available in the details.

## Consequences

Codex and Claude subscription quotas work without host token access. Claude's experimental control can change in future releases, and API-key/third-party accounts may have no subscription quota. Cursor, OpenCode and Grok quota probes remain unsupported; their usage panels say so. Multi-account sign-in/switching is outside this feature. Reset redemption is tested with fixtures and native offer checks, not by spending the owner's credits.

Sources and verified versions: [provider usage guide](../development/provider-usage.md).
