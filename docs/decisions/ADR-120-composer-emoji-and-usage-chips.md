# ADR-120: Composer emoji picker and plan-usage chips

> **2026-10-13:** Removed at the owner's request. The composer no longer has the emoji button or the plan-usage chips; their components, the chip logic, the emoji catalog and its generator script are gone. The text below is kept as the historical record.

**Status:** Accepted (2026-10-12). Builds on ADR-013 (provider usage) and ADR-096 (popup motion).

## Context

The owner asked for an emoji picker in the composer and plan-usage chips beside it. Sirus already reads plan usage (`provider_usage`, the sidebar rings, `/usage`), but the composer never showed it, and there was no way to insert an emoji except the macOS character viewer.

## Decision

### Emoji picker

- **Button.** A smiley button sits right after the add (+) button. It opens a popover (the shared `[data-popup]` entrance and exit) with a search field, a skin-tone button, a category bar, the grid and a preview line with the emoji's name.
- **Catalog.** `src/lib/emoji/emoji-catalog.json` is generated from emojibase-data 16 (MIT) by `scripts/generate-emoji-catalog.mjs`, which takes the package folder as its argument, so the package is not a dependency. Each row holds the emoji, category, Unicode emoji version, English and Portuguese names, folded keywords from both languages and its skin-tone version. The five tones are only written out when they cannot be derived by putting the modifier after the first code point. The "component" category and multi-tone combinations are left out. About 1,900 emoji, ~200 KB raw and ~60 KB gzip. `loadEmojiCatalog` loads it with a dynamic import on the first open, so it is a separate chunk and never part of the main bundle.
- **Search.** Every query word must start a word of the English or Portuguese name or keywords. Accents are folded, so "coracao" finds "coração". Matches on the name in the UI's language come first.
- **Recent picks and skin tone.** These are saved on this Mac in local storage (`sirus.emoji.recent`, up to 24 base emoji, and `sirus.emoji.skinTone`). Recent picks are stored without a tone and drawn with the saved one.
- **Emoji the font cannot draw.** One emoji per Unicode emoji version (newest first), a flag and a toned hand are drawn once on a canvas. An emoji counts as drawable when it paints colour pixels (a missing glyph falls back to the black text fill) and is no wider than about one emoji (a broken ZWJ sequence or flag splits into several glyphs). Newer emoji, regional flags or tones are hidden when their sentinel fails. Tones that arrived after their emoji (for example a kiss with tones, 13.1) follow their own version. Where there is no canvas, as in tests, nothing is hidden.
- **Insertion.** The composer remembers the caret and selection from `select` and `change`. A pick replaces the selection or goes in at the caret (at the end if the field was never focused), then returns focus with the caret after the emoji. Closing without a pick also returns focus to the composer.
- **Keyboard.** The search field takes focus on open. ↓ enters the grid and Enter picks the highlighted (first) result. In the grid, arrow keys move with a roving tab stop, and ↑/↓ keep the column across category breaks. ↑ from the first row goes back to search, Home/End jump to the ends, and Enter or Space picks. Escape closes the picker only: the popover stops the key there, so a side chat or the agent's Stop behind it never sees it.
- **Localization.** Labels and category names come from `i18n/composer-extras-strings.ts` (en/pt), not from emojibase's category messages, whose Portuguese "flags" is wrong. Emoji names follow the UI language.
- **Cost.** The grid renders every category at once with `content-visibility: auto`, so off-screen sections skip layout. The grid is memoized, so hovering only updates the preview line.

### Plan-usage chips

- **What.** Next to the emoji button, `ComposerUsageChips` shows the session provider's short window (5 hours) and long window (weekly, or monthly for OpenCode Go) as small pills, each with a ring and a percentage. This is the same pair the sidebar rings draw (`ringUsageWindows`). At 70% the pill turns to the warning colour, and at 90% to danger. The tooltip lists each window with its reset time and countdown. A click opens the same `ProviderUsagePanel` as the rings.
- **When.** The chips appear only for providers that report usage (Codex, Claude, Cursor, OpenCode) and only once there is a reading. They are hidden on an error, while dictating and in narrow composers. Under 460 px the window names drop, and under 380 px the chips are hidden.
- **No new polling.** The chips read `usageByProvider`. When the provider has no reading yet, they make one unforced `refreshProviderUsage` call, which the native 60 s cache answers. A finished turn already refreshed the sidebar's providers, and it now also refreshes a provider whose usage is already loaded, so the chips stay current without timers.

## Consequences

- Updating the catalog means running the generator against a newer emojibase-data and committing the JSON. The runtime check keeps older macOS versions from showing emoji they cannot draw.
- Devin and other providers without a usage reader show no chips until `provider_usage` supports them.
