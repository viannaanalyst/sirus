# ADR-139: A conversation list as a third sidebar style

- Status: accepted
- Date: 2026-10-18

Extends ADR-092 and ADR-121.

## Context

The icon rail (ADR-092) leaves conversations to the header switcher and tabs. The owner approved a preview of a third style: a column listing recent conversations from every project, with project, age, title, branch and model.

## Decision

- `AppSettings.sidebarMode` (`rail` | `list`, default `rail`) chooses the style; the existing `sidebarCollapsed` still means hidden. Settings → Appearance → Sidebar offers Icon rail, Conversation list and Hidden; Hidden keeps the style, so ⌘B toggles the chosen style and hidden. Old settings load unchanged as the rail.
- `sidebarListOrder` (`recent` | `project`, default `recent`): Recent is flat by last activity ("Working and unread first" applies); By project groups under one header per project, ordered by its latest unpinned conversation. Pinned conversations come first. Both are closed native enums.
- The list shows listed conversations only: no archived, snoozed, side chat or Ask conversation, and no team worker whose coordinator is listed. The order holds while the pointer is over it (ADR-121).
- The column was 280 px, resizable within 264–420 px (memory only); since round 3 it is a fixed 256 px. Its header has the window controls space, Sirus, back/forward, the order menu and collapse; then search (command palette), Open project, New project, New conversation; Inbox / Tasks / PRs with their dots; the rows; usage rings, Archived and Settings at the foot. Rows show project icon, name and age with the status dot, the title (stronger when unread), branch and model mark; one list-level context menu offers split, rename, pin, snooze, archive, move and delete. The header project switcher is hidden while it shows (⌘⇧P opens the palette instead); tabs stay.
- Performance: windowed past 60 items with two measured heights, rows memoized on session metadata, no polling (ages refresh on focus, visibility or pointer entry), rows unmounted while hidden.

## Consequences

- The list's width is not persisted.
- The PR dot is computed in the list's own nav hook as well as in the rail; they should share one helper.

## Amendment: round 2 (2026-10-18)

The owner approved a second preview: a simpler list header, folder-only By project, a fourth style that shows one project at a time, and Settings cards with real miniatures.

- **List header.** The top row holds the window controls space, Sirus, small back/forward arrows (the titlebar arrows are not drawn while a column shows, so they stay here) and collapse. The second row is Search (palette), Order (the order menu, moved down), New project and New conversation. The separate Open project button is gone: New project opens the dialog that already offers opening a folder, creating one and cloning. Inbox · Tasks · PRs follow, unchanged. The order menu also toggles "Working and unread first".
- **By project is folders only.** One collapsible row per project (chevron, project icon, name, the dot of its most urgent conversation — working, then waiting, then unread — and its count). An open folder is followed by its conversations, indented, without the project line: dot, title and age, then branch and model mark. Folder rows are buttons with `aria-expanded`. Which folders are open is `AppSettings.sidebarOpenFolders`: `null` until the person opens or closes one (then the selected project's folder shows open), otherwise at most 100 owned project ids, validated and pruned natively. Selecting a conversation anywhere opens its folder.
- **Project style.** `sidebarMode` gains `project` (closed enum; older settings still load as the rail). The column has the window controls space, collapse and back/forward on top; the project switcher (the header's popover, mounted here instead, opening on click or ⌘⇧P but not on hover) with New conversation; the same Search · Order · New project and Inbox · Tasks · PRs rows; the selected project's conversations under Pinned, Today, This week and Older (compact rows: dot, title, age; branch and model in the tooltip); a sideways-scrolling strip of every project's icon in the switcher's order, with arrows, the current one highlighted and a click switching project; and below it the usage rings, Archived and Settings, kept as their own small row so they stay reachable. The header switcher hides while this column shows, like with the list.
- **Settings → Appearance → Sidebar** shows three radio cards (Icon rail, Conversation list, Project), each with a still miniature of the window in that style: lightweight replicas that reuse the sidebar's own classes, fed with the person's first projects and conversations from the store, drawn at 1000 × 600 and scaled to the card. They exist only while the page is open, never animate and make no native calls (project icons skip the favicon lookup).
- **Round 3 (folders and choices).** Folder rows drop the count and put the chevron at the end (New conversation appears just before it on hover). A closed folder's status dot (working, waiting or unread) takes the chevron's place and gives way to the chevron on hover; an open folder shows none, since its rows carry their own. An open folder's conversations are one line each under a thin guide from the folder: dot, title, the branch only when the conversation has its own worktree, and age; the model is in the tooltip. A folder shows its first 5 conversations (plus the selected one) and Show more / Show less, remembered for the window only. Opening grows the rows in with a short stagger; closing folds them up before they leave the list (none with reduced motion). The Hidden card is gone: ⌘B hides any style, and picking a card shows the sidebar again. The list and Project columns now have one fixed width, 256 px, with no resize handle (the owner preferred a slimmer, steady column).
- Performance is unchanged: one windowed list for all three orders (conversation rows and headers each have one measured height), memoized rows, no polling; time groups read the clock only when ages refresh, and a held order also holds each row's group.

## Amendment: hover card (2026-10-19)

The owner approved a hover card for the conversation rows (Recent, folder and Project style rows).

- **When.** After the pointer rests on a row for 450 ms, a card opens to the right of the column, lined up with the row (a virtual anchor at the column's edge, 12 px away, collision-aware), so it never covers the list. While one is open, or within 300 ms of it closing, the next row's card shows at once and the card glides there; leaving waits a 120 ms grace. It never opens on touch, while a button is pressed or a split drag runs, while the list's context menu is open, or on keyboard focus; a press on a row closes it until the pointer leaves that row, and scrolling, Escape or the window losing focus close it. It is read only (`role="tooltip"`, no pointer events) and animates in and out like the other popups.
- **What.** The title (two lines), a pill only when the conversation is working, needs the person (an approval, a question or a computer-use request) or failed, the project and the row's age, the branch with the lines changed (only for the selected conversation with its own worktree, from the Git status the window already reads; a shared checkout's totals belong to the project, so none), the model and the computer it runs on (the active computer's name, omitted when unknown), and what the conversation said last.
- **The reply preview is native.** Sessions arrive metadata-only (ADR-048), so the metadata gains `lastReplyPreview`: the last paragraph of the newest assistant reply that has words, as plain text (Markdown dropped, code blocks skipped), credentials masked (ADR-077), at most 280 characters (`reply_preview.rs`, mirrored by `src/lib/conversation-hover.ts`). It is derived like the transcript counts (ADR-099): from the content while a transcript is loaded (so a running turn's events carry its newest paragraph) and otherwise kept beside the counts in the session's index row; an index written before it reads the newest 40 message rows once at load. No command, no IPC per hover and no polling. A transcript the window holds is read directly, so the card follows a streaming reply between events.
- **Performance.** One card and one controller per list, driven by delegated pointer events on the rows' container: rows and the list never re-render when it opens, moves or closes.


*2026-10-19:* the owner dropped the reply snippet from the hover card (the session metadata keeps `lastReplyPreview` for other surfaces) and the rows' native tooltip, which showed over the card. Folder rows in By project show the project's uncommitted `+N −M`, as the header switcher does.

## Amendment: one project menu (2026-10-19)

Right-clicking a project now opens the same menu everywhere: the header switcher's rows, a By project folder row, the Project style's title (a left click still opens the switcher) and each icon in its strip (without switching project). The icon rail's project shortcuts have no context menu and keep none. Before, folder rows opened an empty menu and the Project style's title and strip had none.

- **Items, in order:** New conversation (in that project, through the same path as the folder's New conversation button); Edit project; Move to each other folder, Remove from its folder, New folder with this project; Open in Finder (`open_path`, which only opens project, worktree and app data folders) and Copy path; Remove from list, with its confirmation. Rules separate the groups.
- **One builder.** `useProjectMenu(onAction?)` in `ProjectActions.tsx` returns `items(project)` and the dialogs (Edit project, the folder dialog, the Remove confirmation) to render once per surface; the rows come from the pure `projectMenuEntries` in `src/lib/project-menu.ts`. `onAction` runs before the items that leave the surface, so the header switcher closes before New conversation or a dialog, as it did for Edit; Remove from list now closes it too.
- **No empty menus.** The conversation list keeps one delegated menu, which reads the row under the pointer: a conversation's menu, or the project's on a folder row. On a time group or Show more, and on the strip's arrows, nothing opens; and a `ContextMenu` with no items never shows an empty pill (the header tab list's empty area had the same flaw).


*2026-10-19 (hover actions):* folder rows in By project lose their chevrons (the row still opens and closes; `+N −M` and the closed folder's status dot sit at the right edge). Pointed-at rows show the same buttons as the header project switcher, from one component (`HoverActions.tsx`): a project offers Review changes, New terminal session and New conversation in place of its numbers; a conversation offers Pin and Archive in place of its age, in all three list orders and the Project style.

*Update 2026-10-26:* the Project style was removed; only the icon rail and the conversation list remain (Settings shows two cards). Its switcher in the sidebar, time groups, compact rows, project strip and "New conversation in …" button went with it. A saved `sidebarMode: "project"` loads as the list (`#[serde(alias = "project")]` natively, the settings normalizer in the window).
