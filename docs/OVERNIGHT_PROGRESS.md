# Overnight progress

Evidence ledger for the Sirus overnight mission, resumed through **2026-10-01**. This replaces the earlier partial checkpoint. Implementation, automated checks, actual provider calls and real-window interaction are separate claims. No phase is declared complete on the strength of a mock.

## Starting point

The inherited app already had JSON persistence, projects, Session/worktree types, CLI detection/catalogs, basic Git/diff/file/PTY surfaces, Settings and sidebar collapse. Initial typecheck/build/Rust check passed, but native warnings, lifecycle races, partial i18n and missing test/lint infrastructure remained. Graphify and ADRs were inspected before cross-module work. This folder has no `.git` metadata; no repository initialization, commits, push, release or deployment occurred.

## Concluído

UI Arc Pro is excluded by the owner. All 100 free MIT components are included.

- Native lifecycle and transcripts: cancel interrupts wait, owned process groups stop, readers drain before exit, native protocol failures remain failures, output is bounded, checkpoints save partial text, restart preserves interrupted conversations.
- Provider/model domains: five adapters/registry identities, bounded cached detection/discovery, provider-qualified model identity, central brand resolver, persisted model/provider enablement and favorites, keyboard-searchable model selection and model-aware Session creation.
- **Actual provider edits:** Codex, Claude Code and the initial OpenCode integrations each wrote the required file in their disposable isolated worktree, streamed normalized text, completed and preserved the original checkout. Codex also passed with the actually discovered explicit model `gpt-6.1-sol`. **Cursor edits:** passed an actual native isolated edit using Auto-review with sandbox enabled, without force/yolo.
- **Actual desktop Codex flow:** the isolated validation app created a Session/worktree, accepted a prompt through the composer and persisted a completed `sirus-desktop.txt` edit. The original checkout remained untouched. This proves one UI→Client→IPC→native→provider loop, not every desktop interaction.
- Native interaction: Codex app-server and Claude stream-json provide exact bound continuation and native-owned, one-request host decisions. OpenCode now uses native ACP with exact bound load and offered one-shot file approvals; initial-policy live edit/resume/refusal/cancel passed, but final hardened-policy inference remains unverified after a shared vendor APIError. Actual desktop Claude Write approval, refusal and follow-up passed; ordinary command/file/secret/unsupported limits are explicit.
- Projects/workspaces/review: canonical paths, safe unique worktrees, confirmed dirty-aware removal, real structured inventory linked to Sessions, lazy file selection, NUL-safe status, staged/unstaged/deleted diffs and bounded regular-file previews.
- Terminal core: real PTY in the Session workspace, input/resize/close/reopen APIs, pre-spawn listeners, generation/queue ownership, incremental UTF-8, owned shell/background-job cleanup. Native tests verify an unrelated process survives cleanup.
- UI Arc: **all 100 public MIT components** vendored with license and original SHA-256 provenance; all 100 render in the Advanced Settings explorer. Main buttons/switches/overlays, New Session, dialogs, search, composer, errors, code/diff/workspace surfaces reuse Arc through host tokens/primitives. Gallery examples are explicitly local demonstrations, never backend functionality.
- Preferences: real 12/13/14 typography scaling, persisted validated custom shortcuts and opt-in rotating aggregate lifecycle diagnostics. Session reopen opt-out preserves history and drafts without starting agents. Native main-window close/Quit confirmation uses a parented sheet; actual decline, repeated Quit and accepted interruption passed.
- UI/i18n: full-page Settings, truthful disabled upcoming controls, pt-BR default/English persisted, labels/tooltips/empty states, central shortcuts, collapse with accessible reopen, reduced motion, focus-visible and trigger-based overlay motion.
- Composer: multiline/send/stop, model/workspace indicators, owner-validated restart-persisted drafts, explicit workspace/diff context snapshots bounded to 12 KiB. Draft context cannot name a worktree before it exists.
- Security/performance: native trust boundary, no generic execute IPC, production CSP separation, jailed paths, Git filesystem-hook/fsmonitor suppression, configured-hook refusal and conservative external-filter refusal, bounded probes/previews/streams, blocking work on native workers, lazy Settings/terminal/gallery and no background polling.
- Tests/lint, independent source reviews, competitive research/backlog, runtime/build/Arc documentation and ADR updates. Local macOS debug packaging now uses an explicit ad hoc signing override without distribution credentials.

### Checklist of all requested phases

“Implemented” means the code and the stated checks exist. Desktop acceptance limitations apply to visual/keyboard rows; they are not hidden by this checklist.

| Phase | Result | Evidence / remaining limit |
| --- | --- | --- |
| 0 Audit | Implemented | Root contract, ADRs, frontend/native/domain audit, Graphify and this ledger |
| 1 Foundation | Implemented | Lifecycle/save/selection fixes; typecheck, native checks, tests and build |
| 2 Design system | Implemented | Existing APIs adapt Arc; neutral OFF/accent ON Switch, corrected thumb geometry, motion/focus tokens |
| 3 Motion system | Implemented | Central host/Arc tokens, short overlay entry/exit, trigger origins, OS/host reduced motion |
| 4 Sidebar | Implemented | Persisted centralized default Cmd+B with custom override, fade then width, accessible reopen, main space recovery |
| 5 Settings | Implemented | All eight categories, rows/separators, native values, unsupported controls disabled/Coming soon |
| 6 i18n | Implemented | Main product flows/aria/errors translated; persisted pt-BR/English fallback; raw external text unchanged |
| 7 Provider registry | Implemented | Five identities, central capabilities/notes, native argv/decoder/catalog adapters |
| 8 Detection | Implemented | Real executable/path/version, bounded concurrent probes, cache and explicit refresh |
| 9 Provider icons | Partial | Claude/Codex/Cursor/OpenCode bundled assets; official Grok archive refused HTTP 403 |
| 10 Model registry | Implemented | Provider-qualified descriptors: display/brand/availability/preferences; model capabilities explicitly unknown rather than inferred |
| 11 Discovery | Implemented | Real Codex/Cursor/OpenCode/Claude catalogs; official temporary Grok catalog verified with unauthenticated rows unavailable; aliases only an explicit unknown fallback |
| 12 Brand resolver | Implemented | Central model-family→asset mapping independent of hosting provider |
| 13 Enable models | Implemented | Persisted preferences filter new choices, existing Sessions preserved |
| 14 Favorites | Implemented | Settings/selector, persisted provider-qualified keys, favorites-first, brief interaction |
| 15 Model selector | Implemented | Model + provider, search/groups/favorites/enablement/availability and keyboard handling |
| 16 Projects | Implemented | Add/open/recent/canonical Git identity/persistence, metadata-only removal and errors |
| 17 Session domain | Implemented | Explicit native-owned states, project/model/worktree/branch/timestamps/messages, process maps native-only |
| 18 New Session | Implemented | Quick project/model/workspace/default flow, no process until Send; real desktop fixture passed |
| 19 Worktrees | Implemented | Real list/create/Session association/path/branch/confirmed dirty-aware removal tests |
| 20 Agent layer | Implemented | Native start/input/stream/status/stop/exit; Codex typed host approvals/questions and Claude original-input tool approvals; OpenCode native ACP; Cursor/Grok interaction limits remain explicit |
| 21 Codex | Implemented | Actual native app-server edit, exact bound follow-up and typed approval decline; discovered model and desktop edit loop passed |
| 22 Claude Code | Implemented | Native and desktop Write allow/refusal, exact bound follow-up and restart continuation with existing CLI login |
| 23 Other providers | Partial | OpenCode native ACP initial-policy suite passed; final hardened-policy prompts fail with APIError also seen in CLI run; Cursor Auto-review edit passed; Grok authenticated execution blocked |
| 24 Streaming | Implemented | Native deltas/message identity/offset, start/run/fail/stop/completion, stderr separation; no fabricated percentages |
| 25 Composer | Implemented | Multiline, send/stop, readiness/errors, model/workspace, real opt-in context, bounded snapshots |
| 26 File tree | Implemented | Lazy expand/collapse/select, heavy-directory exclusions, permission/retry handling and real path context menu |
| 27 Git status | Implemented | NUL parsing and actual change kinds, event/explicit refresh with stale-response protection |
| 28 Diff | Implemented | File/status/counts, staged/unstaged/deleted/untracked preview, copy and readable colored hunks |
| 29 Terminal | Implemented | Native PTY fixtures plus actual desktop input, correct worktree cwd, keyboard resize, close/reopen, exit/reopen and Git review passed |
| 30 Palette | Implemented | Session/project/terminal/sidebar/Settings actions, search and keyboard navigation, shared motion |
| 31 Keyboard-first | Partial | Central shortcuts and focus/menu paths reviewed; actual model/worktree Session creation, palette/sidebar/custom bindings, Settings search/recording focus, native Quit, gallery and PTY resize tested; exhaustive gestures/platforms unverified |
| 32 Empty states | Implemented | Relevant project/Session/model/provider/Git/file/workspace states and actions |
| 33 Errors | Implemented | Structured native IPC, normalized Error/toasts, retained failed prompts, no React-destroying rejection handler |
| 34 Security | Implemented review | Paths/FS/Git/PTY/providers/IPC/CSP reviewed; regression fixtures; documented POSIX/cross-platform limits |
| 35 Performance | Implemented | Cached lazy discovery, worker I/O, event streaming, lazy directories/large UI chunks; optional gallery size warning remains |
| 36 Polish | Implemented scoped | Arc/tokens/focus/control cleanup; actual modal cascade, Settings focus leak and context tabs clipped at minimum width fixed; targeted native screenshots/interactions verified |
| 37 Docs | Implemented | Contract, README, runtime/build/Arc guides, lifecycle/design ADRs and source-backed backlog |
| 38 Tests | Implemented | Minimal Node runner + real Rust fixtures + 100-preview rendering + opt-in live providers |
| 39 Lint | Implemented | Minimal ESLint bug/hooks rules; no formatting-rule expansion |
| 40 Final validation | Implemented | Complete command results below; external interaction limits remain explicit |
| 41 Final audit | Implemented | Starting/final comparison, every phase classified, blockers and next steps explicit |

## Parcialmente concluído

- Codex and Claude use exact bound vendor-native continuation, one owned process per turn, with typed host interaction. Codex supports ordinary approvals and non-secret native questions; Claude supports reviewable built-in tool approvals. Claude AskUserQuestion, secret input, MCP and unsupported tools are explicitly denied. OpenCode uses exact native ACP continuation and reviewable once-only file approvals. Its final hardened-policy edit/follow-up/refusal/cancel success remains unverified: the same final environment/model fails with APIError in both CLI run and ACP. That establishes a shared assistant API failure, not its cause, quota, entitlement or outage. Cursor/Grok retain bounded textual fallback, not exact native continuation.
- Claude initialization-only discovery returns the real selectable CLI IDs and filters account metadata. Documented aliases remain an unknown fallback when initialization fails. Catalog presence for any provider does not prove every model is runnable or entitlement.
- Grok 1.0.46 official temporary binary returned model rows without authentication; parser marks those unavailable. Official upstream streaming envelopes are normalized by fixture tests. Authenticated inference and official logo retrieval remain blocked.
- Drafts persist through native owner-validated JSON and graceful restart without being sent. Edit-driven leading checkpoints can leave admitted edits unsaved while idle after a sudden kill. Terminal closes with its panel rather than becoming detached.
- Unsupported optional Settings remain Coming soon/disabled: updater (no configured signed release endpoint), Git auto-fetch (requires config-isolated networking/ref import), and unspecified experiments. Font, custom shortcuts, developer diagnostics and reopen opt-out are functional. Close confirmation passed actual macOS Quit decline/repeat/accept with a real running agent.
- The gallery is complete for the free kit, with real controls and explicit local samples. It is not a claim that every chart/business form is a meaningful coding product feature. Its approximately 742 kB lazy JavaScript chunk triggers a build warning while staying outside startup/Settings navigation.
- Atomic JSON/event-driven checkpoints reduce loss but are not a crash-proof journal. A sudden kill can lose recent output. Windows/Linux desktop/provider/PTY behavior remains unverified; macOS is the validated host.
- POSIX PID/session termination has a check/use race; detached sessions are excluded. Preview descriptor-path revalidation is implemented on macOS/Linux, with weaker untested Windows guarantees.

## Bloqueado

| Item | Concrete condition | Action taken |
| --- | --- | --- |
| Grok authenticated runtime | Official temporary binary reports unauthenticated; vendor login would require owner credentials | Verified version/help/catalog/upstream schema without inference; no login/config changes |
| Grok official icon | [Official archive](https://data.x.ai/logos/SpaceXAI_Grok_Assets.zip) returned HTTP 403; [guidelines](https://x.ai/legal/brand-guidelines) require the provided unaltered logos | Neutral placeholder; no X/logo invention or runtime hotlink |
| Cursor manual host approvals | No verified machine-response protocol in installed CLI/help | Safe Auto-review edits pass; adapter reports manual interaction limitation, no force/yolo |
| OpenCode final hardened-policy live acceptance | Both final-env CLI run and ACP prompt return APIError; initialize/new/load/config and merged permission assertions pass | Preserve strict approvals; no silent fallback, auth changes or invented success |
| Safe Git auto-fetch | Existing Git config can rewrite URLs, invoke helpers and apply sensitive headers; direct fetch is not config-isolated | Control remains disabled; manual terminal is explicit. A separate secure object/ref-import design is required |
| Automatic updater | No signed release endpoint/key and publication is excluded | Disabled, no fabricated releases |
| Local Git checkpoints | Supplied folder has no `.git` metadata | Worked in place; no repository initialization or remote operations |

All validation projects, worktrees and process markers are owned disposable fixtures. User repositories/state were not used for provider prompts. One CUA observation after quitting auto-relaunched the validation bundle without its test environment; that idle instance was closed without prompts or preference changes, then explicitly rebound to the running fixture instance. No production-state non-use claim is made for that brief launch. Credentials, login and vendor configuration files were not changed. No cloud, mobile/remote implementation or publishing was introduced.

## Bugs encontrados e corrigidos

1. Stop waited behind a locked child wait; cancel could not interrupt execution or descendant pipes.
2. Renderer callbacks owned native completion/output, and unknown promise rejections destroyed React.
3. Starting/admission/removal/cancel races admitted superseded processes; late exits overwrote newer or interrupted states.
4. Reader abort was not joined, output/final snapshots reordered, and replayed first chunks could duplicate or resurrect finished messages.
5. Stderr warnings entered assistant prose and follow-up prompts; native System messages now remain separate and collapsed.
6. Protocol failure with zero process exit looked successful; stream line/message limits and recovery streaming flags were missing.
7. Invalid/corrupt state could be replaced with defaults; active states survived restart without live processes.
8. Metadata disappeared before worktree removal succeeded, dirty/unconfirmed removal paths were unsafe, and same-title branches collided.
9. Diff traversal/symlink escapes, staged deletions, nested Git roots, literal backslashes and NUL filenames were mishandled.
10. Large tracked/deleted/index blobs bypassed preview limits; directory/FIFO previews could block. Descriptor-backed regular-file checks and regression cases now bound reads.
11. Git hooks/fsmonitor/external filters could execute project commands during automatic inspection/checkout. Hook/fsmonitor overrides and conservative filter refusal prevent those paths without corrupting content semantics.
12. Read-only Git/filesystem and worktree I/O blocked the native event thread; native worker wrappers now preserve UI responsiveness.
13. Old Git/diff/project responses replaced newer selections; rapid preference saves dropped favorites or restored stale settings.
14. PTY listeners/start/exit/unmount/reopen races lost output or closed the next terminal; generation IDs and per-Session queues preserve ownership. UTF-8 split reads corrupted characters.
15. Closing a terminal left shells ignoring HUP or normal separate-group background jobs running. Owned PTY-session cleanup now covers those jobs and preserves unrelated processes.
16. Exited CLI probe leaders left descendants holding pipes indefinitely. Capture keeps ownership until drain, bounds time/output and kills/reaps only its own group on failure.
17. Arc CSS loaded before Tailwind resets, erasing padding/borders. Explicit HTML layer ordering fixed the actually observed modal defect.
18. Arc modal gallery menus escaped the focus/pointer trap; inline containment/Escape ordering fixed them. Inline menu bubbling suppressed the next trigger click; scoped review caught and fixed it.
19. BottomSheet global pointer handlers could outlive unmount; local pointer capture replaces them. Switch physical border caused thumb overshoot; inset border preserves geometry.
20. Error toast used success semantics and timed dismissal; error role/icon/color and explicit dismissal now match failure.
21. macOS local bundle retained an invalid resource signature; explicit debug ad hoc signing produces a verifiable artifact.
22. Draft workspace context named the shared checkout before creating an isolated worktree. Drafts now state only the requested destination; actual Session snapshots use native worktree metadata.
23. Initialization/catalog reads lost partial NDJSON when stderr completed; persistent buffers and bounded joined readers fix cancellation and completed-task repolling.
24. Codex native reads dropped partial JSON when a reply interrupted select; persistent frames fix the race. Empty output identities bypassed byte limits; count/key-byte bounds now cap retained state.
25. Settings Shift+Tab reached the hidden terminal; existing Radix Dialog focus containment and restoration fix the actual desktop reproduction.
26. Context tabs clipped at minimum width; compact labeled icons keep all five controls accessible at 260 px.
27. Fallback provider leaders exited while descendant tools kept pipes open; unreaped owned-group cleanup now kills descendants before draining/unregistering/reaping. An unrelated process survives the regression fixture.
28. Selecting an unchanged provider/model silently discarded native continuation; no-op selection now preserves identity and existing model, verified through native regression and final desktop selection.
29. Late native stderr failure was overwritten as Stopped; shared reader results preserve Failed and its diagnostic.
30. A loading catalog was labeled unavailable, and empty searches were mislabeled; distinct localized loading/no-match states now show the actual condition.

31. UI font preference changed the root font while fixed-pixel text stayed unchanged; central typography scale now changes product text independently of terminal size.
32. Invalid/conflicting custom bindings could steal system/editing keys; frontend and native validators now agree and hints use the effective saved binding.
33. Git 2.54 configured hooks bypassed hooksPath suppression; preflight rejects effective hook configuration. Final review reproduced a GIT_CONFIG-only inspector mismatch; the final fix aligns and bounds inspectors.
34. Settings search lacked Arrow/Enter result navigation. Native desktop also exposed Escape capture dismissing Settings before search/recorder cancellation; final host arbitration passed actual clear-then-close and recording cancellation.
35. OpenCode deprecated mode.build permission conversion could override child policy; global/build/deprecated-mode policies now agree. Ambiguous offered approval IDs are rejected and upstream diagnostics retain only safe codes/classes.

36. macOS predefined Quit called NSApp terminate and bypassed ExitRequested; a custom native Quit routes through the existing guard. Parented native-sheet cancellation/repeat/acceptance passed with a real running Claude process.

## Decisões arquiteturais

- Preserve Tauri/Rust/React/Vite/Tailwind/Radix/Motion/Zustand and UI→Client→Transport→Rust. Only LocalTransport imports Tauri; no arbitrary command, renderer completion authority or frontend filesystem/shell plugin.
- [Native Session ownership](decisions/ADR-008-native-session-lifecycle.md): native message IDs/offsets, separate diagnostics, bounded output/checkpoints and lifecycle are authoritative.
- Provider identity remains distinct from model and Session; model keys include provider, disablement applies to new selection, capabilities/notes are centralized.
- [Codex native interaction](decisions/ADR-009-native-codex-interaction.md) and [Claude native interaction](decisions/ADR-010-native-claude-interaction.md): exact IDs bound to Session/project/workspace/model, native-retained original inputs, typed per-generation callback ledger, no arbitrary protocol writer or persistent grants.
- [OpenCode ACP](decisions/ADR-011-native-opencode-acp.md): exact bound load, native-owned config negotiation and one-shot offered file approval IDs; child-only policy overrides never change vendor configuration files. ACP is not an OS filesystem sandbox.
- Preferences stay in the existing JSON/Zustand domain; diagnostics contains fixed aggregate counts only (two private 256 KiB files). Reopening means automatic selection of the newest-created saved Session, never starting a process.
- [Arc integration](development/ui-arc.md) keeps licensed upstream source behind host tokens/primitives, visible focus and reduced motion. Optional examples remain lazy and local.
- Native Git uses argv and safe worktree checks; external filters are refused rather than silently disabled. Tests use owned Git repositories and subprocesses only.
- Reuse atomic JSON and Node's runner; no second store, database, framework migration or paid infrastructure. Read-only IPC work moves to workers without expanding renderer authority.
- Local build signing is an explicit debug override; product identifier/data identity stays `com.siruscode.app` despite Tauri's suffix warning. No release certificate/notarization workflow is added.

## Validation evidence

| Check | Result |
| --- | --- |
| TypeScript application + test typecheck | Passed on final code |
| ESLint bugs/hooks | Passed on final code |
| Node domain/race/context/transcript tests | **36 passed, 0 failed** |
| All free Arc real component render/coverage smoke | **100/100 passed** |
| Rust regression/unit suite | **84 passed, 0 failed, 13 opt-in live tests excluded by default** |
| Rust Clippy all targets, warnings denied | Passed |
| Rust fmt/check | Passed |
| Frontend and ad hoc desktop debug build | Passed on final code; local macOS app **46.62 MiB**, main JS approximately **329 kB** |
| Local bundle `codesign --verify --deep --strict` | Passed on final artifact; isolated-data launch stayed alive, then only its owned process was stopped/reaped |
| Live Codex native edit/exact follow-up + typed approval decline | **2 passed, 19.28 s** on final native code; prior explicit discovered-model smoke also passed |
| Live Claude native Write allow/exact follow-up + Write decline | **2 passed, 10.64 s** on final native code |
| Live OpenCode native ACP | Initial-policy edit, exact cross-process follow-up, refusal and cancellation passed. Final hardened-policy suite unverified due shared APIError; older single-shot 11.84 s evidence is historical |
| Live Cursor native workspace edit | Passed **16.03 s** on final native code, Auto-review + sandbox enabled |
| Actual isolated desktop Codex Session | Completed, no error, worktree file exact, original checkout unchanged |
| Actual desktop Claude | Write allowed, exact file and unchanged original checkout; exact follow-up and post-restart continuation; same-model reselection preserved native identity; declined second Write created no file |
| Actual desktop PTY/Git | Correct cwd, input, keyboard resize, close/reopen/exit/reopen, changed-file selection and real preview |
| Actual desktop preferences/drafts | Graceful restart preserved language/sidebar/favorite/custom binding and unsent text; reopen opt-out selects no Session but history/draft remain. Actual 12/14 font changes and conflict/reserved binding refusal passed |
| Actual desktop keyboard creation | Cmd+N, title, model search/arrows/Enter, workspace radios and create selected actual Codex gpt-6.1-sol plus isolated worktree; saved idle with zero messages |
| Actual desktop Settings Escape | Expanded unmatched search clears while staying open; second Escape closes. Focused recorder cancellation retains Settings and original shortcut. Reopen aliases find the correct row |
| Actual desktop close/Quit | Running Claude → Cmd+Q sheet; Keep running preserves live output. Repeated Cmd+Q retains one sheet; accept exits zero, persists Stopped and partial output, retains draft, and leaves no validation app/fixture CLI process |
| Actual desktop diagnostics | Toggle persisted; private 0600 aggregate-only journal written; native folder opening showed the real log file; disable stopped logging |
| Actual desktop focus/layout/gallery | Settings reverse-tab stayed inside active scope; five tabs fit 260 px; Arc nested dropdown keyboard selection/Escape/focus return passed |
| Live Claude initialization-only catalog | Passed **1.90 s**, real selectable IDs, no inference/account fields returned |
| Incremental Graphify | **3,171 nodes, 6,677 edges, 143 communities**; graph/report refreshed without paid semantic extraction |
| Production dependency audit | 0 reported vulnerabilities |
| Local documentation links | Passed |
| Independent source reviews | Catalog partial-read, Codex framing/bounds and final lifecycle/no-op identity/diagnostics findings fixed; independent scoped re-reviews passed; final Git environment/bounds, Settings Escape, macOS menu and sheet ownership also passed |

No check above proves every provider model, permission path, desktop gesture or cross-platform runtime. The remaining build warnings are identifier identity and the optional full-gallery lazy chunk; they are documented trade-offs, not hidden errors.

## Próximos passos recomendados

1. Broaden targeted macOS keyboard/gallery coverage to every main flow and validate Windows/Linux separately.
2. Exercise Grok after owner-managed CLI login and retrieve the exact official asset when the archive becomes accessible.
3. Repeat the existing opt-in OpenCode ACP hardened-policy suite when vendor inference works; preserve exact IDs/native stdin/once approvals and inspect the actual error rather than changing authentication.
4. Consider Session filters/export and explicit provider handoff drafts from the complementary backlog; never auto-send restored content.
5. Split the optional gallery by component family if first gallery load becomes a measured issue; avoid adding startup cost.

Official competitive evidence and implementation status: [FEATURE_BACKLOG.md](FEATURE_BACKLOG.md). Architecture/limits: [runtime guide](development/runtime.md). Exact local commands: [build guide](development/building.md).

## Composer refinement (2026-10-01)

- Single composer surface with an unframed Arc textarea, subtle outer keyboard focus, compact toolbar, context control and send/stop buttons following the supplied reference design.
- One model picker shared by composer and new-session dialog, using the chosen integrated icon rail. Hover/focus browses only the corresponding provider's models without changing selection. The selected trigger shows only icon and model name, with no provider label, separator or synthetic CLI-default control. Native null defaults remain unchanged until explicit selection.
- Global favorites retain exact provider-qualified IDs and filter unavailable, disabled or uninstalled providers/models. Cached/coalesced discovery runs only for the browsed provider or providers owning saved favorites; search receives focus and arrows navigate both rail and model list.
- Cursor presets display one clean model name; offered IDs, current selection, variant enablement and favorites remain authoritative. Settings retains its full catalog. Presentation policy: [UI Arc integration](development/ui-arc.md).
- Validation: typecheck, lint, **42 Node tests**, **100 Arc previews**, all four interactive HTML previews, Rust check, frontend build and ad hoc signed macOS desktop build passed. Graphify updated to **3,179 nodes / 6,706 edges**.
- Disposable desktop checks passed with real Cursor, Claude and Codex catalogs: browsing retained current selection; favorite/search/Down/Enter selected native `claude::claude-sonnet-5`; the trigger displayed only icon + Sonnet 5. In New Session, Home/Down/Right/End/Enter selected Codex GPT-6.1-Sol; first Escape closed only the popup and second closed the dialog. No inference was requested; zero sessions were created, draft retained, exact preference IDs persisted and the validation app exited normally.
- The user's app was reopened with its saved conversation intact and the integrated selector visible; its existing null Codex model remained null.

## Composer execution integration (2026-10-01)

### Concluído

- The selected Orbit design is implemented in the app. One model-family icon/name trigger sits beside Send/Stop. Its single popup switches between effort/Fast controls and the scoped provider/model catalog; selection returns to effort controls. No CLI-default row or Shared checkout control remains in the composer. New Session retains actual workspace/worktree choice.
- Normal animation keeps the approved luminous cloud, particles and flare. Fast adds branched traveling lightning, driven by small SVG stroke/transform layers. Model family controls the color even through Cursor. Hidden/offscreen/closed panels and reduced motion pause decorative motion; label changes use Motion.
- Real per-turn execution preferences: discovered Codex reasoning levels and priority/default tier; Claude catalog levels and process-only Fast preference with initialization confirmation; Cursor exact offered effort/Fast presets. Preferences retain qualified IDs and are saved. Native validation uses a catalog tagged with the executable override and rejects unknown/unsupported options.
- Real planning mapping for Codex (read-only turn plus plan preset), Claude (plan permission mode) and Cursor (plan argv). Returning to regular mode explicitly clears native plan/Fast preferences where the protocol retains them. Unsupported adapters are disabled.
- Composer menus measure their trigger offset once on opening and appear above the complete input surface while retaining the horizontal anchor. The Add panel has exactly Files and folders, Goal and Planning mode. A native OS picker supplies regular UTF-8 snapshots or shallow folder inventories; removable chips are owner-scoped. Goal reaches the submitted task. Failed sends/session creation retain captured context, and late completion cannot clear newer edits or another owner's context. Rapid preset changes are serialized per Session; Send waits for confirmed native model admission, and old native thread identity is cleared in the renderer when the native selection changes.
- Security review: no frontend Tauri/fs/shell imports, no arbitrary path reader/config/protocol passthrough, no permission bypass. Native attachment selection has one-dialog admission, owner/closing checks, eight-selection/12 KiB limits, bounded nonrecursive folder names, nonblocking/no-follow regular-file checks and binary rejection. Approval UI describes fixed native policies; full access is disabled. Account configuration is unchanged.
- Architecture and limitations are recorded in [ADR-012](decisions/ADR-012-composer-execution-and-attachments.md), AGENTS.md and the UI integration guide. All five standalone comparisons remain illustrative; Orbit previews now use lightning in Fast mode too.

### Parcialmente concluído

- Unsent references/goal/planning are memory-only; closing the app preserves the text draft but clears this additional context. UTF-8 files are supported; images/binaries are explicitly rejected. Folder references contain names from one level, not recursively ingested contents. Sent reference text is persisted with the user message.
- Real provider option wiring is validated against installed CLI metadata/schema/settings and native tests; fresh paid inference was not requested for these options. Earlier live agent-loop evidence in this document predates the new composer options.

### Bloqueado

- Local Claude initialization reports `fast_mode_disabled_reason: extra_usage_disabled`. Fast stays disabled for this account; no billing/extra-usage preference was changed. It is only offered when native initialization reports availability for a supporting model, and startup must confirm it before input.
- This checkpoint's OpenCode/Grok control limitations are superseded by the provider capability correction below. Full access remains unavailable by the existing security contract.
- Final desktop visual/native-picker gesture review is blocked by `Sky Computer Use native pipe startup failed` (native inventory unavailable). An isolated-data application launched successfully; this is not claimed as visual verification. Production React DOM interactions and native snapshot fixtures are verified separately. The user's running app/data have not been killed or overwritten to work around the automation failure.

### Validação

- TypeScript application/test typecheck and ESLint passed.
- **49 Node tests**, **89 Rust tests** and **100 Arc render/coverage checks** passed; **13 live Rust cases remain opt-in**.
- Rust Clippy (all targets, warnings denied), fmt and check passed.
- Production React DOM harness passed single popup, real typed send payload, qualified model IDs, rail/search keyboard navigation, unsupported Fast, lightning presence, reduced motion, picker boundary, goal/planning, removable context, disabled full access and cleanup. Its OS picker calls use controlled fixtures, not an observed OS dialog.
- All six HTML comparisons passed existing normal/Fast/lifecycle, Add-panel and unified/control interaction regressions.
- Incremental Graphify refreshed the code map to **3,247 nodes / 6,913 edges / 149 communities**, without paid semantic extraction. The final frontend and ad hoc signed macOS desktop build passed; bundle size is **47.16 MiB**. `codesign --verify --deep --strict` passed on this final artifact. Remaining warnings concern the unchanged bundle identifier and optional gallery chunk.

### Próximos passos recomendados

1. Repeat native desktop/picker gesture and animation review when the computer-use bridge is available.
2. Exercise supported execution options through the existing opt-in provider tests in disposable workspaces; never enable paid account features as a test prerequisite.
3. Add image or recursive-folder context only with an explicit bounded ingestion contract and review of provider support.


## Composer capability correction (2026-10-01)

This checkpoint supersedes the execution-control limitations of the preceding composer checkpoint. Official references and the per-adapter matrix live in the [provider execution guide](development/provider-execution.md).

### Concluído

- Add and approval menus span the measured composer width and align both input edges above it, overriding Arc's shared 22rem width cap. Approval rows have icons and descriptions. The model effort panel is 300 px wide and centered over its trigger; catalog navigation remains in the same 360 px popup.
- Cursor now discovers native per-model parameters through initialization-only ACP alongside exact legacy presets. New selections prefer parameterized base models while existing Sessions retain their exact preset identity. Effort raw values are retained only in the native cache, including the `extra-high` → UI `xhigh` mapping. Known bracket parameters become one argv value; the persisted Session keeps its base model ID. Renderer brackets, option-like IDs and injected parameter values are rejected.
- Composer 2.5 offers a real independent Fast button using its discovered boolean/default. Its native catalog exposes no manual effort selector; automatic effort has decorative normal/Fast energy without a pretend disabled slider. Manual effort uses only each model's actual offered levels.
- OpenCode discovers known actual reasoning variants from its verbose catalog, selects and confirms the native ACP reasoning option before prompt submission, and clears retained reasoning through the offered default on exact resume. Planning selects/confirms the offered plan mode and denies edits at global/build/plan/deprecated mode layers in child-only configuration. Malformed configuration returns an error instead of panicking.
- Grok effort wiring requires the detected CLI's `--effort` flag, a discovered model and the documented bounded model table. Unknown models receive no invented effort levels. Its live runtime remains unverified as detailed below.
- Claude Fast exposes a sanitized account/organization/model restriction explanation. No usage-credit, authentication or organization preference was changed. Approval choices remain truthful about each adapter's fixed policy; full access is disabled.

### Parcialmente concluído

- Catalog/parameter negotiation and typed option payloads are verified; no fresh paid inference was requested. Existing provider inference evidence earlier in this document does not constitute a fresh test of every new execution option.
- OpenCode ACP configuration negotiation passed with disposable XDG directories and an offered free model, without `session/prompt`; its hardened-policy inference still has the separately documented upstream APIError limitation.

### Bloqueado

- Grok is absent from the local PATH and has no configured executable override; documented effort wiring cannot be claimed as live CLI execution.
- Claude Fast remains unavailable because native initialization reports `extra_usage_disabled` on this account. The corresponding explanation is visible on the lightning control.
- A fresh computer-use inventory returned `Sky Computer Use native pipe startup failed`. Native visual/OS-picker gesture verification and reloading the user's running app are not claimed. Production React DOM interactions use controlled metadata/picker fixtures; these do not replace an observed native picker or visual animation review.

### Bugs encontrados e corrigidos

- Shared Arc max-width silently capped the supposedly wide menus.
- Legacy-only Cursor catalogs hid Composer Fast and missed native parameterized model options. Switching an existing legacy Fast preset to the parameterized base now preserves the explicitly chosen normal mode instead of restoring the provider's Fast default.
- A non-adjustable effort slider implied a control the CLI did not expose.
- The renderer could supply bracket syntax as a Cursor model ID; native argument construction now rejects it and uses only native-retained known mappings.
- OpenCode retained reasoning is explicitly cleared on default turns; malformed planning configuration is handled without indexing panics.

### Decisões arquiteturais

- Keep `ExecutionOptions` typed and bounded; no new arbitrary parameter/config/protocol IPC and no new dependencies. The native catalog remains authoritative and stamped with its executable override.
- Separate automatic reasoning, adjustable effort and priority/Fast service. Neither every reasoning model nor every CLI exposes all three controls.
- Preserve fixed native approvals and vendor account configuration. Document unsupported controls instead of simulating them.

### Validação

- Application/test typecheck, ESLint and **53 Node tests** passed. Native fmt/check, all-target Clippy with warnings denied and **94 Rust tests** passed; **14 live cases remain opt-in**.
- The initialization-only native Cursor catalog test passed in **4.72 s**, without session creation or inference. OpenCode native reasoning selection/confirmation was separately probed without a prompt, using disposable XDG directories.
- Production React DOM harness passed full-width Add/approval geometry, compact 300 px effort panel, automatic Composer/Fast payloads, legacy Fast-to-normal migration, qualified identity, keyboard/Escape/focus return, goal/planning/context, disabled full access and cleanup. Its picker and metadata are controlled fixtures. The harness waits for Radix's deferred focus restoration rather than asserting before lifecycle cleanup; native visual verification remains unclaimed.
- Final frontend and ad hoc signed macOS desktop build passed: **47.59 MiB**, with `codesign --verify --deep --strict` passing on the final bundle. Existing identifier/optional-gallery/notarization warnings remain documented; this is a local debug artifact, not a release.
- Incremental Graphify refreshed **3,274 nodes / 6,970 edges / 155 communities** without paid semantic extraction. No credential changes, vendor billing changes, release or push occurred.

### Próximos passos recomendados

1. Review actual desktop geometry, picker gestures and both animation modes when the computer-use bridge is available.
2. Exercise new options with the existing opt-in native provider tests in disposable workspaces when inference is available, without enabling paid account prerequisites.
3. Exercise Grok against the owner's installed/authenticated CLI before claiming live integration.


## Model panel positioning (2026-10-01)

- Effort and model catalog now anchor directly above the selected-model toolbar button with a 6 px gap, centered in the composer. Removed the composer-top offset from this selector; Add and approval menus retain their full-width placement.
- Application/test typecheck, lint, Rust check, production React interaction harness, frontend/desktop build and final bundle signature verification passed. Native visual verification remains blocked by the computer-use bridge described above.

## Composer placement and model ordering (2026-10-01)

- Lowered the empty composer group slightly with 48 px of top inset in its centered container; reduced the conversation composer's bottom inset from 20 to 8 px. Popovers still follow their actual trigger geometry.
- Removed the status sentence (including “Concluído”) from the footer while retaining its Terminal action. Native session state and sidebar indicators remain intact.
- Centralized descending numeric generation order for provider model menus and Settings: Opus 5.5 precedes Opus 5, including when the older model is favorited or selected. Favorites remain in their separate rail. Dotted and hyphenated versions work; unrelated unknown families, context capacities and date suffixes do not become a shared generation sequence. Unversioned models retain deterministic name ordering rather than invented release dates.
- Added regressions for numeric version comparison, input-catalog immutability, exact saved IDs, favorites, raw CLI names and capacity/date exclusions. **55 Node tests** passed, including the previously failing generation-order regression. Rust check passed without native code changes. Native visual verification remains blocked as documented above.
- Final application/test typecheck, lint, production React composer interaction harness, frontend/desktop build and strict bundle signature verification passed. Incremental Graphify refreshed **3,278 nodes / 6,976 edges / 151 communities** without paid semantic extraction. The local desktop artifact is **47.59 MiB**; existing optional-gallery, identifier and notarization warnings remain unchanged.

## Composer bottom anchor and CLI usage footer (2026-10-01)

### Concluído

- Composer now stays at the bottom in both empty and conversation states, 8 px above a 32 px footer with a subtle separator. Usage indicators/refresh/provider visibility are on the left, Terminal on the right.
- Real Codex and Claude Code account quota reads use their existing native CLIs, without a prompt, host token readers or new dependencies. Initialization-only verification returned two actual quota windows per CLI. Claude's experimental `get_usage` skips the transcript-behavior scan; no credentials are read or copied.
- Provider visibility is persisted; multiple providers can be shown together and the current provider remains visible. Refresh is explicit or lifecycle-driven with native caching, no polling/countdown timers. Popovers show all returned windows, reset times and last-read timestamps.
- Earned Codex reset UI requires explicit confirmation and a real short-lived native offer. Rust retains/revalidates the named credit, reserves it once, and supplies the UUID idempotency key. Post-reset quota is read from the CLI, never inferred as zero. No purchase, overage, account setup or automatic redemption is present.

### Parcialmente concluído

- Reset parsing, confirmation/cancel behavior, once-only offers, argument shape, denied protocol callbacks and post-reset reads are validated with disposable native fixtures and production React DOM interactions. A real account reset was deliberately not consumed during validation; live redemption is not claimed.

### Bloqueado

- Cursor/OpenCode/Grok adapters have no verified credential-free account-quota CLI protocol; their panels say quota is unavailable rather than fabricating percentages.
- Fresh desktop inventory still returns `Sky Computer Use native pipe startup failed`. Actual desktop geometry/gestures and reloading the running user app are not claimed. DOM fixtures do not replace that visual review.

### Bugs encontrados e corrigidos

- Switching executables during a read could discard the new read along with the stale one; replacement probes now queue under the new executable identity.
- The new reset Cancel action initially missed the existing translation key; the production interaction harness caught it and now passes the Portuguese action.
- Repeated/stale/foreign reset attempts are rejected; failed attempts retire the renderer offer without automatically redeeming again or guessing new quota.
- Closing Sirus cancels probe work and drops owned process groups. Confirmed reset outcomes are retained even if the subsequent quota refresh fails.

### Decisões arquiteturais

- Two fixed Client/Transport commands, normalized bounded quota metadata, one native reset offer and no renderer-selected RPC methods, credit IDs, binaries or credentials. Cache/offer state is memory-only; visibility uses the existing settings persistence. [ADR-013](decisions/ADR-013-provider-usage-and-earned-resets.md) and the [usage guide](development/provider-usage.md) record verified versions and limitations.
- Security review covered argument boundaries, CLI callback denial, metadata redaction, output/deadline bounds, shutdown cleanup, executable-stamped cache, native credit identity/expiry and confirmation/replay admission. Tauri capabilities/CSP did not change.

### Validação

- **62 Node tests** and **100 Rust tests** passed; **15 native live tests remain opt-in**. The added read-only usage test passed separately in **2.79 s**, using Codex 0.159.3 and Claude Code 2.1.286 without inference or real reset consumption.
- Production React composer and usage-footer harnesses passed: multiple quota chips, persisted visibility, refresh, confirmation/cancel, one retained offer, unavailable adapter, Escape/focus return and Terminal. Metadata/reset responses in these interaction tests are controlled fixtures.
- Final application/test typecheck, lint, Rust check, fmt check and Clippy with warnings denied passed. Frontend and desktop builds passed; the signed local app is **48.24 MiB**, and strict bundle signature verification passed. Existing optional-gallery chunk, bundle identifier and local notarization warnings remain unchanged.
- Incremental Graphify refreshed **3,366 nodes / 7,217 edges / 153 communities** without paid semantic extraction. Agent exits refresh the current visible provider even when it is not separately pinned.

### Próximos passos recomendados

1. Review native desktop geometry and pointer/keyboard gestures when the computer-use bridge works.
2. Exercise earned-reset redemption only through the user's explicit confirmation when a real named credit is available.
3. Add further provider quota adapters only when a verified protocol avoids host credential readers and account-management changes.

## Usage popup identity, motion and further provider research (2026-10-01)

### Concluído

- Footer chips show their provider names. Details distinguish the current session/default provider from other monitored providers.
- Each details panel includes “Add provider,” sharing the existing persisted visibility checklist with the footer. The current provider remains visible; no agent/model/account is switched by monitoring another provider.
- Usage panels enter with opacity/scale/trigger-relative translation over the existing normal token and leave faster over the fast token. Real exit keyframes retain Radix presence; reduced motion uses opacity only. No new dependencies, global listeners or polling.

### Parcialmente concluído

- Further quota research is recorded in the [usage guide](development/provider-usage.md). OpenCode Go's official authenticated endpoint returns rolling/weekly/monthly subscription limits. Installed OpenCode stats reports local token/cost history, not subscription quota. Cursor's installed JSON status/about exposes account metadata, not quota; independent usage integrations rely on authenticated dashboard APIs.

### Bloqueado

- Existing credential access for those API integrations requires an explicit exception to the current AGENTS/ADR-013 no-host-credential-reader contract. The user was asked through the asynchronous input tool; no answer has arrived. No credential reader, token access or fake quota was added while authorization is pending.

### Validação

- Application/test typecheck, lint, **62 Node tests**, Rust check and frontend/desktop build passed. Strict signature verification passed for the **48.24 MiB** local bundle. No native implementation changed in this checkpoint.
- Production React DOM interaction harness passed, including provider identity, an in-panel visibility checklist, refresh, reset confirmation/cancel, Escape/focus return and Terminal. This does not prove native visual appearance or real reset redemption.
- Incremental Graphify refreshed **3,367 nodes / 7,220 edges / 151 communities**, without paid semantic extraction.

### Próximos passos recomendados

1. Resolve credential-access authorization before implementing authenticated Cursor/OpenCode quota adapters; keep tokens native-only and queries read-only if approved.
2. Verify motion and positioning in the real desktop when computer-use access works.

## Authenticated Cursor / OpenCode Go quotas and account identity (2026-10-01)

### Concluído

- Owner authorization resolved the preceding credential-reader blocker. Cursor and OpenCode Go now perform real read-only native quota requests using their existing CLI credentials, without inference, login/config changes or prompt execution. Cursor returned two pools; Go returned rolling 5-hour, weekly and monthly windows.
- Every quota popup identifies the account behind its snapshot: actual email and plan from Codex/Claude/Cursor, or the connected Go key's 12-hex SHA-256 fingerprint. Go does not expose email; none is fabricated. Existing motion, provider identity and the shared in-panel “Add provider” checklist remain.
- HTTP credential identity is checked before cache reuse and after requests. A changed credential/executable discards the old result. No raw credential/response reaches IPC, logs or persistence; normalized account metadata is memory-only.
- Earned Codex reset offers require a known CLI account email and reject a fresh mismatched or missing identity before credit consumption. Confirmation, expiry, named-credit checks, once-only reservation and idempotency remain.

### Parcialmente concluído

- OpenCode support covers the Go subscription. Other OpenCode backends have separate vendor/account quotas; local token/cost stats are not substituted.
- Real reset credits were not consumed. Reset confirmation and account-change rejection are covered with controlled fixtures and native checks.

### Bloqueado

- Grok has no verified quota protocol in this adapter. Unsupported/expired credentials and incompatible server schemas remain explicit errors/unavailable states.
- Native desktop visual review remains unverified because the computer-use bridge previously failed to initialize. Production React DOM checks do not prove native geometry or a running app reload.

### Bugs encontrados e corrigidos

- Cursor's fractional values are already percentages; 0.36 stays 0.36%, and absent values never become zero. Two pools are preserved separately.
- Go's monthly window now takes priority in the footer without inventing a fixed 30-day duration. Reset dates come from the server.
- Failed snapshot reads clear stale account labels. Native cache hits and in-flight results cannot reuse another HTTP credential's snapshot.
- Codex reset offers can no longer cross a CLI account change merely because the executable is unchanged.

### Decisões arquiteturais

- [ADR-014](decisions/ADR-014-scoped-authenticated-quota-reads.md) records the owner-authorized policy exception: one exact Cursor CLI access-token source, one Go auth namespace, three fixed HTTPS GET endpoints, no broad credential discovery or account management. ADR-013 reset lifecycle rules remain.
- Security review covered sensitive headers, TLS/deadlines, disabled redirects/retries/proxies, bounded bodies/files, regular-file/current-owner checks, final-symlink refusal on Unix, secret-free serialization/logging, cache identity and shutdown cancellation. Tauri IPC allowlist/capabilities/CSP are unchanged.
- Explicit reqwest/base64/SHA-256 dependencies provide native TLS transport, bounded JWT parsing and credential fingerprints; the crates already existed in the lockfile. No renderer networking library or token-bearing process argv was added.

### Validação

- Application/test typecheck, lint, Rust check, fmt check and Clippy with warnings denied passed. **64 Node tests / 104 Rust tests** passed; **16 live tests are opt-in**.
- Read-only native CLI probes passed against existing Codex/Claude accounts (two real windows and actual email each); HTTP probes passed against Cursor/Go (two/three windows, actual Cursor email and Go key identification). No account values or secrets were printed and no reset credit was consumed.
- Production React usage-footer interactions passed with four provider quota fixtures, email/key identity, multiple pinned providers, refresh, confirmation/cancel, one retained reset offer, unavailable Grok, Escape/focus return and Terminal. The composer harness also passed.
- Incremental Graphify refreshed **3,439 nodes / 7,374 edges / 152 communities** using local AST extraction, without paid semantic extraction. Frontend/desktop builds and strict bundle signature verification passed. The local ad hoc signed app is **60.32 MiB**. Existing optional-gallery chunk, bundle identifier and local notarization warnings remain unchanged.

### Próximos passos recomendados

1. Review the actual desktop popup and motion when computer-use access is available.
2. Keep private Cursor dashboard schema and experimental Claude control failures explicit; do not add alternate-account credential fallbacks.
3. Add other OpenCode backend quotas only with a verified vendor protocol and a separately scoped credential policy.

## Provider account profiles (2026-10-01)

### Completed

- The usage popup now opens **Add account** for the same provider, with default/named account rows, verified CLI email/plan/quota, selected new-session default, browser sign-in, cancellation and name management. The separate footer visibility control continues to choose monitored providers.
- Codex/Claude profiles use private native-generated directories and fixed vendor login. CLI credentials remain vendor-owned; no token input, copies, imports or raw auth output enter Sirus IPC/persistence/logs. Default login is preserved. Registered metadata and new-session selection persist atomically.
- Session admission and exact continuation bind the profile. Switching providers and returning retains that session's original per-provider account. Changing the selected account applies to future sessions, not existing ones. Agent processes, quota probes and Codex reset offers receive the same native profile.
- Native paths reject foreign/unknown IDs, traversal, directory symlinks and unsafe ownership/permissions. Missing profiles fail explicitly. Used profiles cannot be overwritten by in-app re-login. Login has bounded discarded output, a ten-minute deadline, cancellation and synchronous owned-group shutdown; spawn/PID registration shares the shutdown admission lock.
- README, AGENTS and the short usage guide now describe real accounts. [ADR-015](decisions/ADR-015-isolated-provider-accounts.md) records the owner-authorized new-profile login exception; existing scoped Cursor/Go reads and earned-reset policy remain.

### Partially completed

- Multiple accounts are supported for Codex/Claude. Cursor/OpenCode still use their actual default accounts and real existing quota adapters; isolated multi-account login is unavailable until a verified CLI flow exists. No fabricated account support.
- Account management currently renames profiles. No deletion, logout, credential import or default-profile sign-in is offered.
- Session binding is to a CLI profile, not an immutable vendor account ID. External vendor tools can replace profile credentials, especially the shared default. Actual last-read email identifies the popup snapshot; reset redemption additionally checks fresh email.

### Blocked / not exercised

- Real browser OAuth requires the user's participation and was not completed during automated validation. Tests never signed into a new vendor account, changed default credentials or consumed a reset credit.
- Native desktop geometry/motion remains unverified because the computer-use bridge previously failed. Production React DOM checks are functional evidence, not native screenshots.

### Bugs found and fixed

- The former in-panel provider-visibility action did not add accounts. It now opens the actual account workflow.
- New account selection could otherwise affect existing session ownership. Native persisted profile bindings and per-provider return bindings prevent it; legacy sessions retain default.
- Late quota responses from old account/executable selections are discarded in the store. Native profile versions also reject probes started before an overlapping login, including after the login has finished.
- Quota-only protocol failures previously erased independently confirmed CLI identity. Identity is retained with unavailable quota, allowing a verified profile to remain selectable.
- Async cancellation alone could leave vendor login running during Tauri's immediate exit. Shutdown now kills owned login groups synchronously; spawn registration cannot race past shutdown.

### Architectural decisions

- Five fixed account commands stay behind Client/Transport; no renderer-selected paths, binary, argv, URL, headers or credentials. Codex/Claude child-only environment chooses the registered profile and removes inherited token variables. No new capabilities, CSP authority, plugins or dependencies.
- Quota cache and reset offers are profile-scoped. Account snapshots remain memory-only and event/explicit-refresh driven. A fresh account email is still mandatory before consuming an earned reset.
- Independent code review found the shutdown and quota-identity bugs; both were fixed and re-reviewed with no remaining material findings.

### Validation

- Application/test typecheck and lint passed; **67 Node tests / 113 Rust tests** passed, **17 native live tests remain opt-in**. Rust fmt and Clippy with warnings denied passed.
- Read-only installed Codex/Claude probes returned two real quota windows each. Empty named profiles initialized without inheriting default account identity. No prompt/inference, login, credential copy or reset consumption occurred.
- Native regressions cover profile UUID/provider validation, private paths, token-free child environment, fixed login arguments/cancel, synchronous shutdown without an async turn, per-provider session return binding, profile-scoped offers, stale probe rejection and identity retained after quota-only failure.
- Production React usage/composer DOM checks validate add/login/select/rename with controlled fixtures, existing account identity/quota/reset visibility, keyboard/focus and Terminal. These fixtures do not claim real browser OAuth success.
- Frontend and local ad hoc signed desktop builds passed; strict deep signature verification passed. The bundle is **61.11 MiB**. Existing optional Arc-gallery chunk, bundle-identifier and local notarization warnings are unchanged. Reopen the built app to load the native commands.
- Graphify local AST update produced **3,525 nodes / 7,634 edges / 155 communities**, without paid extraction. Its parser still reports partial extraction of the existing TypeScript store; application/test TypeScript compilers pass.

### Recommended next steps

1. Complete a named account's official browser login through the UI and review its real desktop account row.
2. Verify isolated login/config semantics before offering named Cursor/OpenCode accounts; keep default-account quota support intact.
3. Consider stronger vendor-identity pinning against external reauthentication if providers expose a stable verified account ID.

## Sidebar topbar refinement (2026-10-01)

### Completed

- One persistent PanelLeft control now sits at the window's top edge, fully visible and to the right of the native green traffic light. It stays in the same position while the sidebar opens/closes; the collapsed session chip reserves its own space instead of overlapping window controls.
- Sirus branding and a working search icon form the next sidebar row. Navigation starts lower, with quieter New session styling. The existing project/session organization and real actions are preserved; no Recent menu or fabricated navigation feature was added.
- The sidebar control reuses IconButton, translated labels and the central customizable Command+B shortcut. Existing saved preference and fade/resize behavior remain. Native geometry uses central inset/height tokens aligned to the actual macOS window.
- Escape from the search palette returns focus to its initiating control. Radix still owns initial focus/trapping; opening another modal through a command does not steal its focus back to the sidebar.

### Validation

- Typecheck, lint and all **67 Node tests** passed. Frontend and signed desktop builds passed; strict deep signature verification passed. Native code/capabilities were unchanged.
- The computer-use bridge is available again. Actual desktop screenshots verified the expanded and collapsed control position against native traffic lights and the spacing of the session chip. Clicking collapse retained focus; Command+B reopened the sidebar. Search opened the real command palette, and Escape returned native focus to Buscar. The updated app was left open with its original project/session selected and sidebar expanded.
- Reviewed for duplicate controls, clipping during resize, native drag regions, existing tokens, translated labels and keyboard reachability. No data loss, authentication or native execution-policy changes were introduced.

## Compact sidebar and session Kanban (2026-10-01)

### Completed

- Reduced the persistent sidebar control to a real 24 px target with a 14 px icon. Explicit minimum-height/padding overrides keep Arc Button's default dimensions from enlarging it. The brand uses the existing 14 px token; navigation/project/session labels use 12 px, with 13 px navigation icons and lighter Lucide strokes.
- Moved Notes beside Search in the header. Notes remains disabled and explicitly labeled Coming soon; its placement does not claim a working notes feature. The existing Lucide package supplies all control icons without another dependency.
- Added an active Kanban menu and command-palette action. The lazy board groups the selected project's real sessions into queued, active, waiting, completed and interrupted columns. Native events drive the statuses; clicking a card opens its session, and the existing new-session flow is available. This is a lifecycle overview, not a task board with manual status changes.
- Navigation uses the existing Zustand store, with memory-only session/board selection. Board grouping preserves session objects and drafts, filters foreign projects and sorts recent activity first. No IPC or execution policy changed. New strings support Portuguese and English.

### Validation

- Application/test typecheck, lint, **70 Node tests**, Rust check, frontend build and signed desktop build passed. Strict deep signature verification passed. The optional Arc-gallery size and local bundle/notarization warnings remain unchanged.
- Actual desktop screenshots verified compact typography/icons, Notes beside Search, native topbar alignment and both sidebar states. Click and Command+B toggles worked. Kanban displayed the saved completed session in the correct column; selecting its card reopened the original transcript. Searching Kanban and pressing Enter opened the board through the command palette and returned focus to Search.
- Three added regressions cover all seven lifecycle states, project filtering/order without list mutation, and session opening without altering execution or unsent drafts. No prompts, credentials or user files were modified during these checks. The updated app remains open with the sidebar expanded and Kanban visible.

## Session header and compact composer (2026-10-01)

- The session title has no chip background and uses the session's actual provider asset. Kanban keeps its board icon; new drafts use their default provider. A quiet border separates the 40 px main header from the content, matching the existing footer divider. Expanded and collapsed title positions retain the native window-control clearance.
- The composer text area starts at 32 px and grows with wrapped text up to a CSS maximum of 180 px. Excess content scrolls inside the field; clearing the draft shrinks it again. Reduced padding/gaps make the empty composer approximately 110 px tall. Pre-paint sizing handles draft changes; a local, cleaned-up ResizeObserver handles width changes, and loaded fonts trigger remeasurement. No polling or new animation loop was added.
- Typecheck, lint, all **70 Node tests**, Rust check, frontend and signed desktop builds passed. Actual native screenshots verified the plain title, correct Claude icon, divider, three-line growth, twenty-line maximum, internal scrolling and shrink-back to the empty field. Only temporary unsent test text was entered and removed; no inference was submitted. The updated app remains open with the original session selected.

## Assistant footer, transcript forks and message pins (2026-10-01)

### Completed

- Settled assistant responses now end with compact Copy, GitFork and Pin controls plus their actual localized timestamp. Copy reuses Arc feedback; pin state is native-persisted and accessible as a toggle. Both languages include labels, tooltips, errors and empty states.
- The context panel includes pinned messages. Selecting a bookmark scrolls to and focuses its response; pins do not add model instructions. The native metadata acknowledgment queue preserves newer output and prevents old lifecycle snapshots from undoing a pin.
- A fork creates a new native-owned Session from the selected transcript prefix, preserving provider/model/account/execution preferences, with fresh message identities and a link back to the original boundary. Diagnostics, pins, pending callbacks and vendor identity are excluded. The first prompt receives the reconstructed context; subsequent successful native continuation uses the exact saved thread. Nothing is sent automatically.
- Git forks own a new branch/worktree at the source workspace's current HEAD. Native Git fixtures verify the source commit is used, dirty files are not copied, original files remain untouched, and origin/account/message metadata survive persistence. Non-Git checkout sharing is explicit. Live status/handles, foreign boundaries and oversized histories are refused before filesystem mutations.
- Reconstruction has a 32 KiB/512-message admission budget and never silently clips the imported prefix. Fork creation clones only that admitted prefix. Failed/interrupted native initialization cannot discard handoff context merely because a vendor identity arrived early: successful bootstrap records its thread, and provider/model changes clear that marker. A retry after partial acceptance may repeat quoted context.
- Fixed controlled-dialog focus returning to the page on Escape, and a store acknowledgment bug that left the previous provider's account/bootstrap metadata visible after a provider change. Architecture/security rationale: [ADR-016](decisions/ADR-016-transcript-forks-and-message-pins.md).

### Validation and limits

- Final application/test typecheck, lint, **73 Node tests**, Rust fmt/check/Clippy with warnings denied, **120 Rust tests** and doctests passed; **17 existing live tests remain opt-in**. Seven native regressions and three store regressions cover fork boundaries, process handles, source worktree/dirty-file safety, bootstrap failure/retry, persistence, metadata sequencing and navigation races.
- Frontend and the final local signed desktop build passed; strict deep signature verification passed (**61.50 MiB**). An earlier overlapping native build/test run produced a Rustdoc dependency lookup failure; the serial full validation passed including doctests. Existing optional Arc-gallery chunk and local bundle/notarization warnings remain.
- Actual desktop checks verified footer layout, successful Copy feedback, pin/unpin, bookmark navigation/focus, pin restored across application restart, dialog Tab navigation, Escape and corrected focus restoration. The temporary validation pin was removed; the original session/provider/empty draft remain selected and the updated app is open. No new inference, reset redemption, auth operation or user-file edit occurred.
- Real Git/persistence behavior was tested on disposable native fixtures. A new authenticated fork inference was not submitted; provider-private tool state and historical filesystem snapshots are not claimed. Graphify was incrementally updated (**3,589 nodes / 7,844 edges / 162 communities**); its existing TypeScript-store partial extraction warning remains, while both TypeScript compilers pass.

## Chat transcript alignment (2026-10-01)

- User messages are right-aligned in subtle, content-sized bubbles limited to 85% of the transcript width. Assistant responses stay left-aligned and unboxed. Role labels remain available to assistive technology; long words and multiline input wrap within the message.
- User Copy and the real localized sent timestamp appear on message hover or keyboard focus, with reserved footer space to avoid layout shifts. Devices without hover retain visible actions. The existing Copy feedback and assistant Copy/Fork/Pin footer are preserved; no native execution or persistence behavior changed.
- Application/test typecheck, lint, all **73 Node tests**, Rust check, frontend build, signed desktop build and strict deep signature verification passed. Actual desktop screenshots verified the resting layout, hover with focus elsewhere, keyboard access to Copy, and hiding both metadata controls after the pointer leaves. The updated app remains open on the original session with an empty draft; no inference or user-file change was submitted.

## Compact approval profiles and separate CLI diagnostics (2026-10-01)

- CLI System messages remain native and excluded from conversation context, but are displayed in the context panel's diagnostics tab. Actual turn errors remain visible in the chat. The approval menu uses compact inline icon/title rows, descriptions below and a right-aligned selected check; Full access uses the existing orange token.
- Ask/Auto/Full selections drive typed native per-turn profiles for verified adapters. Cursor offers Auto/Full; Grok remains vendor-managed. OpenCode Auto allows file edits only within its adapter policy. Unsupported callbacks remain denied, and no generic configuration or protocol IPC was added. The owner explicitly requested this change; rationale and provider-specific boundaries are in [ADR-017](decisions/ADR-017-per-turn-approval-profiles.md).
- Memory-only selections are draft-owner/provider scoped. Planning replaces remembered Full selections; successful sends cannot silently restore them. Native settings reject approval grants in model defaults. Codex confirms thread policy before input and explicitly restores restrictive turn policy; Claude requires Auto/Full mode acknowledgement before input; OpenCode generates per-process policy layers; Cursor keeps prompt/model arguments separate.
- Application/test typecheck, lint, **76 Node tests**, Rust fmt/check/Clippy with warnings denied, **127 Rust tests** and doctests passed; **17 live tests remain opt-in**. The installed Claude acknowledged manual, auto, bypassPermissions and plan in disposable directories with zero user prompts. Fixed protocol fixtures verify policy confirmation/rejection and restrictive restoration, not full-profile inference. Final read-only review found no remaining blockers.
- Frontend and the local signed desktop build passed (**61.60 MiB**); strict deep signature verification passed. Graphify was incrementally updated (**3,618 nodes / 7,922 edges / 170 communities**); its existing TypeScript-store partial extraction warning remains while both TypeScript compilers pass.
- The native computer-use connection began returning `cgWindowNotFound` for Sirus and other apps during reopening. The updated UI could not be visually checked in that build or confirmed open; the user must reopen the compiled app to load it. No new inference, login, credential change, reset redemption or user-project edit was submitted. Full mode was not selected through the actual desktop UI.
