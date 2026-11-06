# ADR-118: Continuing conversations after a crash; small fixes

**Status:** Accepted (2026-10-12). Extends ADR-114 and ADR-115; refines ADR-057.

## Context

ADR-114 continued delegated tasks that the app's closing cut, but ordinary conversations only got "Execution was interrupted when Sirus closed." after a crash, a forced quit or a power loss, and the person had to notice and ask again. Such chats can continue with a guarded follow-up, capped, skipping threads that are put away or waiting on the person. Three small bugs came along: the context meter read "1000K" between 999,500 and 999,999 tokens, an empty context ring drew a dot, and Grok effort levels matched exact model ids, so `grok-4.7-build-fast` had none.

## Decision

- **Crash or normal quit.** A normal quit (including the confirm-close flow) runs `AppState::shutdown`, which saves every running turn as stopped. A session still saved as active at the next launch was therefore cut by a crash, and only those are candidates. Nothing new is written to tell the two apart.
- **Cut mid-turn.** At load, `crash_resume::cut_mid_turn` keeps sessions saved as starting or running whose last turn had not ended, was not waiting on an approval or a question, and had no open `request_secret` call (a secret card lives in memory only, so its tool row is what the transcript keeps). The list replaces the one `delegation` kept.
- **Ordinary conversations continue.** At startup `crash_resume::resume_interrupted` sends each one, in turn, a follow-up saying Sirus closed unexpectedly mid-turn and asking the agent to check what already happened (files, git state, processes) before repeating anything. It goes through `send_prompt` with the session's own execution options, so model, account and native thread are kept. Side chats, delegated tasks and team workers are left to their owners.
- **Shared limits.** `crash_resume::may_continue` applies to ordinary conversations and to the delegated path (`delegation::resumable`): not stopped by the person, not archived, not snoozed (an elapsed snooze does not hold it), and at most three automatic continuations in a row. The count is `Session.autoContinuations`, raised before each automatic send, cleared when a turn completes normally or any other prompt is sent. Delegated tasks still also need an unsettled link and a parent that was neither stopped nor archived.
- **Setting.** Chat behavior → "Continue interrupted conversations after a crash" (`continueInterruptedSessions`, on by default) turns the ordinary path off. Delegated tasks keep the ADR-114 behaviour, within the shared limits.
- **Context meter.** The formatter moved to `src/lib/context-meter.ts`; a count that would round to 1000K reads 1M (and 10M rather than "10.0M"). No progress arc is drawn while usage rounds to 0%, so a reported window with nothing used shows the empty ring rather than a dot.
- **Grok effort.** `grok_effort_levels` reads the version in the id: 4.6 and newer offer low to Extra High, 4.5 low to High, whatever follows the version (`grok-4.7-build-fast` is Grok 4.7's fast tier). The retired grok-build line (4.3 and older, `grok-build`, `grok-code-fast`, unversioned aliases) still gets no `--effort`.

## Consequences

- A crash no longer leaves working conversations silently stopped; a crash loop stops after three continuations and waits for the person.
- A turn that was mid tool call is not replayed; the agent decides what to redo after checking.
- Delegated tasks that were waiting on the person when the app died no longer continue automatically.
