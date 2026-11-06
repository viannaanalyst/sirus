import { Image, Pressable, ScrollView, View } from "react-native";

import { modeChips, type ComposerModes } from "@sirus/lib/composer-modes";

import { SymbolView } from "../AppSymbol";
import { AppText } from "../AppText";

/** One attachment in the composer: a picture shows as a thumbnail, anything else as a chip. */
export interface StripAttachment {
  readonly key: string;
  readonly name: string;
  readonly image: boolean;
  /** A small picture to show (a data URL from the computer, or the phone's own file). */
  readonly uri?: string;
  readonly onRemove: () => void;
}

const THUMB = 60;

function Thumb(props: { readonly item: StripAttachment }) {
  const { item } = props;
  return <View style={{ width: THUMB + 6, height: THUMB + 6, paddingTop: 6, paddingRight: 6 }}>
    <View className="overflow-hidden rounded-[14px] border border-border-subtle bg-card" style={{ width: THUMB, height: THUMB }}>
      <Image source={{ uri: item.uri }} style={{ width: THUMB, height: THUMB }} resizeMode="cover" accessibilityLabel={item.name} />
    </View>
    <Pressable onPress={item.onRemove} hitSlop={8} accessibilityRole="button" accessibilityLabel={`Remover ${item.name}`}
      className="absolute right-0 top-0 size-[22px] items-center justify-center rounded-full border border-border-subtle bg-sheet active:opacity-60">
      <SymbolView name="xmark" size={10} weight="bold" tintColorClassName="accent-foreground" />
    </Pressable>
  </View>;
}

function FileChip(props: { readonly item: StripAttachment }) {
  const { item } = props;
  return <View className="h-8 max-w-[180px] flex-row items-center gap-1.5 self-end rounded-full bg-card pl-2.5 pr-1">
    <SymbolView name={item.image ? "photo" : "doc"} size={13} tintColorClassName="accent-foreground-muted" />
    <AppText numberOfLines={1} className="shrink text-xs">{item.name}</AppText>
    <Pressable onPress={item.onRemove} hitSlop={8} accessibilityRole="button" accessibilityLabel={`Remover ${item.name}`} className="size-6 items-center justify-center rounded-full active:bg-row-hover">
      <SymbolView name="xmark" size={10} weight="semibold" tintColorClassName="accent-foreground-muted" />
    </Pressable>
  </View>;
}

/**
 * Above the field: the attached files (pictures as thumbnails, with remove) and the active
 * modes as chips (planning, debug, team, goal), each removable.
 */
export function ComposerStrip(props: {
  readonly attachments: readonly StripAttachment[];
  readonly modes: ComposerModes;
  readonly onRemoveMode: (id: "planning" | "debugging" | "team" | "goal") => void;
  readonly children?: React.ReactNode;
}) {
  const chips = modeChips(props.modes);
  if (!props.attachments.length && !chips.length && !props.children) return null;
  return <ScrollView horizontal showsHorizontalScrollIndicator={false} keyboardShouldPersistTaps="always" contentContainerClassName="items-end gap-1.5 px-1.5 pb-1 pt-1">
    {props.attachments.map((item) => item.image && item.uri ? <Thumb key={item.key} item={item} /> : <FileChip key={item.key} item={item} />)}
    {props.children}
    {chips.map((chip) => <View key={chip.id} className="h-8 max-w-[220px] flex-row items-center gap-1.5 self-end rounded-full bg-subtle-strong pl-2.5 pr-1">
      <SymbolView name={chip.icon as never} size={13} tintColorClassName="accent-foreground" />
      <AppText numberOfLines={1} className="shrink text-xs">{chip.label}</AppText>
      <Pressable onPress={() => props.onRemoveMode(chip.id)} hitSlop={8} accessibilityRole="button" accessibilityLabel={`Remover ${chip.id === "goal" ? "meta" : chip.label}`}
        className="size-6 items-center justify-center rounded-full active:bg-row-hover">
        <SymbolView name="xmark" size={10} weight="semibold" tintColorClassName="accent-foreground-muted" />
      </Pressable>
    </View>)}
  </ScrollView>;
}
