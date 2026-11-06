# Provider execution controls

Checked against official documentation and installed CLI metadata on 2026-10-01. The native catalog, not a model-name heuristic, authorizes adjustable controls. A model can reason without exposing a manual effort selector. Fast is a separate service/variant capability, not a synonym for low effort.

The composer enables the effort range only when multiple selectable levels exist. A catalog with one level keeps the bar at its upper endpoint, disables dragging and labels it Fixed instead of repeating the same level at both endpoints. A catalog with no levels shows provider-managed effort. For example, the installed OpenCode catalog offers only `max` for Kimi K3 and no manual variants for Kimi K2.7 Code; this is not evidence of multiple adjustable levels.

| Adapter | Effort | Fast | Planning |
| --- | --- | --- | --- |
| Codex | Offered `model/list` levels → typed `turn/start.effort` | Offered priority/default service tier | Read-only turn and collaboration plan preset |
| Claude Code | Initialization model levels → `--effort` | Supporting model and account availability → process-only `fastMode`; startup confirms it before input | Native plan permission mode |
| Cursor | Initialization-only ACP model parameters → retained known values in one model argument; exact legacy presets remain supported | Offered `fast=true/false`, independent of manual effort | Fixed `--mode plan`, existing sandbox and Auto-review |
| OpenCode | Actual verbose catalog variants → offered and confirmed ACP reasoning option | No verified independent priority toggle | Offered and confirmed plan mode plus child-only edit-denial policy |
| Grok | CLI must advertise `--effort`; discovered 4.7/4.6 offer low/medium/high/xhigh, 4.5 low/medium/high | No verified independent priority toggle | Not implemented in this adapter |

| Antigravity | Not exposed | Not exposed | Not exposed |
| Droid | Not exposed | Not exposed | CLI default read-only; no `--auto` |
| Pi | Not exposed | Not exposed | Built-in read/grep/find/ls tools only |
| Devin | Not exposed | Not exposed | Not exposed |

The four additional adapters use CLI-native, constrained print modes; details, installation and authentication limits are in [ADR-026](../decisions/ADR-026-additional-local-cli-providers.md). Their catalog presence does not prove authentication or credits. Provider and upstream model marks are bundled SVGs; provenance is in `src/assets/providers/SOURCES.md` and `src/assets/models/SOURCES.md`.

## Sources and interpretation

- **Codex:** [app-server](https://developers.openai.com/codex/app-server) and [configuration reference](https://developers.openai.com/codex/config-reference). The installed app-server schema/catalog supplies the actual offered levels and typed service tier; approval and workspace policies use the typed profiles in ADR-017.
- **Claude:** [model configuration](https://code.claude.com/docs/en/model-config), [Fast](https://code.claude.com/docs/en/fast-mode) and [permissions](https://code.claude.com/docs/en/permissions). Effort and Fast availability differ by model/account. Fast may require usage credits or organization authorization. Sirus exposes sanitized reason codes and never enables billing or modifies the user's vendor configuration.
- **Cursor:** [model parameters](https://cursor.com/docs/sdk/python), [CLI parameters](https://cursor.com/docs/cli/reference/parameters), [ACP](https://cursor.com/docs/cli/acp) and [Composer 2.5](https://cursor.com/docs/models/cursor-composer-2-5). Parameter IDs/values vary by model. The installed CLI's `cursor/list_available_models` returned Composer's `fast` option with both values, default true, and no effort option. Together with the documented effort calibration, this supports automatic effort and a real Fast button, not an invented manual slider. Native decoding excludes arbitrary context/thinking/router parameters.
- **OpenCode:** [models and variants](https://opencode.ai/docs/models/), [CLI](https://opencode.ai/docs/cli/) and [ACP](https://opencode.ai/docs/acp/); native config negotiation is implemented in the [official adapter source](https://github.com/anomalyco/opencode/blob/dev/packages/opencode/src/acp/agent.ts). Named known strengths are presented in order; custom arbitrary variant names are not interpreted as effort or Fast.
- **Grok:** [CLI reference](https://docs.x.ai/build/cli/reference), [reasoning](https://docs.x.ai/developers/model-capabilities/text/reasoning) and [permissions](https://docs.x.ai/build/features/permissions). The documented model table is explicitly maintained in this adapter and additionally gated by the installed CLI flag. Unknown models receive no invented levels.

## Verification and limits

- The native Cursor catalog test passed without creating a session or sending a prompt. Composer Fast and reasoning-only/combined controls were present in real metadata; legacy IDs remain valid for existing selections.
- An OpenCode ACP probe used disposable XDG directories, selected an actually offered free model and confirmed `effort=high` without `session/prompt`. This verifies negotiation, not inference. Native fixtures verify confirmation failures and planning edit denial at every effective policy layer. Upstream APIError remains the separate inference blocker recorded in the runtime guide.
- Claude initialization reports `extra_usage_disabled` on this account, so Fast remains disabled. Codex/Claude typed execution and default clearing have native regression coverage; fresh paid inference was not requested for this refinement.
- Grok is not installed or overridden locally; its documented wiring has not been exercised against a live CLI. CLI catalogs alone do not prove model access or quota.
- Approval rows select the native profiles in [ADR-017](../decisions/ADR-017-per-turn-approval-profiles.md). OpenCode Auto-review allows file edits only; Cursor manual host decisions and selectable Grok modes remain unavailable. Full is an explicit per-turn selection, displayed in orange, and cannot combine with planning. The installed Claude CLI successfully acknowledged manual, auto, bypassPermissions and plan in disposable directories without any user prompt; full-profile inference and every vendor policy combination are not covered by that check.

Implementation/security rationale: [ADR-012](../decisions/ADR-012-composer-execution-and-attachments.md). Agent runtime limitations: [runtime guide](runtime.md).
