# ADR-149: Links open where Chat behavior says, and look like links

**Status:** Accepted (2026-10-19).

## Context

Links in replies, in the person's own messages and in terminals opened in one fixed place: GitHub pull requests and issues went to the Pull requests page, other web pages to the conversation's Sirus browser, ⌘-click to the system browser. The setting only covered GitHub, and every link looked like plain blue text, so a bare URL read like prose. Another open-source client shows GitHub references and sites as small chips with the site's icon, and its right-click menu gives the same choices everywhere.

## Decision

- **Where a click goes** (`lib/link-chips.ts`, `components/ChatLink.tsx`). `Chat behavior → Open links in` (`linkOpenTarget`: `app`, the Sirus browser, or `system`; default `app`) decides plain clicks on web links in replies, the person's messages and terminals. ⌘, Ctrl, Option and middle click always open the system browser. A pull request or issue of a repository that belongs to a saved project (`githubLinksInApp`, default on) opens on the Pull requests page first; other GitHub items follow `linkOpenTarget`. The project repositories come from `pull_request_action { type: "repositories" }` (the origins of saved projects, no `gh` call), read once a minute.
- **Right-click** on a link offers Open pull request or Open issue (only for a project's repository), Open in Sirus browser, Open in system browser and Copy link, through the app's `ContextMenu`. Terminal links offer the same menu; elsewhere in the terminal the native menu stays.
- **Chips.** GitHub URLs show the GitHub mark and `owner`, `owner/repo`, `owner/repo#N` or `owner/repo@abc1234`. Other URLs show the site's favicon and the address without `https://`, `www.` and the trailing slash; local and private hosts show a local-server glyph instead. In replies, a link whose text is its own address is a chip, and a link with written text keeps its words after the icon. In the person's messages, bare URLs are chips. The full URL is the tooltip. The composer's mirror paints typed or pasted URLs in the link colour; the text itself stays the raw URL, so no icon or short label appears there, and the mirror wraps exactly like the textarea.
- **Label** (`lib/link-chips.ts`, `styles/chat-markdown.css`). A link that is only its address shows, after the favicon (or the GitHub mark or local glyph), its short label in the link colour with no box, as another open-source client does: GitHub repositories, pull requests, issues and commits read as `owner/repo`, `owner/repo#12` or `owner/repo@abc1234`; any other address is shown whole without `https://`, `www.` and the trailing slash, and wraps like text. The full URL stays the tooltip. The composer mirror paints typed addresses in the same link colour. (An earlier single-line pill with a middle ellipsis read badly on long addresses and was dropped.)
- **Addresses in code.** Inline code that is exactly one http(s) address (`codeLinkUrl`), as agents often write a local server's URL in backticks, renders as the same link, so a click opens it like any other; any other inline code stays code.
- **Favicons** (`site_favicon.rs`). `site_favicon { host }` returns a data URL or null. The renderer passes only the host, never the URL or path. Sources in order: Google's favicon service, DuckDuckGo's, then `https://<host>/favicon.ico` for public hosts only; that last request resolves the host first, refuses any private or loopback address and does not follow redirects. Only `https`, image content types, a body of at most 64 KiB; in memory and in `favicons/` next to the state (24 hours for found icons, 1 hour for misses); at most 500 hosts; concurrent asks for one host share one lookup. Localhost, single-label names, `.local`, `.internal`, `.ts.net` and IP literals never leave the Mac. The renderer keeps the globe while loading and on failure.

## Privacy

A favicon request sends the host name to Google and DuckDuckGo, as any site-icon service does. Full URLs, paths and queries never leave the Mac for this, and local hosts are never sent.

## Consequences

- `githubLinksInApp` is kept. When it is off, GitHub items follow `linkOpenTarget`.
- Issues open on the Pull requests page when the repository belongs to a project, since the page lists issues too.
- On a paired phone, the link has no Sirus browser (it is a Mac window), so a plain click opens the system browser there.
