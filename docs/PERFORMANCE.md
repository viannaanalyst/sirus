# Performance and resource audit

Status: **Stages 1–5 done**, 2026-10-03. Stage 5 repeated the measurements on the real app with real provider output. Stage 1 recorded the baseline without changing product code; Stage 2 removed idle work; Stage 3 made streaming and persistence cheaper.

Sirus is a desktop app that may stay open for hours. Its goal is to stay clearly lighter than an Electron app and to do almost no work when nothing is happening. This document records what was measured, what was found and what is planned. Numbers are only stated where they were actually measured.

## Current architecture (performance-relevant)

| Layer | Shape |
| --- | --- |
| Processes | One Tauri/Rust app process, plus WebKit's WebContent, GPU and Networking helpers. Agents, PTY shells, Git, version probes and MCP children are child processes. |
| Native state | One `AppData` behind one global `Mutex` (`state.data`). It holds every project, every session and every message. |
| Persistence | `persist::save` serializes the whole `AppData` as pretty JSON, writes a temp file, fsyncs it and renames it. |
| Frontend state | One Zustand store. `sessions` holds every session with its full `messages`. |
| Realtime | Tauri events: `agent-output` (one per provider delta), `session-updated` (the full `Session`), `pty-output` (one per 4 KiB read), `browser-state`, `computer-state` and notification events. |
| Rendering | React 19 with no React Compiler, almost no `memo`, and no list virtualization. |

## Measurements (baseline)

### How they were taken

- **Native app.** `scripts/perf/measure-idle.sh` launches the debug bundle, waits 20 s, then samples `top` every 5 s for 60 s (Stage 1; later stages use cumulative CPU time). Memory is `footprint` (phys_footprint) per process. WebKit helpers are attributed by launch diff: helper processes that appear after the launch belong to Sirus. This is a heuristic.
- **Frontend.** Vite dev server in headless Chromium, with the recording stage's demo IPC (fictional data). Scripts: `scripts/perf/idle-probe.mjs` and `scripts/perf/stream-probe.mjs` (Playwright is not a project dependency; set `PLAYWRIGHT_MODULE` to an installed copy). Tools are CDP `Performance.getMetrics` and an instrumented `requestAnimationFrame` / timer counter. Chromium is not WebKit, so these numbers show **relative** costs and where work comes from, not absolute app CPU.
- **Persisted state.** The user's real `state.json` at the time of the audit: 32 KB, 2 projects, 3 sessions, 25 messages. Every result below reflects this **small** state unless stated otherwise.

### Native app, debug build, idle on the restored view

| Metric | Run 1 (cold) | Run 2 (warm) |
| --- | --- | --- |
| Launch → app process | 0.09 s | 0.08 s |
| Launch → WebKit helpers | 0.27 s | 0.25 s |
| Idle CPU, `sirus-code` | 5.7 % | 5.8 % |
| Idle CPU, WebKit GPU | 7.3 % | 7.4 % |
| Idle CPU, WebKit WebContent | 6.0 % | 6.0 % |
| **Idle CPU, total** | **≈19 %** | **19.1 %** |
| Footprint, `sirus-code` | 33 MB | 37 MB |
| Footprint, WebContent | 118 MB | 117 MB |
| Footprint, GPU | 76 MB | 76 MB |
| Footprint, Networking | 5 MB | 5 MB |
| **Footprint, total** | **≈232 MB** | **≈235 MB** |

`sample` of the idle native process shows nearly all threads waiting: `__psynch_cvwait`, `mach_msg`, `kevent`. Its 5.7 % is therefore most likely compositing work on the main thread driven by the web animations below. To be confirmed after Stage 2.

Not measured yet:

- time to first usable UI (the launch numbers above are process start, not readiness);
- release-build numbers;
- memory with many sessions, a running terminal or a live agent.

### Frontend idle, Chromium, 10 s window per screen

| Screen | Task time | rAF/s | Sources |
| --- | --- | --- | --- |
| Landing (new thread) | 20 % of a core | 120 | composer shader (`@paper-design/shaders`); the orbit loop was removed 2026-10-07 |
| Session view | 16 % | 60 | composer shader |
| Session view, composer focused | **0.4 %** | 0 | none |
| Settings open over the landing | 6 % | 120 | both loops keep running behind Settings |

One 30 s `setInterval` exists, the midnight/profile day check. No other polling was found.

### Streaming (proxy), Chromium, composer focused, 10 s scripted turn

| Transcript | Task time | Script | JS heap |
| --- | --- | --- | --- |
| 6 messages | 19.4 % | 101 ms/s | 44 MB |
| 300 messages with code blocks | 22.4 % | 134 ms/s | 39 MB |

The demo emits a full `session-updated` per word, which approximates Codex activity events but over-states plain text deltas. Use this only to compare runs.

## Findings

Severity: **H** high, **M** medium, **L** low. File references are from the audit at this date.

### Idle work

- **H** The composer's WebGL liquid-metal rim (`ComposerContour` / `composer-metal.ts`) renders on every animation frame whenever its textarea is not focused. That means while reading a transcript, with the dock open, behind Settings, and on window blur. Measured as the main idle cost.
- ~~**M** The landing orbit loop runs at display rate on the landing.~~ Removed with the orbit background (2026-10-07).
- **L** The waiting-state activity orbit (SMIL `animateMotion` plus CSS beat) has no offscreen or hidden gate. A pending approval can wait for hours.
- OK: the activity clock interval, the dictation interval and the global listeners all clean up. There is no frontend polling.

### Streaming and rendering

- **H** Every `agent-output` delta maps all sessions and copies the message array. `App` subscribes to the current session and almost nothing is memoized, so the whole tree re-renders per token: Sidebar, header tabs, transcript, composer, dock, Environment.
- **H** `CodeBlock.highlight` is not memoized and is roughly O(n²) for tsx/jsx/html. It re-runs for every code block on every render.
- **H** The transcript re-parses every message per render (`parseTranscript`), with no row memoization and no virtualization.
- **H (native)** `record_output` counts UTF-16 units over the whole message on every chunk, under the data lock (`agent.rs`). This becomes quadratic for long replies.
- **H (native)** `session-updated` serializes the entire Session, every message included, on every activity change, under the lock (`codex.rs`).
- **M** An open transcript search re-scans every session on each chunk.
- **M** The sidebar recomputes groups twice on every chunk and every keystroke, because it subscribes to all drafts.
- **M** Each terminal instance adds its own global `pty-output` listener (N+1 deliveries per chunk). PTY output is not batched natively.

### Persistence and locks (native)

- **H** Whole-state pretty JSON with fsync runs under the global lock. It happens about once per second per stream (stdout and stderr each), up to 4 times per second while typing drafts or notes, and several times per turn.
- **H** `state.json` has no size bound: no message-count cap, up to 8 MiB per message, and 16 retained diffs per session. Every cost above grows with it.
- **H** Git and worktree work runs under the data lock: session create/fork/delete and team start. The main thread can block on that lock because sync commands and main-thread callbacks take it.
- **M** Blocking fsync and lock waits run on tokio worker threads, which can starve other sessions' readers.

### Processes and lifecycle

- **H (bug)** `remove_project` looks up PTYs by **session id**, but the map is keyed by **terminal id**. Those shells and their reader threads are orphaned. `delete_session` does it correctly.
- **H (conditional)** `write_terminal` holds the `ptys` lock during a blocking write, while the reader needs that lock to drain output. A large paste into an echoing program can deadlock.
- **M** Git `run`/`run_ok` have no timeout or output cap, and one `git::status` spawns about 16 git processes. Terminal kill uses two `ps` scans plus a sleep, run in sequence at shutdown.
- **M** The frontend forces a usage probe on every `agent-exit`. The native cache lasts 2 s, so this is almost always a new process.
- **M** Turn review hashes up to 4096 files (16 MiB) before and after each turn, in the admission and settlement paths. On very large repos it pays the cost and then returns an empty review.
- **M** Sent attachments stay cached until the owner is deleted. (The audit also reported browser webviews left open on session delete; that was wrong: `deleteSession` and `removeProject` already call `browserClose`.)

### Startup

- **H** `ready`, and therefore the window reveal (the launch splash until 2026-10-08; ADR-098), waits for 9 sequential listener registrations and `detect_agents` (a `--version` probe per installed CLI, up to 3 s each) and `gitIdentity` for every project.
- **M** `load_state` runs on the main thread and clones the whole state. On launch, persistence parses the file twice and may re-save.

### Memory growth

- **M** `editorBuffers` keeps the full content of every file opened in Files until its tab is explicitly closed.
- **M** All transcripts stay in memory in both the frontend and native layers.
- **L** `unseenSessionIds` and `modelChangesPending` keys are not pruned on session delete. Browser MCP tokens and per-session MCP config files are never removed.

## Plan

| Stage | Scope | Risk |
| --- | --- | --- |
| **2. Cheap, safe fixes** | Gate the decorative loops (composer shader, landing orbits, waiting orbit) on window blur, Settings coverage and visibility; default the shader to rest, with the design decision confirmed with the owner. Fix the `remove_project` PTY orphan and the PTY write/read lock. Prune stale store keys. Discard clean editor buffers. Stop the forced usage probe per turn. Unblock first paint from `detect_agents` and `gitIdentity`. | Low |
| **3. Streaming and persistence** | Coalesce `agent-output` per frame. Keep a running UTF-16 offset natively. Make `session-updated` a delta for activity. Memoize message rows and code highlighting. Move to compact JSON, serialize outside the lock and coalesce writes on a writer thread (with crash-safety review). Batch PTY output. | Medium |
| **4. Session content on demand (ADR)** | Separate session metadata from message bodies in memory and on disk, and load transcripts when opened. Bound or page old history. | High |
| **5. Re-measure** | Repeat every measurement above with the same scripts and compare. | — |

## Stage 2 changes

| Change | Where |
| --- | --- |
| One shared gate stops decorative loops while the window lacks focus, the document is hidden or Settings covers the window. Focus comes from window focus/blur events, not `document.hasFocus()`, which WKWebView can report as false while the app mounts. The composer rim keeps animating while the window is focused (owner decision, option B). | `src/lib/ambient-motion.ts`, `composer-metal.ts`, `landing-orbit-motion.ts`, `App.tsx` |
| The waiting-approval orbit and its beat stop when offscreen, unfocused or covered. | `AgentActivity.tsx`, `agent-activity.css` |
| `remove_project` now closes its sessions' terminals, matching them by owning session instead of looking up the terminal map by session id. | `commands.rs` |
| `write_terminal` releases the terminal map lock before a blocking write, and the reader emits output without holding it. | `commands.rs`, `pty_term.rs` |
| Closing several terminals (session delete, project removal, app shutdown) uses one process scan per signal and one shared 100 ms grace period, outside the data lock. | `pty_term.rs`, `commands.rs` |
| The first paint waits only for `load_state`, `host_info` and the selected project's Git identity. CLI detection and other projects' identities fill in afterwards. A detection failure no longer fails bootstrap. | `app-store.ts` |
| The usage refresh after each turn is no longer forced, so the native 60 s cache bounds probes. | `app-store.ts` |
| A clean editor buffer is released when its last view closes; unsaved edits stay. | `EditorPane.tsx` |
| Deleted sessions are pruned from `unseenSessionIds`. | `app-store.ts` |

### Stage 2 measurements

Chromium idle probe (same method as the baseline):

| Screen | Before | After |
| --- | --- | --- |
| Landing, window focused | 20 % · 120 rAF/s | 21 % · 120 rAF/s (kept, option B) |
| Session view, window focused | 16 % · 60 rAF/s | 15 % · 60 rAF/s (kept, option B) |
| Session view, window blurred | 16 % · 60 rAF/s | **0.2 % · 0 rAF/s** |
| Settings open | 6 % · 120 rAF/s | **0.4 % · 0 rAF/s** |

Native app, debug build, the same restored session view, CPU time over 15 s (old build kept outside the repository, same machine and state):

| Build | Window focused | Window in background |
| --- | --- | --- |
| Before Stage 2 | 22.3 % | 21.3 % |
| After Stage 2 | 22.3 % (kept, option B) | **0.1 %** |

`measure-idle.sh` now computes CPU from cumulative CPU time (`ps -o time=`). Its earlier `top` averaging produced false 0 % readings when several `-pid` filters were combined, so the first focused-window comparison was discarded.

## Stage 3 changes

| Change | Where |
| --- | --- |
| `persist::save` writes compact JSON. Every snapshot gets a generation number while the state lock is held, and a write older than the file on disk is dropped. | `persist.rs` |
| Streamed text and activity no longer save inline. `checkpoint_soon` coalesces all streams into at most one write per second, encodes under the lock and writes and fsyncs outside it. Identity, request and final states still save synchronously. | `persist.rs`, `agent.rs`, `codex.rs` |
| Each output stream keeps its message's known UTF-16 length, so offsets no longer recount the whole message per chunk. An outside edit (length mismatch) falls back to a full count. | `agent.rs` (`OutputCursor`) |
| PTY reading and emitting are split: the emitter drains every queued read into one `pty-output` event (up to 64 KiB). No timers. | `pty_term.rs` |
| `agent-output` deltas are applied to the store once per animation frame (a macrotask when hidden). `session-updated` and `agent-exit` flush pending deltas first. | `app-store.ts` |
| A `session-updated` snapshot keeps the previous object of every unchanged message. | `agent-events.ts` (`reuseMessages`) |
| Views that never read messages (sidebar, tabs, dock, Environment, composer, usage footer, notifications) select session metadata that stays stable while only streamed text changes. `App` subscribes to the current session's agent, title and started state only. | `app-store.ts` (`selectSessionsMeta`, `selectCurrentSessionMeta`), `App.tsx`, `SessionPane.tsx` |
| Transcript rows are memoized and parse their message once per content change. Code highlighting is memoized and no longer slices the whole source per token. | `SessionPane.tsx`, `code-block.tsx` |
| Message-trail ticks are memoized with stable items, so the rail no longer rebuilds every tooltip per frame. | `MessageTrail.tsx` |
| Offscreen earlier messages use `content-visibility: auto` (the latest turn stays visible), so the browser skips their layout and paint. | `index.css` |
| Terminal output and exit events use one native listener fanned out to every terminal, instead of one native listener (and payload copy) per terminal. | `client/index.ts` |

### Stage 3 measurements

Realistic streaming probe (`scripts/perf/stream-probe.mjs`, Vite dev build in Chromium): about 100 `agent-output` deltas per second for 10 s, with a code block, into a live message. Dev-mode React adds overhead to both columns; compare relatively.

| Transcript | Before (initial commit) | After Stage 3 |
| --- | --- | --- |
| 6 messages | 97 % of a core, script 913 ms/s; fell behind (3,204 chars shown) | **30 %**, script 206 ms/s; kept up (4,584 chars) |
| 300 messages with code | 98 %, script 956 ms/s; effectively frozen (168 chars shown) | **41 %**, script 274 ms/s, layout 35 ms/s; kept up (4,479 chars) |

The 6-message profile is now about 70 % idle; most of the remaining script time is the probe's own 100 Hz emitter. The older demo probe (one full `session-updated` per word) went from 22.4 % to 17.5 % for 300 messages.

Not measured: real provider CLIs in the native app, and WebKit release-build numbers. Heap readings in the dev probe vary between runs with GC timing and are not used.

## Stage 4a changes

[ADR-047](decisions/ADR-047-per-session-transcript-files.md): `state.json` keeps metadata only and each session's transcript has its own `sessions/<id>.json`. Saves hash every transcript and write only the changed files, then the index. The streaming checkpoint clones the state under the lock and encodes, hashes and writes outside it. A legacy file is backed up to `state.json.pre-split-backup` and split once; the user's real state migrated with identical transcripts.

Synthetic 300 sessions × 60 messages (release build, one message changed per save):

| | Before | After |
| --- | --- | --- |
| Bytes written per save | 25.3 MB (whole file) | 216 KB (135 KB index + 81 KB transcript) |
| Save time (encode, hash, write, fsync) | — | 21–28 ms |
| Load | — | 19 ms |

Comparable tools keep conversations apart from the index too: Codex CLI and Claude Code use per-session JSONL and OpenCode SQLite (see the ADR).

Still open: the native side encodes every transcript per save (CPU, not disk), and a single huge session is rewritten whole per checkpoint.

## Stage 4b changes

[ADR-048](decisions/ADR-048-transcripts-on-demand.md): `load_state` sends metadata only, `session-updated` sends only the current turn, and the closed read-only `transcript_action` loads one transcript, returns bounded search candidates, or returns prompt activity. The renderer loads transcripts when opened and keeps a bounded set.

Native debug build, synthetic 300 sessions × 60 messages (25 MB), separate `HOME`, first 25 s after launch, two runs each:

| | Stage 4a build | Stage 4b build |
| --- | --- | --- |
| `load_state` payload | ~25 MB | 165 KB |
| Native process | 101 MB | 70 MB |
| Webview (WebContent) | 220–221 MB | 199 MB |
| CPU (all processes) | 2.0–2.2 s | 1.6 s |
| Same build, empty state | — | WebContent 81 MB, CPU 0.6 s |

Verified end to end on the demo IPC (same contract): opening and switching sessions, returning to a cached session, all-conversations search and jumping into an unloaded session, sending and streaming a turn, and Profile statistics.

### Sidebar rows

The remaining webview growth came from the sidebar, not transcripts. It rendered every session row, including those of collapsed folders: about 18 DOM nodes and 125 KB of JS heap per row, each with its hover card, context menu and store subscriptions.

- Collapsed folders now unmount their rows once the close transition ends.
- An open folder with more than 60 sessions renders only the rows near the visible area (12 rows of overscan), with padding standing in for the rest. Rows keep one measured height.
- The visible design is unchanged.

`scripts/perf/sidebar-probe.mjs`, Chromium, one open project:

| Sessions | Before | After |
| --- | --- | --- |
| 5 | 417 DOM nodes · 27 MB heap | 383 nodes · 27 MB |
| 305 | 5,817 nodes · 65 MB heap | **972 nodes · 32 MB** (34 rows rendered) |
| 305, scrolled to the middle | — | 1,215 nodes · 34 MB (47 rows, the correct ones) |

Selecting a windowed row, collapsing (rows unmount after the transition) and reopening were verified.

## Stage 5: before and after

Native debug build on the owner's machine with the display on, the owner's real state (3 sessions) and the restored session view. CPU is cumulative CPU time over 20 s for the app and its WebKit helpers; the window was driven through Accessibility.

| Metric | Stage 1 baseline | Stage 5 |
| --- | --- | --- |
| Idle CPU, window focused | 22.3 % | **12.5 %** (composer rim still animates, option B) |
| Idle CPU, window in background | 21.3 % | **0.0 %** |
| Footprint, app process | 33–37 MB | 23 MB |
| Footprint, WebKit GPU | 76 MB | 19 MB |
| Footprint, WebContent | 117–118 MB | 151 MB (not reduced; see below) |
| Launch to visible window | — | 1.75 s |

A real Codex turn (40-line answer, no tools) streamed and settled in the app. The app processes averaged 31.6 % of a core over the 30 s turn, excluding the provider CLI. The turn's own transcript file was the only one rewritten, and `state.json` held no messages.

The 300-session synthetic history is covered in the Stage 4b table above (`load_state` 25 MB → 165 KB, native process 101 → 70 MB, startup CPU 2.0–2.2 → 1.6 s).

Chromium probes, same scripts, compared with the Stage 1 baseline:

| Probe | Stage 1 | Stage 5 |
| --- | --- | --- |
| Session view, window blurred | 16 % · 60 rAF/s | 0.3 % · 0 rAF/s |
| Settings open | 6 % · 120 rAF/s | 0.5 % · 0 rAF/s |
| Landing / session, focused | 20 % / 16 % | 24 % / 20 % (kept animating, option B) |
| Realistic streaming, 6 messages | 97 %, fell behind | 36–37 %, keeps up |
| Realistic streaming, 300 messages | 98 %, froze (168 chars in 10 s) | 55–57 %, keeps up |
| Sidebar with 305 sessions | 5,817 DOM nodes · 65 MB heap | 972 nodes · 32 MB |

Notes:

- The streaming probe now keeps its fictional data in the same JS heap as the app (about 290 MB in the 300-message case). That raises GC work against the Stage 3 figures (30 % / 41 %), which used a different setup, so compare those with care.
- Render counting during streaming found finished code blocks, the selection menu and a closed search bar re-rendering per frame. They are now memoized, which cut per-frame motion presence work about 6×.
- WebContent did not shrink on this small state. Its size is dominated by the engine, styles and fonts rather than data, and Stage 1 sampled it after a different view had loaded.

Computer use (ADR-038) was verified live through its driver: Accessibility and Screen Recording are recognized, Calculator was observed (26 elements), 7 + 8 = 15 was pressed through Accessibility, and a window screenshot was captured in 2.7 s. The end-to-end agent flow (approval card, control pill, Escape) still needs an agent turn in the app.

## Follow-up audit (2026-10-05)

A code-reading audit after the workspace features. It found issues; none of them were measured.

**Fixed:**
- **Chat file links.** A bare filename in a reply (`` `App.tsx` ``) ran a recursive workspace search, up to 8,192 entries or 150 ms, per reference. Such a name can only match a root entry. Bare names now check one shallow root listing per session, cached for 15 s.
- **Unfocused window.** Running-turn shimmer and pulses kept animating while the app was visible but not focused:
  - the activity line;
  - running tab dots;
  - team progress;
  - status indicators.

  They now follow the shared ambient gate. `html[data-ambient]` pauses the CSS pulses.
- **Side chat.** The side-chat composer no longer mounts a second animated rim shader.
- **Keystrokes.**
  - The sidebar views subscribe to a stable list of draft owners (`useDraftOwners`) instead of every composer keystroke.
  - The tab/unseen store subscriber exits early when none of its inputs changed.
- **Streaming frames.**
  - The usage-limit resume timer re-arms only when a limited session's state changes, not on every streaming frame.
  - Team progress cards read session metadata only.
- **Editor.**
  - The editor skips the full-document comparison when the incoming value is its own last edit.
  - The dock subscribes to the set of unsaved files rather than every buffer, so typing re-renders only the edited pane.
- **Streaming replies.** Each Markdown block is memoized by its source, so a streaming reply re-parses only the block that changed.
- **Native browser.**
  - A session keeps at most 8 tabs; the oldest closes when a new one opens.
  - At most 4 sessions keep live WKWebViews. The least recently opened one is released and reopens at its last page.

**Still open:** the Environment card's blur over a streaming dock may be recomputed per frame. Not measured.

## Scroll and attachment pass (2026-10-08)

From code reading after reports of scroll jank and paste hitches:

- **Chat background:**
  - the image is a `blob:` URL, never a multi-MB data URL in a CSS variable on the root;
  - Haze is blurred once on a canvas;
  - `.session-pane::before` sits on its own layer.
- **Per-second renders:**
  - the Working panel keeps its own clock, so the conversation no longer re-renders each second;
  - running turns cache parsed text instead of re-parsing on every tick and streamed frame.
- **Compositing:**
  - the activity pulse grows and fades instead of animating a shadow;
  - shimmers step at about 15 fps and pause with ambient motion off;
  - the composer rim rests while the transcript scrolls.
- **Attachments:**
  - clipboard TIFF→PNG runs off the main thread;
  - a paste tries the native clipboard before encoding files in JavaScript;
  - live `session-updated` events send the turn's user message (and its thumbnails) once per turn;
  - `sameMessage` compares fields instead of stringifying messages;
  - `serde_json` is optimized in debug builds.
  - sent image thumbnails are files (`thumbnails/<id>.jpg`) loaded through `sirus-thumb://`, so transcripts, checkpoints, `session-updated` events and cached transcripts in JS carry only `hasThumbnail` instead of up to ~640 KB of base64 per image; older sessions migrate on load.

## Graphics memory (2026-10-08)

In the owner's running app, WebContent swung between ~475 MB and ~1.7 GB. Almost all of it was graphics memory, while the WebKit heap stayed at ~220 MB. An isolated copy of their data ran at their 5K window size, where one full-window layer is ~58 MB. Measured in that copy:

| Scenario | Graphics steady | Graphics peak | WebKit Malloc |
|---|---|---|---|
| Owner's settings, no background | ~185 MB | 314 MB on a session switch | ~155 MB |
| Background image shown in sessions (effect none) | 400–434 MB | 672 MB | ~159 MB |
| Scrolling a 62-image session top to bottom | ~185 MB | 592 MB | 155 → 264 MB |
| Scrolling a text-heavy session | ~186 MB | 477 MB | 98 → 141 MB |

Fixes:
- **Background layer:** `.session-pane::before` no longer forces its own layer. The image is static, and the transcript already scrolls on its own layer.
- **Running-turn shimmers:** they no longer carry `will-change`. Their animation moves `background-position`, which is never composited, so the extra layer inside the scroller cost memory for nothing.
- **Bubble thumbnails:** a sent image's bubble loads a 192 px copy (`?size=small`, made once beside the thumbnail). Before, the bubble decoded the 1280 px thumbnail, ~3 MB per image; the viewer still opens the full one.
- **Effect canvases:** freed as soon as they are exported.

**Not done:** the transcript scroller spans the whole pane width (about 4600 px at 5K), so scroll tiles cover empty margins. Narrowing it would cut the scroll peak, but wheel scrolling in the margins would then need forwarding.

## Following the end of the transcript (2026-10-08)

Following the latest output changes only on the reader's own direction. A wheel or trackpad step up releases it at once, so the next streamed frame cannot snap the reader back. Steps with no vertical direction (sideways, the end of a momentum swipe) change nothing. A small reversal while reading does not resume it; reaching the very end while moving down does. Content changing size under the reader never flips it. Covered in `tests/transcript-scroll.test.ts`.

## Memory under agent load (2026-10-08)

**Report.** With 3 conversations running agents, Activity Monitor showed "Sirus" at 6.28 GB. That figure groups the app, its WebKit helpers and child processes (provider CLIs and their subagents). Later, mostly idle, the owner's debug app read as follows (`footprint`, `vmmap --summary`, read-only):

| Process | Footprint | Peak | Main categories |
|---|---|---|---|
| App (Rust) | 311 MB | **2.5 GB** | Malloc Large 201 MB, all of it *empty* (freed blocks kept by the allocator, 201 MB swapped). The zones held only ~112 MB of live allocations. |
| WebContent | 2.1 GB, 2.19 GB 25 min later | **8.5 GB** | "Owned physical footprint (unmapped)" 1272 MB, all of it swapped or compressed; WebKit Malloc 534 → 614 MB (304 MB swapped); graphics 184 MB. |
| WebKit GPU | 90 MB | — | — |

Baseline earlier this month was a WebContent of ~118 MB.

**What it is not.**
- **Not stored data.** The whole data folder is 24 MB. All 10 transcripts together are 2.9 MB; the largest is 1.28 MB.
- **Not command output buried in old turns.** Outputs are capped natively at 16 KiB per row, and the largest seen was 3.5 KB. Across the three big sessions, command outputs total 0.24 to 0.46 MB per session.
- **Not terminals.** xterm uses its DOM renderer (no WebGL) and 2,000 lines of scrollback by default. At 12 bytes per cell that is about 5 MB per terminal at 200 columns ([xterm BufferLine](https://raw.githubusercontent.com/xtermjs/xterm.js/master/src/common/buffer/BufferLine.ts)).
- **Not `evaluateJavaScript` keeping scripts alive** ([WebKit bug 215729](https://bugs.webkit.org/show_bug.cgi?id=215729)). An isolated WKWebView probe on this machine evaluated 2,000 Tauri-shaped 150 KB `session-updated` scripts (300 MB in total). Its WebContent stayed at 13–18 MB and peaked at 23 MB.

**What it is.** The memory comes from transient volume, not from what is kept.
- **Where the volume comes from.** Tauri delivers every event by evaluating a script that embeds the JSON payload ([Tauri: events are evaluated JS and "not suitable for bigger messages"](https://v2.tauri.app/develop/calling-frontend/)). Each `session-updated` carried the whole current turn, so every tool row, every subagent step and every context reading re-sent it, with all of its command outputs.
- **Estimated size.** Replaying the owner's 116 stored turns, with one start and one end publication per row, gives 2,460 events and 40.6 MB. That is a lower bound: context readings and subagent steps publish too.
- **What a burst costs.** If the main thread falls behind, the scripts queue in the app process and then land in WebContent together. The same probe with 2,000 queued scripts peaked at 299 MB in the host and 243 MB in WebContent, about the size of what was queued, and dropped back once processed.
- **Why it stays.** The allocators keep what such bursts freed:
  - Native: the 201 MB of empty Malloc Large regions.
  - WebKit: Malloc keeps its high-water mark, and WebContent stays swapped.

  "Owned physical footprint (unmapped)" is memory the kernel charges to the process but that is not mapped into it ([footprint(1)](https://leancrew.com/all-this/man/man1/footprint.html)). WebKit charges memory the GPU process creates to WebContent ([webkit.org #220770](https://bugs.webkit.org/show_bug.cgi?id=220770), [#241455](https://bugs.webkit.org/show_bug.cgi?id=241455)). Telling which process maps the 1.27 GB needs `sudo footprint --unmapped`, which was not run.
- **Not ruled out.** The app is a debug build, so the Web Inspector is enabled ([Tauri `devtools` is on in debug](https://docs.rs/tauri/2.2.2/tauri/)). Its cost was not measured.

**Fixes:**
- **Coalesced activity events.** Activity changes and context readings are published at most once per 250 ms per session; lifecycle changes (requests, answers, identity, model, the end of a turn) still go out at once and replace a pending one. The snapshot is taken under the state lock when it is sent, so streamed deltas stay ordered around it. `transcript_view::emit_soon`; covered by `frequent_publications_coalesce_and_an_immediate_one_supersedes_them`.
- **Each command output sent once.** A live event sends a command row's output only when it changed in this turn. The renderer keeps the output it already holds when a command row arrives without one; natively, a command's output never goes back to empty. Transcript loads still carry every output. On the stored turns this takes the replay from 40.6 MB to 15.8 MB (2.6×), before coalescing. Covered by `live_events_send_each_command_output_once_per_turn` (Rust) and `transcripts.test.ts`.
- **Streaming checkpoints clone only streaming sessions.** Before, each checkpoint (at most one per second while output streams) cloned the whole `AppData`, every transcript included, and re-encoded them all. Now it encodes the index (metadata only, ~18 KB here) and clones only the transcripts of the sessions that streamed. Other files are left alone, and full saves still write every changed transcript and remove orphans. Covered by `a_streaming_checkpoint_writes_only_the_named_transcripts`.
- **Freed native memory goes back to the system.** `Info.plist` sets `MallocLargeCache=0` through `LSEnvironment`, and `main` removes the variable so agents and shells keep the default.
  - Probe on this machine: 2,000 blocks of 150–600 KB, 98 % freed. With the default large cache, the footprint stayed at 755 MB after freeing; with the variable it dropped to 17 MB. Verified through a bundle launched with `open`.
  - `malloc_zone_pressure_relief` released 0 bytes in the same probe, so it is not used.
  - Cost: about 13 µs per 64–512 KB allocate/free cycle instead of 0.4 µs. That is negligible at a few large allocations per event.
  - It only applies when the app is launched as a bundle, not under `tauri dev`. `MallocLargeCache` is an undocumented libmalloc switch; `MallocSpaceEfficient=1` behaved the same in the probe.
- **Memory pressure.** `memory.rs` watches macOS memory-pressure notifications (a libdispatch source) and emits `memory-pressure`. The renderer then keeps only the transcripts in use (selected, active, queued, retained); the others reload when opened. WebKit trims its own caches on pressure ([webkit.org #206077](https://bugs.webkit.org/show_bug.cgi?id=206077)).

**After.** Not measured in the running app: the owner's app was not rebuilt or restarted. Measure the next build at the same point (3 agents, then idle): footprint and `vmmap --summary` peaks for the app and its WebContent.

**Still open:**
- Subagent step lists and `detail` are still re-sent with every event of a turn. Per-row deltas would need a new event shape.
- Large events could move to a `tauri::ipc::Channel`, which fetches payloads of 8 KB or more instead of evaluating them. Channel messages are not ordered with `agent-output`, so both streams would have to move together.
- Old turns' command outputs stay in loaded transcripts. Loading them on demand needs the activity rows (`AgentActivity.tsx`) to fetch when a row opens.
- Compare a release build without the inspector.
- Run `sudo footprint --unmapped` on a grown WebContent.

## Front end: turn window, row props and transcript budget (2026-10-08)

Three renderer changes.

- **Only the last turns are mounted.** `SessionPane` mounted every message of a loaded transcript; `content-visibility` skipped painting old rows but not building them (markdown, code blocks, activity). Now a turn window (`src/lib/turn-window.ts`) mounts the last 3 turns on first paint and grows to 20 in a `startTransition`. "Show earlier messages" above the transcript, or scrolling up within 400 px of the top while not following, adds 20 more. The first mounted row is measured before and after, and the scroll position moves by the difference before the follow/anchor logic of `transcript-scroll.ts` runs, so its captured anchor stays valid. The button sits outside the content element, because the anchor treats the content's grandchildren as rows. Store jumps (search hits, pins, fork origins) widen the window and run once the target is mounted; trail clicks widen synchronously (`flushSync`). The message trail still lists every request. Each session opens with the default window. Rows inside the window keep `content-visibility`. Covered in `tests/turn-window.test.ts` and `scripts/verify-message-trail.mjs`.
- **Rows skip streamed frames by what they read.** The store builds a new session object per flushed frame, and native events rebuild its nested objects. The row's `memo` compare already ignored session identity, but checked `team` by identity (new on every native event) and missed fields its children read (`worktree.path`, `astro`, `sideChat`, `execution.approval`, the last message). `sameRowProps` (`src/lib/transcript-row.ts`) compares those as values, compares the whole team only on the coordinator's row, and the row gets `last` instead of reading `session.messages`. Covered in `tests/transcript-row.test.ts`.
- **A byte budget for kept transcripts.** Besides the 8 most recent, kept transcripts are capped at an estimated 32 MB together (`TRANSCRIPT_BUDGET`). The estimate counts UTF-16 text of contents, activity labels, details and outputs, review diffs, steers and attachments, cached per message object in a `WeakMap`. The selected, active, queued and retained transcripts are never released and count first; recent ones are added newest first while they fit, and one that alone exceeds the room left is skipped. Memory pressure still keeps only those in use. Covered in `tests/transcripts.test.ts`.

**Not measured** in the running app (no build or launch in this pass).

## Diffs and terminals (2026-10-08)

- **Large diffs mount in chunks.** `DiffViewer` (Changes pane, turn review, pull request detail) mounted one row per diff line. It now mounts the first 400 rows; a sentinel below the last row, watched by one `IntersectionObserver` per list (root: the diff's scroll area, 800 px bottom margin), mounts 400 more in a `startTransition`. The observer is recreated per chunk, so a sentinel still in range reports again. The sentinel reserves the height of the unmounted rows (20 px each), so the scrollbar reflects the whole diff. Row indices are positions in the full parsed diff, so line numbers, the comment range and "Copy diff" (the whole text) are unchanged; a selected comment range is always mounted. The diff rows themselves are not syntax-highlighted. Untracked files used the highlighted `CodeBlock` whole; now only files up to 400 lines do, longer ones use the chunked rows. The viewers show one file at a time, so there is no multi-file body to defer, and there is no jump-to-line in diffs yet (`diffCountFor` mounts up to a target row for one). Native find (⌘F in WebKit) sees only mounted rows. Covered in `tests/diff-window.test.ts` and `scripts/verify-turn-review.mjs`.
- **Hidden terminals are skipped.** Inactive tabs were `opacity-0`, so they still laid out and composited and redrew on every write. They are now also `visibility: hidden` and `content-visibility: hidden`; they stay mounted because their shell lives with the xterm instance (an unmounted terminal stops its PTY). xterm still parses writes in the background, so terminal queries (cursor reports) keep working. Whether xterm's own render loop pauses depends on WebKit reporting skipped content as not intersecting to its observer; this was not measured.
- **Terminals read only their settings.** Each `TerminalInstance`, pane header and tabs pane subscribed to the whole `settings` object, so any settings save re-rendered every terminal. They now select the locale, terminal font, size, cursor style, scrollback and the three resolved colours.
- **Bounded scrollback.** Sirus has no native replay buffer: the PTY reader emits and forgets, and a terminal is never re-attached (closing it stops the shell). The kept history is xterm's scrollback, which was unbounded in stored settings. `terminalScrollback` is now bounded to 1000–10000 lines in `mergeSettings` and in Rust (`appearance::validate` / `normalize`, `TERMINAL_SCROLLBACK`), fallback 2000. Covered in `tests/appearance.test.ts` and the Rust range test.
- **No terminal polling to pause.** Sirus runs no periodic process or cwd poll for terminals (the only `ps` in `pty_term.rs` is in a test), so there is nothing to skip while the document is hidden.

**Not measured** in the running app (no build or launch in this pass).

## Optimized daily build (2026-10-08)

The owner's daily app was an unoptimized debug build. `[profile.dev]` now builds the app crate at opt-level 1 and every dependency at opt-level 2, with line-table debug info only. The bundle stays at `src-tauri/target/debug/bundle/macos/Sirus Code.app`. The first build after the change recompiles everything; later rebuilds of the app crate stay quick. The full release profile (LTO, one codegen unit) is kept for distribution.

## Native process hygiene (2026-10-09, ADR-100)

- **Git locks.** Every native Git command sets `GIT_OPTIONAL_LOCKS=0`, and `git::status` counts lines with `git diff-index -M --numstat HEAD` instead of porcelain `diff`, so status polling never writes the index or holds `.git/index.lock` while an agent runs Git. A test checks the index bytes are unchanged after a status over a stat-dirty file.
- **Untracked cap.** `git::status` lists at most 2000 untracked files and sets `untrackedTruncated`; a large untracked build tree no longer ships every path to the renderer.
- **Agent priority.** Agent CLIs are reniced by +5 right after spawn (children inherit it); PTY terminals keep normal priority.
- **GitHub reads.** One gate for every `gh` read: 5 concurrent, a 60 s fail-fast pause after any rate limit, reuse of an identical read from the last 10 s.
- **Not measured** in the running app (no build or launch in this pass).

## Virtualized diffs (2026-10-19, ADR-145)

Synthetic fixture: 50 files with 20 000 changed lines, plus one 20 000-line file, rendered with `renderToString` through Vite (median of 15, on a busy machine, so ranges):

| | Before | After |
| --- | --- | --- |
| Open the 20 000-line file | 400 rows, 172 KB, 12–14 ms | 80 rows, 37 KB, 3–16 ms |
| Scrolled to its end | 20 000 rows, 8.7 MB, 416–505 ms | 80 rows, 37 KB, 3–14 ms |
| Open the 50-file review | one file, 400 rows | all files, 79 rows, 4–10 ms |

Document and layout for 20 000 rows: about 2 ms; window per scroll frame: under 1 µs; parsed-diff cache hit: under 1 µs. Tokenizing one 400-line segment in the worker: about 6 ms (the whole 20 000-line file: about 120 ms, never asked for at once). Checked in a browser on the 50-file review: about 60–110 rows mounted at any position, colours arriving after the rows, pinned headers, collapse and the place restored after unmounting. Not measured in the native app.

## Remaining risks and unknowns

- Behaviour with a large `state.json` (hundreds of sessions, long transcripts) was **not measured** on the native app. The current user state is small. Stage 3 or 4 needs a synthetic fixture with a separate data directory, never the user's own state.
- Agent streaming was measured only through the demo proxy, not with real provider CLIs.
- WebKit numbers in the release build may differ from the debug build.
- Several findings (lock contention, PTY deadlock) come from code reading and were not reproduced.
- Native `session-updated` still serializes the whole session (all messages) on each published activity change, under the lock. The renderer now absorbs it cheaply, but the native cost grows with transcript length. Stage 4b (renderer transcripts on demand) is the place to change the event shape.
- Synchronous saves (settings, drafts, metadata, final states) still encode and fsync under the state lock. Only streaming checkpoints moved out.
- `content-visibility` relies on remembered sizes. A never-rendered old message uses an estimate from its length until it is first shown, and the transcript anchors the reading position by hand, because WebKit has no scroll anchoring. Not yet checked against trackpad momentum in WKWebView.
