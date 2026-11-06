import { ICON } from "../lib/icon-sizes";
import type { ReactNode } from "react";
import { Pressable, Switch, View } from "react-native";

import { SymbolView } from "./AppSymbol";
import { AppText } from "./AppText";

/**
 * The grouped list of iOS Settings: a titled card of rows with
 * hairlines between them, an optional footnote.
 */
export function SettingsGroup(props: { readonly title?: string; readonly footer?: string; readonly children: ReactNode }) {
  return <View className="gap-1.5">
    {props.title ? <AppText className="px-4 text-xs text-foreground-muted">{props.title}</AppText> : null}
    <View className="overflow-hidden rounded-2xl bg-card">{props.children}</View>
    {props.footer ? <AppText className="px-4 text-xs leading-4 text-foreground-tertiary">{props.footer}</AppText> : null}
  </View>;
}

function Divider(props: { readonly inset: boolean }) {
  return <View className={`absolute right-0 top-0 h-px bg-border-subtle ${props.inset ? "left-[52px]" : "left-4"}`} />;
}

export function SettingsRow(props: {
  readonly icon?: string; readonly title: string; readonly subtitle?: string; readonly value?: string; readonly first?: boolean;
  readonly onPress?: () => void; readonly chevron?: boolean; readonly destructive?: boolean; readonly accessory?: ReactNode; readonly disabled?: boolean;
}) {
  return <Pressable onPress={props.onPress} disabled={!props.onPress || props.disabled} className="min-h-[48px] flex-row items-center gap-3 px-4 py-2.5 active:bg-row-hover">
    {!props.first ? <Divider inset={!!props.icon} /> : null}
    {props.icon ? <View className="size-7 items-center justify-center">
      <SymbolView name={props.icon as never} size={ICON.sheetRow} tintColorClassName={props.destructive ? "accent-danger-foreground" : "accent-icon"} />
    </View> : null}
    <View className={`min-w-0 flex-1 ${props.disabled ? "opacity-40" : ""}`}>
      <AppText numberOfLines={2} className={`text-base ${props.destructive ? "text-danger-foreground" : ""}`}>{props.title}</AppText>
      {props.subtitle ? <AppText numberOfLines={3} className="text-xs leading-4 text-foreground-muted">{props.subtitle}</AppText> : null}
    </View>
    {props.value ? <AppText numberOfLines={1} className="max-w-[45%] text-sm text-foreground-muted">{props.value}</AppText> : null}
    {props.accessory}
    {props.chevron ? <SymbolView name="chevron.right" size={12} tintColorClassName="accent-foreground-tertiary" /> : null}
  </Pressable>;
}

export function SettingsSwitch(props: {
  readonly icon?: string; readonly title: string; readonly subtitle?: string; readonly first?: boolean;
  readonly value: boolean; readonly onChange: (value: boolean) => void; readonly disabled?: boolean;
}) {
  return <SettingsRow icon={props.icon} title={props.title} subtitle={props.subtitle} first={props.first} disabled={props.disabled}
    accessory={<Switch value={props.value} onValueChange={props.onChange} disabled={props.disabled} />} />;
}

/** One choice among several, each with its line of help; the chosen one checked. */
export function SettingsChoices<T extends string>(props: {
  readonly value: T; readonly onChange: (value: T) => void; readonly disabled?: boolean;
  readonly options: readonly { readonly id: T; readonly title: string; readonly help?: string }[];
}) {
  return <>{props.options.map((option, index) => <SettingsRow key={option.id} first={index === 0} title={option.title} subtitle={option.help} disabled={props.disabled}
    onPress={() => option.id !== props.value && props.onChange(option.id)}
    accessory={option.id === props.value ? <SymbolView name="checkmark" size={14} weight="semibold" tintColorClassName="accent-icon" /> : <View className="w-[14px]" />} />)}</>;
}
