import type { ReactNode } from "react";
import { Pressable, ScrollView, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { SymbolView, type AppSymbolName } from "./AppSymbol";
import { AppText } from "./AppText";
import { RoundButton } from "./TopBar";
import { ICON } from "../lib/icon-sizes";

/**
 * The sheet of the approved design (tpl2 `.sheet .shd .gr`): a rounded-top card over black with a
 * round close button, a centred title with an optional small line under it, and a scroll body.
 * `footer` floats over the bottom (a fixed search field, for instance).
 */
export function SheetScreen(props: {
  readonly title: string; readonly subtitle?: string; readonly onClose: () => void; readonly closeIcon?: string; readonly closeLabel?: string;
  readonly right?: ReactNode; readonly children: ReactNode; readonly footer?: ReactNode; /** Inside a native form sheet, which already draws the rounded card and grabber. */ readonly embedded?: boolean; readonly bottomPadding?: number;
}) {
  const insets = useSafeAreaInsets();
  const embedded = !!props.embedded;
  return <View className={`flex-1 ${embedded ? "bg-sheet" : "bg-screen"}`}>
    <View className={`flex-1 overflow-hidden bg-sheet ${embedded ? "" : "rounded-t-[30px]"}`} style={{ marginTop: embedded ? 0 : insets.top + 4 }}>
      <ScrollView showsVerticalScrollIndicator={false} keyboardShouldPersistTaps="handled" contentContainerStyle={{ paddingHorizontal: 16, paddingTop: embedded ? 24 : 14, paddingBottom: (props.bottomPadding ?? 0) + Math.max(insets.bottom, 18) + 18 }}>
        <View className="mb-1.5 flex-row items-center">
          <RoundButton icon={props.closeIcon ?? "xmark"} label={props.closeLabel ?? "Fechar"} onPress={props.onClose} />
          <View className="min-w-0 flex-1 px-2">
            <AppText numberOfLines={1} className="text-center font-app-medium text-base">{props.title}</AppText>
            {props.subtitle ? <AppText numberOfLines={1} className="text-center text-[12.5px] text-foreground-muted">{props.subtitle}</AppText> : null}
          </View>
          {props.right ?? <View className="size-11" />}
        </View>
        {props.children}
      </ScrollView>
      {props.footer}
    </View>
  </View>;
}

/** A section label above a group (`.sec`). */
export function SheetSection(props: { readonly children: string }) {
  return <AppText className="mb-1.5 mt-5 px-3 text-[13px] text-foreground-muted">{props.children}</AppText>;
}

/** A round-cornered group of rows (`.gr`), hairlines between them. */
export function SheetGroup(props: { readonly children: ReactNode; readonly className?: string; readonly plain?: boolean }) {
  return <View className={`overflow-hidden rounded-[17px] px-[13px] ${props.plain ? "" : "bg-grouped-card"} ${props.className ?? ""}`}>{props.children}</View>;
}

/** One row of a group: leading icon, title (and small line), value, chevron. */
export function SheetRow(props: {
  readonly icon?: string; readonly lead?: ReactNode; readonly title: string; readonly note?: string; readonly noteColor?: string; readonly value?: string; readonly accessory?: ReactNode;
  readonly chevron?: boolean; readonly destructive?: boolean; readonly first?: boolean; readonly bold?: boolean; readonly disabled?: boolean; readonly onPress?: () => void;
}) {
  return <Pressable onPress={props.onPress} disabled={!props.onPress || props.disabled}
    className={`flex-row items-center gap-[11px] py-3 active:opacity-60 ${props.first ? "" : "border-t border-border-subtle"} ${props.disabled ? "opacity-40" : ""}`}>
    {props.lead ?? (props.icon ? <SymbolView name={props.icon as AppSymbolName} size={ICON.sheetRow} weight="medium" tintColorClassName={props.destructive ? "accent-danger-foreground" : "accent-foreground"} /> : null)}
    <View className="min-w-0 flex-1">
      <AppText numberOfLines={1} className={`text-[15px] ${props.bold ? "font-app-medium" : ""} ${props.destructive ? "text-danger-foreground" : ""}`}>{props.title}</AppText>
      {props.note ? <AppText numberOfLines={2} className="text-[12.5px] text-foreground-muted" style={props.noteColor ? { color: props.noteColor } : undefined}>{props.note}</AppText> : null}
    </View>
    {props.value ? <AppText numberOfLines={1} className="max-w-[45%] text-sm text-foreground-muted">{props.value}</AppText> : null}
    {props.accessory}
    {props.chevron ? <SymbolView name="chevron.right" size={13} weight="semibold" tintColorClassName="accent-foreground-muted" /> : null}
  </Pressable>;
}
