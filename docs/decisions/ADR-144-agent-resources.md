# ADR-144: What the agents cost the Mac, and stopping them

- Status: accepted
- Date: 2026-10-19

## Context

Several agents running side by side can saturate the Mac: a `cargo build`, a test watcher or a browser left behind by a tool keeps the fans on long after the person stopped watching. Activity Monitor shows `node` and `rustc` but not which conversation started them, and stopping a whole conversation is the only control Sirus offered. The person needs to see, per running conversation, what its processes cost and stop either the conversation or one runaway process, without the app sampling the system all day.

## Decision

- **Reading (`agent_resources`, read-only).** For every session with a running turn (`AppState.agents`, the provider CLI's pid), the process tree below that CLI from one `ps -A -o pid=,ppid=,time=,rss=,comm=` read (`LC_ALL=C`, fixed `PATH`, 3 s timeout). Per process: pid, kernel start time, the executable's basename only (no command line: arguments can hold secrets), a coarse kind from that name (agent, shell, runtime, build, packages, Git, browser, tool), CPU and resident memory; per session the sums and process count (the agent first, then the 40 busiest); plus totals, the core count, the Mac's memory size and macOS memory pressure (`kern.memorystatus_vm_pressure_level`). CPU is the share of one core between two readings of cumulative CPU time (100% = one core, as in Activity Monitor): the previous reading is the baseline when it is under 10 s old, otherwise two readings 300 ms apart. Memory is RSS, so shared pages count once per process.
- **When it reads.** The renderer asks every 2 s only while the panel is open and the window is visible. With the panel closed it reads once when the set of running conversations changes (1.5 s after, so the new CLI has settled), on window focus and when the pointer enters the indicator (both at most every 5 s). There is no native sampler and no timer while closed, and nothing while no conversation runs.
- **Stopping one process (`agent_resource_action { stopProcess }`).** Closed action with `sessionId`, `pid` and the start time the panel showed. At that moment the session must still be running, and the kernel (`proc_pidinfo` on macOS, `/proc/<pid>/stat` on Linux; not the snapshot) must confirm: the pid still has that start time (a reused pid differs), its live parent chain reaches the session's agent CLI, and that CLI descends from this app. launchd, the app, the agent CLI itself and anything outside the tree (the person's terminals, other apps) are refused. The process and its descendants (each recorded with its own start time) get SIGTERM, and SIGKILL after 3 s if the same process is still there. A whole conversation stops through the existing `stop_agent`, with its delegated and launched sessions (ADR-101).
- **Indicator.** A gauge at the sidebar's foot in every style (above the quota rings on the icon rail, at the left of the list's foot), shown only while a conversation runs. It turns amber when the agents use 40% of every core or a quarter of memory, or memory pressure is high, and red from 75%, half, or critical pressure (`lib/agent-resources.ts`). Its popover lists running conversations (provider mark, title, CPU, memory, Stop), busiest first, each expandable to its processes (name, kind, CPU, memory, Stop; the agent row has no Stop of its own). Rows tint amber from about one core or a tenth of memory and red from two cores or a quarter. Numbers glide to each new reading (none with reduced motion); a stopped process leaves the list at once. The popover follows the shared popup entrance and cascade; the process list slides open.
- The commands are registered in the desktop app only; the headless host compiles the module but does not expose it. Over remote access (ADR-080) a paired device reaches them like `stop_agent`, under the same checks.

## Consequences

- Seeing the cost of agents costs nothing while the panel is closed beyond a few event-driven reads, and two `ps` runs per 2 s while it is open.
- Stop authority comes from live kernel lookups at the moment of the click, so a stale panel, a reused pid or a process that left the tree cannot be signalled.
- A process that left the tree by losing its parent (re-parented to launchd) is no longer listed or stoppable from here; the session's Stop still reaches it through ADR-100's snapshot.
- Codex and ACP processes kept between turns (ADR-107) are not listed: they are idle and not running a turn.
