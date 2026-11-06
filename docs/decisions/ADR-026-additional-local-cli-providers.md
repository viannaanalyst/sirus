# ADR-026: Additional local CLI providers with bounded print adapters

**Status:** Accepted.

## Context

Sirus needs nine provider identities while preserving local Session/worktree ownership and truthful native capabilities. Antigravity, Factory Droid, Pi and Devin offer headless CLIs, but their interaction protocols and permission guarantees differ from the three existing native-resume adapters.

## Decision

- Add typed IDs in Rust and TypeScript, real executable detection, provider registry entries, CLI catalogs and native-owned process streaming/cancellation. No new IPC, credential reader, remote cloud adapter or automatic login.
- Resolve allowlisted binaries using inherited absolute PATH directories and fixed conventional user/system install locations, without running a login shell. Child-only PATH also supports npm launchers that require `env node`.
- Antigravity: `agy --output-format stream-json --mode default --sandbox -p <Task-prefixed prompt>`. Decode only agent-response deltas and the terminal result; hide tools/metadata. Terminal sandbox is not a filesystem sandbox for every tool. Vendor headless permission requests cannot be answered here.
- Droid: `droid exec --output-format stream-json --cwd <cwd> --auto low -- <prompt>`. Planning omits `--auto`, selecting the CLI's read-only default. No medium/high autonomy or unsafe bypass. Models are the installed CLI's explicit Available Models section in `exec --help`; presence does not verify account access.
- Pi: `pi --print --mode json --no-session --no-extensions --no-approve --offline --tools read,grep,find,ls,edit,write -- <prompt>`. Planning selects only read/grep/find/ls. Shell, extension and project-local trust-gated resources are excluded. Tool restrictions are not OS filesystem isolation. Catalog uses the same resource exclusions and `--list-models`; no automatic model refresh. Native JSON message events retain assistant deltas, skip snapshots/tools/reasoning and propagate final errors.
- Devin: `devin --print --permission-mode accept-edits --sandbox --respect-workspace-trust true -- <prompt>`, local only. Never skip workspace trust or use dangerous/autonomous/cloud modes. Plain-text output preserves JSON/code answers. Catalog uses `models list --format json`, retaining only bounded model ID/name fields from known containers.
- Ask and Full are unavailable in all four new adapters. Droid/Pi/Devin expose only Auto; Antigravity uses vendor policy without a selectable host profile. Effort and Fast remain unavailable until real catalog metadata and execution negotiation are implemented and verified.
- Each turn owns a fresh child/process group and reuses existing bounded textual history; no implicit vendor continuation. Structured streams fail closed on malformed stdout. Existing generation/owner validation, worktree cwd validation and output ceilings remain unchanged.

## Verification and limits

On 2026-10-01, official artifacts were installed without replacing vendor credentials or launching login. Antigravity 1.2.14, Droid 0.231.0, Pi 1.0.0 and Devin 3000.11.3 responded to version/help. Antigravity/Droid/Devin artifacts passed published checksums; Pi came from its official npm package with lifecycle scripts disabled. Droid advertised 51 model IDs in its installed help. Pi offered no authenticated models; Antigravity and Devin catalog probes requested sign-in. Headless probes in disposable directories did not establish authenticated inference; Antigravity exceeded the short smoke-probe deadline. No claim of successful inference, exact native resume, host tool approvals, quota access or universal sandbox isolation follows from these checks.

## Consequences

- **Positive:** nine real provider identities, original transparent provider marks and independent upstream model family icons; missing auth/catalog support remains explicit.
- **Negative:** new print adapters lack exact native continuation and host permission callbacks; constrained modes cannot perform every coding task.
- **Accepted trade-off:** bounded, documented capabilities without silently escalating permissions or fabricating model access.

## Alternatives considered

- Copying another app's adapters: rejected; use first-party CLI interfaces.
- Registering UI-only placeholders or hardcoded available catalogs: rejected; detection and output must be real.
- Adding generic installation, argv, config or token IPC: rejected; native authority remains narrow.

## Sources

- [Antigravity headless](https://www.antigravity.google/docs/cli/headless/) and [installation](https://www.antigravity.google/docs/cli/install/).
- [Droid exec](https://docs.factory.ai/droid-exec/overview); installed `droid exec --help` is authoritative where current flag spelling differs from docs.
- [Pi CLI](https://pi.dev/docs/latest/cli) and first-party npm package documentation (`docs/json.md`, `dist/cli/list-models.js`).
- [Devin command reference](https://docs.devin.ai/cli/reference/commands) and [quickstart](https://docs.devin.ai/cli/index).
