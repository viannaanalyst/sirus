# ADR-211: Faster first token

## Context

Measured on the VPS (Claude Code 2.1.322, haiku, one-word reply): the CLI answers the
`initialize` request about 0.9 to 1.7 s after the spawn, but sends nothing to the model until
every MCP server connected or gave up, then needs another 1.3 to 1.7 s before the first token.
Before the spawn, `send_prompt` took a workspace baseline (`turn_review::Capture`) that
reads every tracked file: 0.4 to 1.3 s on a 2,000-file repository. Persisting twice costs
about 17 ms each on a 55-session `state.db`; binary resolution 0.2 ms; streaming batches at
16 ms. Those are not worth touching.

## Decision

- `SIRUS_TIMING=1` prints stage timings of the send path and of `claude::start` to stderr
  (`timing.rs`). Off, it costs one cached env read.
- For Claude the capture runs while the CLI boots. `claude::start` only spawns (the prompt
  goes out in `run_turn`), and the capture is awaited before the process is admitted and
  monitored, so the baseline still precedes the prompt. Other providers wait for the capture
  first (some take the prompt in their argv).
- When the session has remote connectors, the CLI gets `MCP_TIMEOUT=10000` unless the person
  set one: a connector that hangs costs 10 s instead of the CLI's 30 s, then is skipped.

## Measured

| Stage | Before | After |
| --- | --- | --- |
| Send to prompt written, cold start | capture + boot (about 1.7 to 3 s) | max(capture, boot) |
| First token, one hanging connector | 33.1 s | 11.9 s |

Not fixable here: CLI boot (about 1 to 2 s), the CLI's own wait before the first model
request (about 1.3 s), connector handshakes, and `--resume` reading a long transcript. A kept
process (`agent_pool`) avoids all of them; its key includes connector tokens, so a refreshed
OAuth token starts a new process.
