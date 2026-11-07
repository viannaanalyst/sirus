import { type ReactNode } from "react";
import { ActivityIndicator, View } from "react-native";
import Svg, { Defs, LinearGradient, Rect, Stop, SvgXml } from "react-native-svg";

import { useAppearancePreferences } from "../../features/settings/appearance/AppearancePreferencesProvider";
import { PROVIDER_MARKS } from "../../lib/provider-marks";
import { SymbolView } from "../AppSymbol";

/** The Mac's tile (`tab_tile.rs`): a small raised rectangle, a touch wider than tall. */
const WIDTH = 24;
const HEIGHT = 20;
const RADIUS = 5.5;

/** A provider's brand colors, top to bottom (`provider_tile`). */
const PROVIDER_FACE: Record<string, readonly [string, string]> = {
  claude: ["#E3876A", "#D06A49"], codex: ["#7A8CFF", "#4048FF"], droid: ["#E0733A", "#B9521F"], antigravity: ["#4D4D55", "#2C2C33"],
  cursor: ["#343434", "#161616"], grok: ["#343434", "#161616"], pi: ["#343434", "#161616"], hermes: ["#3A3A3E", "#1E1E22"], devin: ["#3A3A3E", "#1E1E22"],
};
const OTHER_FACE = ["#3B3636", "#1F1C1C"] as const;
/** The colors of a kind of work (`activity_face`). */
const WORK_FACE: Record<WorkKind, readonly [string, string]> = {
  terminal: ["#3A3A3C", "#1C1C1E"], pencil: ["#F6B44B", "#E08A12"], magnifyingglass: ["#6AAEFF", "#2F6FE4"], bot: ["#A98BF5", "#7246E0"],
};
export type WorkKind = "terminal" | "pencil" | "magnifyingglass" | "bot";

/** The Mac's `prompt.svg`, the terminal tile's mark. */
const PROMPT = `<svg width="24" height="24" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg"><path d="M5 7L10 12L5 17" stroke="#fff" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"/><path d="M12.5 17H19" stroke="#fff" stroke-width="2.4" stroke-linecap="round"/></svg>`;

/** What a run of steps shows on its tile: its first command, else its first step (`step_glyph`). */
export function workKind(items: readonly { readonly kind: string }[]): WorkKind {
  const lead = items.find((item) => item.kind === "command") ?? items[0];
  switch (lead?.kind) {
    case "command": return "terminal";
    case "edit": return "pencil";
    case "read": return "magnifyingglass";
    default: return "bot";
  }
}

/** A multicolor logo in one color, for a brand tile: the Mac draws them through a mask. */
export function whiteMark(xml: string): string {
  return xml.replace(/fill="(?!none)[^"]*"/g, 'fill="#fff"').replace(/stroke="(?!none)[^"]*"/g, 'stroke="#fff"').replace(/currentColor/g, "#fff");
}

function Surface(props: { readonly face: readonly [string, string]; readonly id: string; readonly neutral?: boolean; readonly children?: ReactNode }) {
  return <View style={{ width: WIDTH, height: HEIGHT + 1, alignItems: "center", justifyContent: "center" }}>
    <Svg width={WIDTH} height={HEIGHT + 1} style={{ position: "absolute", left: 0, top: 0 }}>
      <Defs>
        <LinearGradient id={`${props.id}-f`} x1="0" y1="0" x2="0" y2="1"><Stop offset="0" stopColor={props.face[0]} /><Stop offset="1" stopColor={props.face[1]} /></LinearGradient>
        <LinearGradient id={`${props.id}-s`} x1="0" y1="0" x2="0" y2="1"><Stop offset="0" stopColor="#fff" stopOpacity={props.neutral ? 0.05 : 0.16} /><Stop offset="1" stopColor="#fff" stopOpacity={0} /></LinearGradient>
      </Defs>
      {/* A hairline lift below, the solid face, a sheen brighter at the top, and a light rim. */}
      <Rect x={0} y={1} width={WIDTH} height={HEIGHT} rx={RADIUS} fill="#000" fillOpacity={0.3} />
      <Rect x={0} y={0} width={WIDTH} height={HEIGHT} rx={RADIUS} fill={`url(#${props.id}-f)`} />
      <Rect x={0} y={0} width={WIDTH} height={HEIGHT} rx={RADIUS} fill={`url(#${props.id}-s)`} />
      <Rect x={0.5} y={0.5} width={WIDTH - 1} height={HEIGHT - 1} rx={RADIUS - 0.5} fill="none" stroke="#fff" strokeOpacity={props.neutral ? 0.1 : 0.14} />
    </Svg>
    {props.children}
  </View>;
}

function Chevron(props: { readonly open: boolean }) {
  return <SymbolView name={props.open ? "chevron.down" : "chevron.right"} size={13} weight="semibold" tintColor="#fff" />;
}

/** The provider's brand tile over a turn; an open fold shows its chevron instead of the mark. */
export function ProviderTile(props: { readonly provider: string; readonly chevron?: boolean }) {
  const mark = PROVIDER_MARKS[props.provider];
  return <Surface id={`p-${props.provider}`} face={PROVIDER_FACE[props.provider] ?? OTHER_FACE}>
    {props.chevron ? <Chevron open /> : mark ? <SvgXml xml={whiteMark(mark.xml)} width={13} height={13} color="#fff" /> : null}
  </Surface>;
}

/** A run of steps' tile: the color of its work; working it turns neutral with a spinner, open it shows the chevron. */
export function WorkTile(props: { readonly kind: WorkKind; readonly live?: boolean; readonly open?: boolean }) {
  const { themeAppearance } = useAppearancePreferences();
  const light = themeAppearance !== "dark";
  if (props.live) return <Surface id={light ? "w-live-l" : "w-live"} neutral face={light ? ["#FFFFFF", "#EDEDED"] : ["#454547", "#333335"]}>
    <View style={{ transform: [{ scale: 0.62 }] }}><ActivityIndicator size="small" color={light ? "#555" : "#fff"} /></View>
  </Surface>;
  return <Surface id={`w-${props.kind}`} face={WORK_FACE[props.kind]}>
    {props.open ? <Chevron open /> : props.kind === "terminal"
      ? <SvgXml xml={PROMPT} width={13} height={13} />
      : <SymbolView name={props.kind === "bot" ? "person.2" : props.kind} size={12} weight="semibold" tintColor="#fff" />}
  </Surface>;
}
