# ADR-122: Prompt links

**Status:** Accepted (2026-10-12).

## Context

People want to share a prompt — in a pull request comment, a README or a chat — that someone else can open in Sirus with one click, in the right project and with the right model. A link comes from outside the app, possibly from a stranger, so it is a trust boundary: it must never run anything, and it must not be able to hide instructions in the prompt.

## Decision

- **The link.** `sirus://new?prompt=…&project=…|repo=owner/name&provider=…&model=…&pr=…`. `prompt` is required. `project` names a project by id, name (case-insensitive) or path; `repo` is a GitHub `owner/name`, matched against each project's `origin` remote (`git::remote_web_url`). `provider` is a Sirus provider id (`codex`, `claude`, `opencode`, …); `model` needs a provider and passes the same check as every model (`launch::valid_model`). `pr` is a pull request number, written into the draft as `Pull request: https://github.com/owner/name/pull/N` (or `#N` when no repository is known); there is no PR checkout from a link.
- **Never sent.** The link opens the project's new-thread landing with the prompt in the composer draft. The person reads it and presses Send. A link cannot choose approval mode, branch, worktree or anything else; the user's defaults apply.
- **Parsed natively.** `prompt_link.rs` reads and checks the link: scheme `sirus`, host `new`, no path, fragment, user or port; only the six known parameters, each at most once (anything else refuses the link); the whole link at most 128 KiB and the prompt at most 8,000 characters; a project value at most 1,024 characters without control or invisible characters; strict `owner/name` and number rules. A refused link shows why (one localized reason per `LinkError`).
- **Hidden characters.** Before anything else the prompt loses the characters a reader cannot see: C0/C1 controls other than tab and newline, zero-width and bidi controls (U+200B–U+200F, U+202A–U+202E, U+2060–U+206F, U+061C), the tag block (U+E0000–U+E0FFF), variation selectors other than the emoji ones, soft hyphen, BOM and Hangul fillers; a zero-width joiner stays only between emoji parts. Line and paragraph separators become newlines. The count goes to the UI, which shows a warning that stays until dismissed ("This prompt had hidden characters … may be malicious"). A prompt made only of hidden characters is refused.
- **Project matching.** `project` first (id, then name, then path, most recently opened first), then `repo`. When nothing matches, the draft goes to the current project (or the first one) and the toast says which project or repository was not found. With no project at all, nothing opens and the toast asks to add one.
- **Provider and model.** When the provider is installed and enabled, the landing's provider and model are set as if picked by hand (the landing's choice is the default, as always). A model missing from the provider's loaded list falls back to the provider's default; an unavailable provider leaves the default. The toast says which.
- **Existing draft.** A linked prompt replaces the landing's draft; the toast offers "Restore my draft".
- **Delivery.** `tauri-plugin-deep-link` registers the scheme (`plugins.deep-link.desktop.schemes` in `tauri.conf.json`; the bundler writes `CFBundleURLTypes` into the app's Info.plist). On macOS a link reaches the running app as an Apple Event, so a second instance never starts and `tauri-plugin-single-instance` is not needed. Native code keeps the latest link (a newer one replaces it; the same URL reported twice within 2 s counts once — the launch URL arrives both from `get_current` and the open event), emits `prompt-link`, and brings the window forward once the launch reveal (ADR-098) has happened. The UI takes the link with `take_prompt_link` when it hears the event and once after start, waiting until the projects have loaded, so a link that started the app is not lost.
- **No JS plugin.** The renderer does not use `@tauri-apps/plugin-deep-link` and the capability grants no `deep-link:` permission: links are read natively and the renderer gets only the checked result.
- **Remote devices.** `take_prompt_link` is refused over remote access (ADR-080), so a paired phone cannot take a link meant for the Mac.
- **Copy prompt link.** A palette action builds a link from the open composer's draft (the conversation, or the landing) with its provider and model, through `build_prompt_link`: `repo=owner/name` when the project's `origin` is on GitHub, otherwise `project=<name>`. Building and reading are tested to round-trip.

## Consequences

- Sharing a prompt is one palette action; opening one is one click, and the person always sees and sends it themselves.
- Unknown parameters refuse the link, so a link made for a newer version fails visibly instead of being half applied.
- Choosing a provider through a link changes the landing's default provider and model, as picking it by hand does.
- The scheme registration works only in a bundled `.app` (Launch Services reads the Info.plist); `tauri dev` does not receive `sirus://` links on macOS.
