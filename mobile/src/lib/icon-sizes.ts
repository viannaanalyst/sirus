/**
 * The app's common icon sizes (points), in one place so a change reaches every screen. They are
 * about 12% above the first design's (18 -> 20, 22 -> 24, ...); hit areas stay as they were.
 */
export const ICON = {
  /** Round buttons of the top bar and sheets. */
  bar: 20,
  /** The top bar's pill segments. */
  segment: 18,
  /** Leading icon of a list row (`ListRow`, `ToolRow`, `SheetRow`). */
  row: 24,
  sheetRow: 20,
  /** The composer's plus, microphone and send arrow. */
  composerPlus: 23,
  composerMic: 22,
  composerSend: 20,
  /** The provider mark and its chevron in the composer's model capsule. */
  capsuleProvider: 20,
  capsuleChevron: 11,
  /** The glyph on a step-tree tile and the tile itself. */
  treeGlyph: 16,
  treeTile: 28,
} as const;
