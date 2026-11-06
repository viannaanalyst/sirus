# ADR-020: Provider CLI update checks and explicit npm updates

**Status:** Accepted

## Context

Users cannot tell when an installed provider CLI is outdated. A
persistent toast ("Codex update available") with Review updates and Update all
actions. Adding that to Sirus crosses two reviewed boundaries: outbound
network reads and process spawn. The project already ships fixed read-only HTTPS
probes (ADR-014) but has no package-manager execution.

## Decision

- `provider_updates.rs` checks only CLIs with a fixed first-party npm package:
  Codex (`@openai/codex`), Claude (`@anthropic-ai/claude-code`) and OpenCode
  (`opencode-ai`). The mapping is native; the renderer never supplies a package,
  a registry or a URL. Cursor/Grok are reported as unchecked.
- The check is one bounded GET per provider to
  `https://registry.npmjs.org/<package>/latest` (no redirects, no retries, 6s
  timeout, 256 KiB body cap) compared against the installed `--version` output.
  Results are cached in memory for six hours; there is no polling loop — one
  check on bootstrap when enabled plus manual refreshes.
- `AppSettings.enableProviderUpdateChecks` (default true) gates the check and is
  exposed in Settings → Providers. The renderer shows a persistent provider
  update toast with **Review updates** (opens Settings → Providers) and
  **Update all**.
- `update_providers` requires `confirm: true` and re-detects installations
  natively. npm-managed installs (canonical path inside `node_modules`) run
  `npm install -g --no-fund --no-audit <package>@latest`; a natively installed
  Claude Code runs its own `<claude> update` subcommand. Every command is a
  fixed argv resolved from detection (never a shell string, never renderer
  input). Installations without a fixed strategy report `updateSupported:
  false`, and the toast replaces Update all with the manual-update note. Output is bounded by `cli_output::capture_command`, the timeout
  is 300s, and only the last stderr line (240 chars) can reach the UI. No command
  runs automatically, and no renderer-provided package, version or flag is
  accepted. Cache is invalidated after updates and agents are re-detected.
- Update failures surface through the existing error toast; the provider update
  prompt itself is dismissible per provider/version set for the session.

## Consequences

- **Positive:** users learn about stale Codex/Claude/OpenCode CLIs and can update
  them in place; the network surface is a fixed read-only registry endpoint and
  the process surface is a fixed npm argv for three allowlisted packages.
- **Negative:** npm must be on PATH; CLIs installed through other channels
  (Homebrew, standalone installers) may be updated by npm into a different
  location than the one detected, and Cursor/Grok receive no update checks.
- **Accepted trade-off:** `npm install -g` executes package lifecycle scripts.
  That is the same trust the user grants by running the command manually, and it
  only happens after an explicit click on Update all; no silent or automatic
  update path exists.

## Alternatives considered

- Scraping vendor release pages: rejected — breaks the fixed-endpoint policy and
  needs per-vendor parsers.
- Running full update command strings: rejected — shell strings and
  renderer-selected commands violate the IPC boundary.
- Auto-updating on startup: rejected — process spawn must stay an explicit user
  action.
- Updating through vendor installers (`curl | sh`): rejected — remote scripts.
