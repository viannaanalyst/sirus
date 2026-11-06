import { memo, useState, type ReactNode } from "react";
import { Pressable, View } from "react-native";

import { SymbolView } from "../AppSymbol";
import { AppText } from "../AppText";
import { ShimmerText } from "./Shimmer";
import { useWorkAnchor } from "./work-anchors";
import { ICON } from "../../lib/icon-sizes";

/**
 * One row of the activity tree (the approved preview's `row`): a small rounded tile on the
 * vertical guide, the sentence, the time, and a chevron when a tap opens details. The live row
 * breathes and its tile is ringed.
 */
export const TreeRow = memo(function TreeRow(props: {
  readonly icon: string;
  /** The sentence; strings render muted with no emphasis, nodes bring their own. */
  readonly children: ReactNode;
  readonly live?: boolean;
  readonly failed?: boolean;
  readonly time?: string | null;
  /** Rendered only once the row is opened. Its presence makes the row tappable. */
  readonly body?: () => ReactNode;
  readonly onPress?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const anchor = useWorkAnchor("live", !!props.live);
  const expandable = Boolean(props.body);
  const press = props.onPress ?? (expandable ? () => setOpen((value) => !value) : undefined);
  return <View ref={anchor} collapsable={false}>
    <Pressable onPress={press} disabled={!press} accessibilityRole={press ? "button" : undefined} className="min-h-8 flex-row items-center gap-[9px] active:opacity-70">
      <View className={`size-[28px] items-center justify-center rounded-[9px] border bg-card ${props.live ? "border-[#7fb4ff66]" : "border-border-subtle"}`}
        style={props.live ? { shadowColor: "#7fb4ff", shadowOpacity: 0.35, shadowRadius: 4, shadowOffset: { width: 0, height: 0 } } : undefined}>
        <SymbolView name={props.icon as never} size={ICON.treeGlyph} tintColorClassName={props.failed ? "accent-danger-foreground" : "accent-foreground-muted"} />
      </View>
      {props.live
        ? <ShimmerText numberOfLines={1} className="min-w-0 flex-1 text-[13.5px] text-foreground-muted">{props.children}</ShimmerText>
        : <AppText numberOfLines={1} className={`min-w-0 flex-1 text-[13.5px] ${props.failed ? "text-danger-foreground" : "text-foreground-muted"}`}>{props.children}</AppText>}
      {props.time ? <AppText className="text-xs tabular-nums text-foreground-tertiary">{props.time}</AppText> : null}
      {press && (expandable || props.onPress) ? <SymbolView name={open ? "chevron.down" : "chevron.right"} size={11} weight="semibold" tintColorClassName="accent-foreground-tertiary" /> : null}
    </Pressable>
    {open && props.body ? <View className="mb-2 ml-[35px] mt-0.5">{props.body()}</View> : null}
  </View>;
});

/** The tree: rows over one vertical guide line. */
export function Tree(props: { readonly children: ReactNode }) {
  return <View className="relative mt-2">
    <View pointerEvents="none" className="absolute bottom-3.5 top-3.5 w-[1.5px] rounded-full bg-border-subtle" style={{ left: 12.5 }} />
    {props.children}
  </View>;
}
