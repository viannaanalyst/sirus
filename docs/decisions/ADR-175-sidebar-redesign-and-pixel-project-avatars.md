# ADR-175: Sidebar redesign and pixel project avatars

**Status:** Accepted (2026-10-28).

## Context

The conversation sidebar (ADR-139) had grown a crowded top: a brand line, history buttons, a
search field squeezed beside the list order, New project and New conversation, and a row of
Inbox and PRs. A project was a folder glyph that the owner could swap for an emoji, a logo, a line
icon or an Astro icon (ADR-059, ADR-116), tinted in one of nine colors. Most projects kept the
plain folder, so the list was a column of identical grey glyphs.

The owner approved a mockup (built outside the repo) with a calmer layout and a small pixel
speech bubble with a face for every project, in the Glass palette (ADR-167). Inbox is being
removed (another change) and Tasks and Archived are already gone (ADR-172, ADR-173), so the new
layout simply does not have them.

## Decision

### Layout (list mode)

- Header: the traffic-light inset, then Back, Forward, Hide sidebar (⌘S) and New conversation (⌘N).
  The "Sirus" brand line is gone.
- Search on its own line (opens the existing palette, ⌘K), then a **Pull requests** row with the
  unread dot the rail already shows (there is no count of pull requests in the app, so none is
  shown), a hairline, and a small muted **Projects** title with **+** (New project dialog) and the
  list order menu at its right.
- A project row is the avatar (24 px), the name and, at the end, the uncommitted `+/-` counts or
  the time of its latest conversation. An open project lists its conversations indented under a
  thin guide, the selected one highlighted, a running or waiting one with its status dot, and Show
  more as before. The footer keeps the plan usage (`AgentResources`), Usage and Settings.
- Everything else is unchanged: both list orders (Recent and By project), pins, drag to split,
  the delegated hover card and context menus, the held order while the pointer is over the list,
  windowing, folder open/close motion, snooze, unread and attention markers, project edits.
- The 52 px icon rail (`Sidebar.tsx`) is untouched except that its project shortcuts show the
  avatar. Inbox modules and rail items are left to the change that removes them.

### Avatars

- `src/lib/project-avatar.ts` is pure. A 12x12 bubble (mask, outline from the mask edge, a Bayer
  dithered light/base/shadow gradient, a highlight, a tail) plus one spare row for a bob. Colors are
  roles (outline, shadow, base, light, shine, white, cheek) built from one hex color in OKLCH, with
  hue drifting cool in shadows and warm in lights, so any picked color gets a coherent ramp. Faces
  are white with a one-pixel drop shadow in the outline tone; there is no black ink.
- Variants: 12 swatches and a free color picker, eyes `dot | round | wide | sleepy`, mouth
  `smile | o | flat`, cheeks on or off. Defaults come from an FNV-1a hash of the project name, so
  every project has a stable look with no setup. (The mockup indexed with signed shifts; the hash
  is read unsigned here so every name maps to a valid eye and mouth.)
- States: idle (blinks), working (a conversation of the project runs: glances sideways, the mouth
  moves, a two-frame bob) and waiting (a conversation needs the person, or finished unseen: happy
  `^ ^` eyes, cheeks and the violet corner notch). They come from `tabStatus` through
  `projectAvatarStates`, memoised on the session list, the unseen ids and the computer requests so
  many avatars cost one pass per change. Waiting wins over working, like `projectStatus`. Eyes
  follow a nearby pointer.
- `ProjectAvatar` paints the sprite into a canvas whose backing store is `12k` by `13k` device
  pixels, with `k = round(scale * devicePixelRatio)`, shown at `12 * scale` CSS pixels with
  `image-rendering: pixelated`, so each sprite pixel is a whole block at any density. `ProjectIcon`
  is the one place that picks the avatar scale from a glyph size and reads the project's state.

### Performance

- One `AvatarScheduler` (`src/lib/avatar-scheduler.ts`) serves every avatar; there is no
  requestAnimationFrame per icon. An avatar repaints only when its look, state or pose key
  changes. One shared 200 ms timer (about 5 fps) runs only while an avatar on screen is working;
  one shared blink timer (every 1.6 to 2.9 s, a third of the idle avatars per round, 140 ms each)
  runs only while an idle one is on screen. The pointer is watched by one passive listener, at
  most one pass per frame, only while an idle avatar is on screen, and only avatars whose gaze
  changed repaint. One `IntersectionObserver` marks which avatars are on screen (the windowed list
  mounts few anyway).
- Everything stops while the document is hidden or the window is not focused. With
  `data-animations="off"`, `data-reduce-motion="on"` or the OS reduced-motion preference, avatars
  draw one static frame (working without the bob, waiting still happy) and no timer or listener
  exists. With no avatar on screen, no timer runs at all. Previews in the appearance popover and
  the sidebar-style thumbnails are still frames that never join the scheduler.
- `tests/avatar-scheduler.test.ts` runs the scheduler against a fake clock and window to hold
  these rules.

### Appearance popover and persistence

- Clicking a project's avatar (a button beside the row's toggle, since a button cannot hold a
  button), or **Appearance…** in any project context menu, opens a small popover: colors, eyes,
  mouth, cheeks, Shuffle and Reset to default. It animates in and out like the other popups, edits
  show at once, and the choice is saved 250 ms after the last change (a color drag is one write).
- `Project.appearance?: { color, eyes, mouth, cheeks }` in `models.rs` and `src/client/types.ts`,
  omitted when absent. The command `set_project_appearance { projectId, appearance | null }`
  (`commands.rs`, next to `rename_project`) calls `sidebar::set_appearance`, which refuses
  anything but a `#rrggbb` color and the known eye and mouth names (`deny_unknown_fields`), lowercases
  the color, and saves through `persist::save`. `state.db` keeps each project as a JSON row
  (`store.rs`), so there is no migration: old rows have no `appearance` and load as before. The
  command is in `generate_handler!`, so the headless host and paired phones run it through the same
  table, and `load_state` returns the field to them. The UI tolerates an unknown value by falling
  back to the name default for that part.

### The old project icon

- Projects no longer render emoji, logos, line icons, Astro icons, tinted folders or their own
  favicon anywhere (sidebar, header tabs and switcher, hover card, working panel, rail shortcuts,
  computer switcher, sidebar-style thumbnails, phone home, projects, sessions, choose-project and
  new-session). The picker in the Edit project dialog, the phone's project-look screen, the
  `updateProjectLook` store action, `client.projectLookAction`, `client.projectAutoIcon` and the
  "Automatic project icons" setting row are removed.
- Older saved data still loads. `Project.look` stays in the Rust model and in `state.db` and is
  simply ignored (never rewritten, so nothing is lost if the owner goes back); the setting
  `projectAutoIcons` stays in `AppSettings`. Project folders (groups in the switcher) keep their
  own icon and picker (`ProjectGlyph`, `LookPicker`), since they have no avatar.
- Pruned: `project_look_action`, `project_auto_icon`, the ICNS and automatic-favicon readers and
  the `projectAutoIcons` setting are removed. `Project.look` is still stored and ignored, and folder
  icons keep `pick_look_image` and `validate_look`.

## v2: 16x16 avatars with accessories

The owner approved a second look (mockup "Balões v2"). It replaces the 12x12 sprite; the scheduler,
states and performance rules above are unchanged.

- **Sprite.** 16x16 plus the spare bob row (`AVATAR_SIZE = 16`, `AVATAR_ROWS = 17`). A rounded
  block body (`MASK`) with the speech-bubble tail at the bottom left, an outline in the ramp's
  `edge`, an "extruded" darker `side` (2 px on the right, 1 px at the bottom), a lit top-left
  corner that fades through a checker into the `base`, and a three-pixel catch-light. Ramp roles are
  now `edge | side | base | light | shine | eye | eyeShade` (plus the violet notch pair).
- **Eyes.** `bars` (default: tall white bars with a shade pixel below), `dots`, `sleepy`, `happy`.
  No mouth and no cheeks. Blinks close the eyes, the working look-around shifts them by the gaze,
  and waiting shows the happy eyes and the violet corner notch.
- **Accessories.** `none | tophat | crown | antenna | bow | flower | beanie | headset | scarf`,
  drawn over the body as coloured rectangles. No pure black: dark parts are graphite with a lit top
  edge so they read on the dark sidebar. Defaults come from the name hash (colour from
  `hash % 12`, accessory from `(hash >>> 5) % 9`; eyes default to `bars`).
- **Sizes.** The canvas backing store is `16k` by `17k` device pixels with an integer `k` and its
  CSS size is exactly `16k / dpr` by `17k / dpr`, so each sprite pixel is `k` whole device pixels.
  `ProjectIcon` snaps a glyph box to 16, 24 or 32 CSS px (up to 18 px, up to 28 px, larger), which
  is `k = 2, 3, 4` at 2x (24 px is 48 device px, 3 per sprite pixel). At 1x the same boxes give
  `k = 1, 1, 2`. Slots of 12 to 17 px (hover card, tabs, phone rows) now show the 16 px sprite.
- **Appearance popover.** Colour swatches and the free picker, an accessory row and an eyes row
  (small avatars of the current look with that option), Shuffle and Reset to default.
- **Persistence.** `Project.appearance?: { color, eyes, accessory? }`. `accessory` is optional: when
  absent the name default applies. Rows saved before v2 keep loading: Rust maps the old eyes on
  deserialize (`dot`, `round` to `dots`; `wide` to `bars`; `sleepy` unchanged) and skips the old
  `mouth` and `cheeks` (`deny_unknown_fields` was dropped for this). The UI maps the same names
  (`LEGACY_EYES`), so an unmigrated value never breaks a render. `sidebar::set_appearance` still
  refuses unknown eyes and accessories, and rewrites the row in the v2 shape on the next save.

## Consequences

- Every project is recognisable at a glance with no setup, and the sidebar top is quieter.
- Avatars cost nothing at rest: no timers, no listeners. Working projects cost one 5 fps timer in
  total, however many are listed.
- (v1, superseded by v2) The avatar scale is a whole number, so glyph boxes snap to 12, 24 or 36 px; 13 to 17 px slots
  (hover card, tabs, phone rows) show the 12 px sprite.
- A project's icon can no longer be a logo or an emoji. This is the owner's choice.
- Old `look` data lingers unused until the follow-up prune.
- Phones show the avatars and state but do not edit them yet; editing is desktop-only.

## Amendment: conversation rows under a project

The one-line rows nested under a project drop the vertical guide line and the large running dot,
following Zeron's list. Each row starts with the conversation's agent mark (the provider logo, 13 px,
in a fixed slot so it lines up under the project name) and then the single-line title. While the
conversation works, the mark is replaced by a mini pixel spinner: a 2x3 grid of 2 px cells where a
light runs clockwise (ring order `[[0,1],[5,2],[4,3]]`, 1.2 s per turn), a CSS animation that pauses
offscreen and stays still with reduced motion or animations off. The right corner shows the age when
idle, otherwise an 11 px status: working (spinner, accent), needs you (dot, warning; pending approval
or question) or done (check, success; finished and not yet seen). The mapping is `rowStatus` in
`src/lib/conversation-row-status.ts`. Recent's two-line rows are unchanged.

## Hierarchy and Recent rows (2026-10-28)

- Projects: project names are primary text; their conversations are a step quieter (12 px, muted, 28 px rows), with the selected or unseen one in primary text, and 8 px of space after an open project's conversations before the next project.
- Recent, after Zeron: two lines per conversation. The project name in small muted type with the status (Working, Needs you, Done) or the age, then the agent mark (the pixel spinner while working) and the title. The branch shows only for a conversation in its own worktree; the model mark and the live dot are gone.
