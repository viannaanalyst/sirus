# ADR-177: Solar icons in the Linear style, like Zeron

**Status:** Superseded by [ADR-196](ADR-196-hugeicons-code-pills-find-words.md) (2026-11-06). Accepted (2026-10-28). Replaces the icon family chosen with Phosphor in the interface (see `docs/development/ui-arc.md`).

## Context

The owner wants the same icons, weight and sizes as Zeron (`zeronsh/comet`, MIT). Zeron's interface
icons are the Solar Icon Set by 480 Design (CC BY 4.0) in the Linear style, adjusted to a 1.7 px
stroke and drawn at 16 px in rows.

Sirus used Phosphor through one layer, `src/components/icons/phosphor.tsx`: about 210 exports with
Lucide-era names and the props `size`, `strokeWidth`, `fill` and `className`, where `fill` other
than `"none"` selected the filled weight for active and pinned items. Three more files imported
Phosphor directly.

## Decision

- The layer is now `src/components/icons/index.tsx` (imports are `@/components/icons`). Every
  export keeps its name and props. `size` defaults to 16 (it was 24). `fill` other than `"none"`
  or `weight="fill"` renders the Solar Bold glyph; `weight="bold"` thickens the linear stroke to
  2.2. `strokeWidth` is accepted and ignored: call sites were tuned for Lucide and Phosphor
  ignored it, and Solar Linear stays at 1.7 everywhere. Refs, `className`, `aria-*` and the other
  SVG props pass through.
- The reviewed mapping (exported name to Solar name) lives in `scripts/build-solar-icons.mjs`
  as `ICON_MAP`. The script writes `src/components/icons/solar-data.ts`, which holds only the
  icons used (191 Solar drawings for 209 names) as static SVG bodies. There is no runtime
  fetching and the full collection is not bundled. The generated file and the script are committed.
- Source order: Zeron's SVG when one exists with the same Solar name (66 icons; their 16 px marks
  such as check, close and terminal are scaled to the 24 px grid), otherwise `<name>-linear` from
  `@iconify-json/solar` (125 icons), a dev dependency. Bold glyphs come from `<name>-bold`; icons
  without one stay linear. All strokes are normalized to 1.7 and coordinates are rounded to two
  decimals. Zeron's files are read as data and never executed.
- Weaker matches, chosen deliberately: Heading1/2/3 share `text-field`, Cookie is `database`,
  Hand is `hand-shake`, Wrench is `settings-minimalistic`, PinOff is `pin` plus a slash drawn in
  the layer, GitPullRequestClosed reuses `pull-request`, and several folder variants use the
  closest Solar folder. Change them in `ICON_MAP` and re-run the script.
- `ProjectGlyph`, `SidebarRows` and `SidebarNavigationIcon` now use the layer. A tinted project
  folder uses the Bold glyph where it used Phosphor's duotone.
- Brand marks (provider and model logos) are untouched.
- Phosphor stays only in `src/lib/project-icons.ts`, the folder icon picker, because saved folder
  looks store Phosphor icon names. Renaming them would break saved projects. `src/components/icons/SquareSplitIcons.tsx`
  only mentions it in a comment. `@phosphor-icons/react` stays a dependency for the picker.
- Licences: Solar (480 Design, CC BY 4.0, with attribution and a statement of changes) and Zeron
  (MIT) are in `THIRD-PARTY-NOTICES.md` and `public/licenses/solar-icons/`.

## Consequences

- The whole app takes the new look through the layer with no call-site changes except the import
  path. Icons without an explicit size are now 16 px.
- The bundle shrank: all JavaScript went from 34,142,600 to 33,971,421 bytes (gzip 7,395,392 to
  7,374,640). Phosphor's React components carry every weight of each icon; `solar-data.ts` is
  261 KB of source (71 KB gzipped) for all 191 drawings in both weights.
- Adding an icon means adding it to `ICON_MAP`, running `node scripts/build-solar-icons.mjs`
  (with a Zeron clone as the optional argument; without it every icon falls back to Solar
  Linear) and adding one `export const` line to the layer.
- Two icon families remain in the bundle until the picker's stored names are migrated.
- Docs to update: the "Icons" row in the stack table of `docs/development/architecture.md`, the
  index in `docs/decisions/README.md`, and `docs/development/ui-arc.md` (the paragraph that names
  Phosphor and `src/components/icons/phosphor.tsx`).
