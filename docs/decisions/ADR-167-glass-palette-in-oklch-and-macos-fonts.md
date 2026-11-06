# ADR-167: Glass palette in OKLCH and macOS fonts by default

**Status:** Accepted (2026-10-28).

## Context

The owner wants Sirus to look premium. Three directions were mocked up outside the repo:
"Graphite" (neutral), "Glass" (translucent, macOS-like) and "Studio" (warm, editorial). The owner
chose Glass, which builds on what Sirus already has: native macOS window and sidebar glass. Its
main window is built from layers of tinted glass, so the colors under the glass have to look
right on their own.

The tokens were fixed hex values. The surfaces were pure neutral greys, and the accent was
near-white in the dark theme and near-black in the light theme. The code and terminal fonts
defaulted to the bundled IBM Plex Mono. The owner asked for the Mac's own fonts for both the
interface and code.

The redesign is split into steps: tokens, then the composer (TipTap), diffs (`@pierre/diffs`),
the terminal (Ghostty's VT core) and small details. This ADR is the first step.

## Decision

- The palette tokens in `src/styles/index.css` (surfaces, borders, text, accent, status colors,
  `--glass`, `--shadow-float`) are written in OKLCH, so each step changes lightness evenly.
  - Dark surfaces are a cool graphite with a faint violet cast (hue 285, chroma at most 0.01).
  - Light surfaces use the same hue, with near-white surfaces and violet-tinted hairlines.
- The accent is the Sirus violet: `oklch(0.8 0.1 288)` in dark and `oklch(0.52 0.17 288)` in
  light, each dark or light enough to carry the text it sits under. It marks focus, selection,
  running state and primary actions. Everything else stays neutral.
- Corners are a step softer: `--radius-sm` 7px, `--radius-md` 9px, `--radius-lg` 14px and
  `--composer-radius` 1.4rem.
- The sidebar glass tints in `appearance.css` follow the new surfaces instead of fixed hex values.
- Code and terminal default to SF Mono (`sfMono`, resolved through `ui-monospace`). The UI keeps
  defaulting to the system font. A font the person already saved is kept. The other mono fonts
  remain choices.
- Translucency stays a material preference in Settings. The palette does not force it on or off.

## Consequences

- The whole app takes the new colors through the tokens without per-component changes. Surfaces
  that still hard-code hex values keep their old color until a later step moves them to tokens.
- Places that used the accent as a neutral highlight now show violet. That is intended: the accent
  is now a signal, not a neutral.
- Existing installs keep IBM Plex Mono when it was saved; choosing SF Mono in Appearance applies
  the new default.
