# ADR-041: Composer modes, persistent goals and native window attachments

**Status:** Accepted

## Context

The Add menu needs explicit debugging guidance and window images alongside existing files, skills and native planning. A goal should survive successful sends and application restarts. Renderer-supplied window identities would broaden capture authority beyond a user's selection.

## Decision

`ComposerAddMenu` uses inline title/description rows. Planning and Debug are mutually exclusive owner-scoped draft modes retained until changed. Planning still uses validated provider execution preferences and downgrades Full access through the existing composer policy. `SendPromptRequest.debugging` admits fixed native observe/reproduce/investigate/fix/verify guidance in `execution.rs`, preserving the visible transcript request; it does not grant permissions or invent observed evidence. Modes remain memory-only. Native Send rejects combining Debug and planning, and legacy requests default Debug off.

`SendPromptRequest.goal` is an optional native admission: omission retains the session goal, an empty string clears it, and a nonempty value stores at most 400 UTF-16 units with no NUL. `goals.rs` quotes this user data when composing each admitted process prompt, while the transcript retains the visible request. Legacy sessions default to no goal; forks and handoffs copy it. Unsent edits stay in the existing owner-scoped composer context until Send. Goals apply to subsequent user-requested turns; they do not autonomously start another turn or claim completion through inferred text.

`capture_prompt_window(owner)` is a closed Client command. Native code validates the owner before opening macOS 14+'s `SCContentSharingPicker` in SingleWindow mode, excluding Sirus. The renderer cannot enumerate applications or supply a PID, window ID, filesystem path, URL or screen region. The selected native filter admits one screenshot through `SCScreenshotManager`; no stream or computer-control approval is created. Output is bounded to 1600 pixels per axis and the existing 10 MiB attachment limit, then admitted into the existing private attachment cache after ownership is rechecked. Other platforms and older macOS return unsupported.

Apple documents using the [shared system picker](https://developer.apple.com/documentation/screencapturekit/sccontentsharingpicker) and [SingleWindow selection](https://developer.apple.com/documentation/screencapturekit/sccontentsharingpickermode/singlewindow); its [WWDC23 screenshot example](https://developer.apple.com/videos/play/wwdc2023/10136/) allows a filter selected through that picker without starting a capture stream.

Window and file pickers share the native single-picker gate. Cancel adds nothing. Completion is once-only with a 120-second timeout; main-thread observer cleanup is generation-bound so delayed callbacks cannot close a later picker. Existing attachment lifetime and batch limits remain authoritative. Successful Send clears the captured draft's binary references while retaining goal/modes; a newer draft edit is preserved. Automatic browser screenshots resolve restored native goals through the same owner-context helper.

## Consequences

- **Positive:** Explicit native selection limits capture authority; Session goal persistence and existing attachment admission share established trust boundaries.
- **Negative:** Window attachment capture is macOS 14+ only and depends on OS picker authorization. The browser fixture cannot exercise native capture.
- **Accepted trade-off:** Debug is guidance, not a dedicated diagnostic runtime. Goals require manual sends rather than a background continuation loop.

## Alternatives considered

Renderer window lists and IDs, as used by the reference application, were rejected because native selection avoids a new window-identity authorization protocol. Reusing computer-use capture was rejected because attaching an image should not authorize window control. Storing goal instructions only in a single prompt was rejected because it loses the goal on future turns and restart. Automatic goal continuation remains a separate execution/lifecycle feature requiring explicit product intent.
