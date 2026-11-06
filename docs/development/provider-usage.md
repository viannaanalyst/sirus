# Provider usage footer

The composer sits above a 32 px footer: provider names/icons/quota bars on the left, refresh and visibility controls alongside them, Terminal on the right. Details identify the current or monitored provider, the account used for the last query, quota windows, reset times and last-read timestamp. “Add account” opens accounts for that provider; the separate footer plus controls which providers are monitored. Enter/exit animations use motion tokens and reduce to opacity with reduced motion. The compact indicator prefers the longer window, including Go's calendar month without inventing a fixed duration. Unknown percentages stay unknown; passing a reset timestamp never manufactures a zero-percent snapshot.

| Provider | Source | Support |
| --- | --- | --- |
| Codex | [Official app-server](https://learn.chatgpt.com/docs/app-server), `account/rateLimits/read` and `account/read` with `refreshToken: false` | Installed CLI 0.159.3 returned two real windows and account email. Named earned-reset availability is native-owned. |
| Claude Code | Anthropic's published [`@anthropic-ai/claude-agent-sdk` 0.3.286 definitions](https://unpkg.com/@anthropic-ai/claude-agent-sdk@0.3.286/sdk.d.ts), experimental `get_usage` with `skip_behaviors: true`, initialize account | Installed CLI 2.1.286 returned two real subscription windows and email. API-key/third-party sessions may have no quota. |
| Cursor | Fixed dashboard GET `https://cursor.com/api/usage-summary` and `/api/auth/me` | Installed CLI 2026.09.28-64d2043 credentials returned two usage pools and account email. Percentages are already 0–100; billing reset comes from the server. These are private dashboard APIs, not a documented personal CLI quota protocol. |
| OpenCode Go | Official server [usage endpoint](https://github.com/anomalyco/opencode/blob/dev/packages/console/app/src/routes/zen/go/v1/usage.ts), fixed GET `https://opencode.ai/zen/go/v1/usage` | Existing Go key returned rolling 5-hour, weekly and monthly percentages/reset dates. The endpoint has no email; the popup identifies the connected key by a short SHA-256 fingerprint. |
| Grok | No verified quota protocol in this adapter | Explicit unavailable state; no invented data. |

Go quota is subscription usage, not local `opencode stats` token/cost history or the limits of every backend selected through OpenCode. See the [official Go docs](https://opencode.ai/v2/docs/console/go). Cursor's [usage documentation](https://cursor.com/help/models-and-usage/usage-limits), installed CLI credential code and independent [CodexBar implementation](https://github.com/steipete/CodexBar/blob/main/docs/cursor.md) informed its adapter. It reads the CLI account rather than browser cookies or the desktop application's database. Cursor and OpenCode retain their existing default login; this adapter has no verified isolated multi-account login for them.

## Named accounts

Codex and Claude have a default account plus up to 32 registered named profiles across providers. In a usage popup, **Add account** opens the account list, then creates a name and starts the vendor's browser login. **Manage accounts** edits profile names. Rows show CLI-reported email, plan and usage; a missing identity is explicitly unverified. Selecting a verified account changes the default for new sessions. Existing sessions keep their profile, including after switching to another provider and returning. Legacy sessions bind to `default`.

Native-generated UUIDs select private directories under AppData `provider-accounts/{provider}/{id}`. Only child processes receive `CODEX_HOME`, or `CLAUDE_CONFIG_DIR` and `CLAUDE_SECURESTORAGE_CONFIG_DIR`; ambient API/OAuth token variables are removed for named profiles. Agent execution, exact native resume, quota reads and earned resets all use the same registered profile. A missing/unsafe directory fails rather than falling back to the system login. Only metadata, selection and session profile IDs persist in Sirus's JSON; vendor credentials remain owned by the CLI.

Five typed native commands create/select/rename a registered profile and start/cancel its login. Login uses only `codex login` or `claude auth login --claudeai`, fixed argv, null stdin, bounded discarded output, a ten-minute deadline and owned process-group cleanup. Closing the account panel cancels its pending login. Default-profile login, token input/import, profile deletion and re-login to a profile already used by a session are unavailable. See [ADR-015](../decisions/ADR-015-isolated-provider-accounts.md).

The binding is to a CLI profile, not an immutable vendor account ID. External vendor tools can still change credentials in that profile, especially the shared default; Sirus does not prevent external reauthentication. The popup identifies the last-read account, and reset redemption separately requires a fresh matching email. Isolated login commands and environment names were checked against installed CLI help local profile adapter. Empty-profile probes verified that both installed CLIs do not inherit default account identity. A real browser login needs the user's participation and was not completed by automated tests. Codex credential ownership is documented in [Codex authentication](https://learn.chatgpt.com/docs/auth); Claude custom configuration lives in [Claude settings](https://code.claude.com/docs/en/settings).

## Credential and cache boundary

CLI quota probes use the same native child-command builder as agent turns and model discovery. It augments the child's PATH with fixed local installation roots, including the runtime needed by npm's `env node` launcher when the app starts from Finder with a minimal environment. The parent's environment is unchanged; no login shell or renderer/project-supplied PATH is accepted. Account-profile binding is still applied afterward. `usage_probe_supports_desktop_path` verifies quota reads in an isolated child with a minimal desktop PATH and disposable runtime/profile fixtures.

The owner explicitly authorized existing credential access for these two adapters; [ADR-014](../decisions/ADR-014-scoped-authenticated-quota-reads.md) records the narrow policy exception. Secrets stay native and never enter argv, IPC, logs or Sirus persistence. Only normalized windows and bounded email/name/plan/key-fingerprint metadata cross IPC. Snapshots are memory-only. No quota polling, token refresh, auth/config modification or automatic request retry. User-initiated login in a new named Codex/Claude profile is the separate exception described above.

- Cursor on macOS reads only Keychain service `cursor-access-token`, account `cursor-user`, through an exact read-only lookup. Linux reads `$XDG_CONFIG_HOME/cursor/auth.json` (default `~/.config/cursor/auth.json`); Windows reads `%APPDATA%/Cursor/auth.json`. Expired/unsupported credentials require the user's existing vendor login flow; no alternate account is searched.
- Go reads only `opencode-go` with `type: api`. Explicit `OPENCODE_AUTH_CONTENT` is authoritative. Otherwise its known `auth.json` lives under absolute `OPENCODE_DATA_DIR`, or `$XDG_DATA_HOME/$OPENCODE_APPNAME` (defaults `~/.local/share/opencode`). Reads are bounded to 256 KiB; unsafe final symlinks, foreign-owner files, directories and special files are rejected on Unix.
- HTTP permits only three fixed GET endpoints, with TLS, no redirects/proxies/retries and bounded 256 KiB responses. Profile and quota use the same credential. Cache entries bind executable override and native credential digest; a changed credential invalidates a cached or in-flight read. Ordinary reads cache for 60 seconds; explicit refreshes have a two-second minimum interval. Reads serialize natively and cancel at shutdown.

## Earned Codex resets

Reset uses only an earned credit after explicit “use one reset” confirmation. Named credit details must come from a fresh native read; count-only responses cannot authorize redemption. The opaque offer is short-lived, executable/profile/account-email-bound and reserved once. A fresh CLI account must match before consumption; post-reset windows come from a new native read. A confirmed outcome survives a subsequent quota-read failure with unknown windows and an explicit refresh message. No purchase, overage, plan change or automatic redemption/retry. See [ADR-013](../decisions/ADR-013-provider-usage-and-earned-resets.md).

## Read-only verification

With existing vendor logins:

```bash
cd src-tauri
cargo test provider_usage::tests::live_usage_without_inference -- --ignored --nocapture
cargo test provider_usage_http::tests::live_http_quotas_without_inference -- --ignored --nocapture
```

Both passed against existing accounts: two windows each for Codex, Claude and Cursor; three for Go. The probes assert actual CLI/Cursor email and Go key identification without printing values. No prompt, transcript scan or reset consumption occurs. Routine tests cover percentage units, missing data, calendar windows, scoped credentials, expiry, bounded files, rejected account changes and normalized UI metadata. Production React DOM interactions use controlled quota/reset fixtures; they do not prove real credit redemption or native desktop geometry.
