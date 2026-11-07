import { memo, useState, type ReactNode } from "react";
import { Pressable, View } from "react-native";

import { SymbolView } from "../AppSymbol";
import { AppText } from "../AppText";
import { ShimmerText } from "./Shimmer";
import { useWorkAnchor } from "./work-anchors";
import { FileIcon } from "../FileIcon";

const MONO = { fontFamily: "Menlo" } as const;

/** A small mono chip: the command's program, or what a step acted on. */
export function TargetChip(props: { readonly children: string }) {
  return <View className="shrink rounded-[5px] bg-card px-[5px]">
    <AppText numberOfLines={1} style={MONO} className="text-[11px] leading-[18px] text-foreground-muted">{props.children}</AppText>
  </View>;
}

/** A command's path chip: the file or folder icon and its name. */
export function PathChip(props: { readonly name: string; readonly folder: boolean }) {
  return <View className="shrink flex-row items-center gap-1 rounded-[6px] bg-card px-1.5">
    <FileIcon name={props.name} directory={props.folder} size={12} />
    <AppText numberOfLines={1} style={MONO} className="shrink text-[11px] leading-[20px] text-foreground-muted">{props.name}</AppText>
  </View>;
}

/**
 * One row of a work group, as on the Mac: a hook off the group's rail, what the model said it was
 * for, and chips. A tap opens details when there are any. The live row shimmers.
 */
export const TreeRow = memo(function TreeRow(props: {
  /** A quiet icon before the sentence; commands have none. */
  readonly icon?: string;
  readonly title?: string;
  readonly chips?: ReactNode;
  readonly live?: boolean;
  readonly failed?: boolean;
  /** Rendered only once the row is opened. Its presence makes the row tappable. */
  readonly body?: () => ReactNode;
  readonly onPress?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const anchor = useWorkAnchor("live", !!props.live);
  const press = props.onPress ?? (props.body ? () => setOpen((value) => !value) : undefined);
  const tone = props.failed ? "text-danger-foreground" : "text-foreground-muted";
  return <View ref={anchor} collapsable={false}>
    <Pressable onPress={press} disabled={!press} accessibilityRole={press ? "button" : undefined}
      className="min-h-[30px] flex-row items-center gap-[7px] pl-[26px] active:opacity-60">
      <View pointerEvents="none" className="absolute border-b border-l border-border-subtle" style={{ left: 12, top: -14, height: 29, width: 10, borderBottomLeftRadius: 6 }} />
      {props.icon ? <SymbolView name={props.icon as never} size={14} tintColorClassName={props.failed ? "accent-danger-foreground" : "accent-foreground-muted"} /> : null}
      {props.title ? (props.live
        ? <ShimmerText numberOfLines={1} className={`shrink text-[13.5px] ${tone}`}>{props.title}</ShimmerText>
        : <AppText numberOfLines={1} className={`shrink text-[13.5px] ${tone}`}>{props.title}</AppText>) : null}
      {props.chips}
    </Pressable>
    {open && props.body ? <View className="mb-2 ml-[26px] mt-0.5">{props.body()}</View> : null}
  </View>;
});

/** Rows outside a group, on the same rail. */
export function Tree(props: { readonly children: ReactNode }) {
  return <View className="mt-1">{props.children}</View>;
}
