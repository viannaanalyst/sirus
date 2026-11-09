import * as Haptics from "expo-haptics";
import { Pressable, View } from "react-native";
import Animated, { FadeIn, FadeOut, LinearTransition } from "react-native-reanimated";

import { SymbolView, type AppSymbolName } from "./AppSymbol";
import { AppText } from "./AppText";
import type { MainTab } from "../screens/main-tabs-context";
import { ICON } from "../lib/icon-sizes";

const SEGMENTS: readonly { readonly tab: MainTab; readonly label: string; readonly icon: string }[] = [
  { tab: "new", label: "Novo", icon: "square.and.pencil" },
  { tab: "conversations", label: "Conversas", icon: "bubble.left.and.bubble.right" },
  { tab: "dashboard", label: "Painel", icon: "chart.bar.xaxis" },
];

const tick = () => void Haptics.selectionAsync().catch(() => undefined);

/** A round glass button of the top bar. */
export function RoundButton(props: { readonly icon: string; readonly label: string; readonly onPress: () => void; readonly badge?: number }) {
  return <Pressable onPress={() => { tick(); props.onPress(); }} accessibilityRole="button" accessibilityLabel={props.label}
    className="size-12 items-center justify-center rounded-full border border-border-subtle bg-glass-fallback active:opacity-60">
    <SymbolView name={props.icon as AppSymbolName} size={ICON.bar} weight="medium" tintColorClassName="accent-foreground" />
    {props.badge ? <View className="absolute -right-0.5 -top-0.5 min-w-[17px] items-center rounded-full bg-[#f0a35e] px-1">
      <AppText className="font-app-bold text-[11px] leading-[17px] text-[#1a1208]">{props.badge}</AppText>
    </View> : null}
  </Pressable>;
}

/**
 * The top bar of the design: the sidebar button, a pill with Novo, Conversas and Painel (the
 * chosen one shows its name), and the settings button.
 */
export function TopBar(props: { readonly selected: MainTab; readonly onSelect: (tab: MainTab) => void; readonly onSidebar: () => void; readonly onSettings: () => void; readonly waiting: number }) {
  return <View className="flex-row items-center justify-between gap-2">
    <RoundButton icon="sidebar.left" label="Projetos" onPress={props.onSidebar} badge={props.waiting} />
    <View className="flex-row gap-0.5 rounded-3xl border border-border-subtle bg-glass-fallback p-[3px]">
      {SEGMENTS.map((segment) => {
        const on = segment.tab === props.selected;
        return <Animated.View key={segment.tab} layout={LinearTransition.springify().damping(22).stiffness(260)}>
          <Pressable onPress={() => { if (!on) { tick(); props.onSelect(segment.tab); } }} accessibilityRole="tab" accessibilityState={{ selected: on }} accessibilityLabel={segment.label}
            className={`h-[42px] flex-row items-center gap-2 rounded-[21px] ${on ? "bg-subtle-strong px-4" : "px-3.5"}`}>
            <SymbolView name={segment.icon as AppSymbolName} size={ICON.segment} weight="medium" tintColorClassName={on ? "accent-foreground" : "accent-foreground-muted"} />
            {on ? <Animated.View entering={FadeIn.duration(160)} exiting={FadeOut.duration(80)}><AppText className="font-app-medium text-[16px]">{segment.label}</AppText></Animated.View> : null}
          </Pressable>
        </Animated.View>;
      })}
    </View>
    <RoundButton icon="gearshape" label="Ajustes" onPress={props.onSettings} />
  </View>;
}
