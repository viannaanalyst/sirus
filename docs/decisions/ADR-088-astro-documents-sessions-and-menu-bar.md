# ADR-088: Astro documents, launched sessions, permissions and the menu bar

**Status:** Accepted

## Context

Astros (ADR-069) gain five features:
- lasting documents;
- a Sessions control on replies;
- a permission mode per Mono;
- hiding the sessions a Mono starts;
- a menu bar item with a floating chat.

Astros had none of these:
- Their work products lived only inside reply text.
- Sessions they started left no trace on the reply.
- Their conversations always started in Ask.
- Delegated sessions crowded the sidebar.
- There was no way to talk to an Astro without opening the main window.

## Decision

1. **Documents.**
   - `astro_documents_list`, `astro_document_read` and `astro_document_write` save Markdown reports and plans.
   - Each document is one JSON file in `documents/` next to `state.json`. The limits are 200 per Astro, a 120-character title and 100,000 characters of Markdown, and ids are UUIDs checked before any path is built.
   - A write lists the document on the running reply (`Message.documents`), which shows it as a card. The card opens a reader with formatting, Copy and Delete.
   - A creation retried in the same reply, with the same title, revises the saved document instead of duplicating it.
   - Deleting a document removes its cards from every saved reply, so a later save cannot restore them.
   - Open readers refresh on `astro-document-changed`.
   - Deleting an Astro removes its documents.
2. **Launched sessions.**
   - Sessions an Astro starts are recorded on the running reply (`Message.launched`) and marked `Session.launchedBy { astroId, hidden }`. This covers `astro_session_start` and, from an Astro, `sirus_create_session`.
   - The reply shows a "Sessions · N" control. It lists each session with provider, model, project, branch and status, and opens it.
3. **Permissions.**
   - `Astro.approval` (Ask, Auto or Full) is chosen in the drawer and defaults to Auto, including for Astros saved before this change.
   - A new conversation starts in that mode, and the composer of an Astro conversation falls back to it.
   - The automatic "sessions finished" turn now uses it too; before, it reset the mode to unset.
4. **Hidden sessions.** `Astro.hideSessions` keeps the sessions the Astro starts from then on out of the sidebar and lists (`launchedBy.hidden`). They stay saved and open from the reply's Sessions control.
5. **Menu bar.**
   - A tray item with the Sirus logo (a template image, so it follows the menu bar) lists "Astros", each Astro with a dot in its colour, then "Open SirusCode" and "Quit SirusCode". Quit takes the same path as ⌘Q.
   - Settings → General → Application → "Astros in the menu bar" (`showAstroMenuBar`, on by default) hides or restores the icon. The choice survives restarts (2026-10-08).
   - Choosing an Astro opens `astro-float`: a resizable window that stays above other windows and follows across Spaces. It runs the normal UI over the same state, showing only that Astro's conversation (`AstroFloat`), so messages, attachments, approvals, questions and stop all work there.
   - "Open in Sirus" shows the conversation in the main window.
   - A rail of Astros sits on the left of the float (2026-10-08): each Astro's icon in its colour with a tooltip, the current one marked (`aria-current`), an unread dot, and "+" for New Astro, which runs the main window's flow (create, open its conversation, open its drawer) inside the float. Choosing an Astro calls `astro_float_select`, the same path as the menu bar, so the window title changes and `astro-float-select` echoes the choice; the menu bar's choice moves the rail the same way. At 460 px and wider the rail stays beside the chat; narrower, it hides behind a toggle in the bar and slides in over the chat (Escape closes it). It dims while the window is in the background. The float now opens at 480×660. A deleted Astro hands the float to the first one left.
   - The float closes when the main window closes. Its close never runs the app quit flow.
   - Opening the float, switching it and showing the main window are refused to paired phones.

## Consequences

- Documents and launch records survive restarts. A launch record points to a session id, and a session that was deleted shows as gone.
- The float window has its own store, kept in sync by the same native events as the main window. It never writes layout or tabs.
- `tauri` now builds with `tray-icon` and `image-png`.
