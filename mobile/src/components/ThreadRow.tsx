import { memo, useEffect, useState } from "react";
import { Alert, Pressable, StyleSheet, View } from "react-native";

import { translate } from "@sirus/i18n";
import { rowMenuItems } from "@sirus/lib/phone-home";
import { relativeTime } from "@sirus/lib/session-board";
import { canSnooze, phoneSnoozeChoices, snoozeDeadline } from "@sirus/lib/snooze";
import type { MenuAction } from "@react-native-menu/menu";
import * as Clipboard from "expo-clipboard";

import { SymbolView } from "./AppSymbol";
import { AppText } from "./AppText";
import { ControlPillMenu } from "./ControlPillMenu";
import { ProjectRobot } from "./ProjectRobot";
import { ProviderIcon } from "./ProviderIcon";
import { isScratch, projectLabel } from "../data/scratch";
import { transcriptKey, useSirus, type RowAction, type SessionRow } from "../data/store";
import { shortDuration, statusSince, THREAD_STATUS_LABEL, threadStatus } from "../data/thread-status";

const ACTIVE = new Set(["starting", "running", "waiting"]);
const t = (key: string, params?: Record<string, string | number>) => translate("pt-BR", key, params);
const failed = (title: string) => (reason: unknown) => Alert.alert(title, reason instanceof Error ? reason.message : String(reason));

/**
 * A conversation row: agent mark, title and time; project, branch and computer
 * below. A long press lifts it into iOS's context menu with the phone web app's actions
 * (`rowMenuItems`): copy the id, snooze, arrange, pin, rename, delete.
 */
/** A row's status: its icon and label, and how long it has been so (ticking). */
export function StatusLabel(props: { readonly status: keyof typeof THREAD_STATUS_LABEL; readonly since: number | null }) {
  const label = THREAD_STATUS_LABEL[props.status];
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (props.since === null) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [props.since]);
  return <View className="flex-row items-center gap-1">
    <SymbolView name={label.icon as never} size={13} weight="semibold" tintColorClassName={label.iconTintClassName as never} />
    <AppText className={`font-app-bold text-xs ${label.className}`}>{label.label}</AppText>
    {props.since !== null ? <AppText className={`text-xs tabular-nums ${label.className}`}>{shortDuration(now - props.since)}</AppText> : null}
  </View>;
}

/**
 * A conversation row: the agent's mark, the title and its time; the state
 * (approval, question, working, waiting, failed, with how long) and the project, branch and
 * computer below. A long press lifts it into iOS's context menu with the phone web app's actions
 * (`rowMenuItems`): copy the id, snooze, arrange, pin, rename, delete.
 * `grouped` draws it on a grouped card (Início, a project's page) instead of the plain list.
 */
export const ThreadRow = memo(function ThreadRow(props: { readonly row: SessionRow; readonly computerLabel: string | null; readonly onPress: () => void; readonly grouped?: boolean; readonly separated?: boolean; /** Draws the project's robot (hopping while it works) where the agent's mark goes. */ readonly robot?: boolean }) {
  const { session, project, computerId, pinned } = props.row;
  const working = ACTIVE.has(session.status);
  const turn = useSirus((state) => state.turns[transcriptKey(computerId, session.id)]);
  const status = threadStatus(session, turn, props.row.childRequest);
  const order = useSirus((state) => state.prefs.threadOrder);
  const time = relativeTime(session.lastActivityAt);
  const details = [projectLabel(project), isScratch(project) ? null : session.worktree?.branch, props.computerLabel].filter(Boolean).join(" · ");
  const items = rowMenuItems({ place: "home", pinned, active: working, order });
  const run = (action: RowAction, title: string) => void useSirus.getState().rowAction(computerId, session.id, action).catch(failed(title));
  const labels: Record<string, string> = {
    "copy-id": t("phoneHome.copyId"), return: t("snooze.return"),
    arrange: t("phoneHome.arrange"), pin: t("mobile.pin"), unpin: t("mobile.unpin"), rename: t("phoneHome.rename"),
    delete: t("mobile.delete"),
  };
  const symbols: Record<string, string> = {
    "copy-id": "doc.on.doc", arrange: "arrow.up.arrow.down", pin: "pin", unpin: "pin.slash",
    rename: "square.and.pencil", delete: "trash",
  };
  // Order: copy, snooze (each choice with its time), pin, rename, delete.
  const now = new Date();
  const actions: MenuAction[] = items.filter((item) => item.id !== "arrange").flatMap((item): MenuAction[] => {
    const base: MenuAction = { id: item.id, title: labels[item.id] ?? item.id, image: symbols[item.id], attributes: { destructive: item.destructive, disabled: item.disabled } };
    return item.id === "copy-id" ? [base, {
      id: "snooze", title: t("snooze.confirm"), image: "clock", attributes: { disabled: !canSnooze(session) },
      subactions: phoneSnoozeChoices(now).map((choice) => ({ id: `snooze:${choice.at.toISOString()}`, title: t(`snooze.preset.${choice.preset}`), subtitle: snoozeDeadline(choice.at, now, "pt-BR", t) })),
    }] : [base];
  });
  const onAction = (id: string) => {
    if (id === "copy-id") void Clipboard.setStringAsync(session.id);
    else if (id === "pin" || id === "unpin") run({ type: id }, labels[id]!);
    else if (id.startsWith("snooze:")) run({ type: "snooze", until: id.slice("snooze:".length) }, t("snooze.confirm"));
    else if (id.startsWith("arrange:")) useSirus.getState().setPrefs({ threadOrder: id.endsWith("created_at") ? "created_at" : "updated_at" });
    else if (id === "rename") Alert.prompt(t("phoneHome.rename"), undefined, [
      { text: "Cancelar", style: "cancel" },
      { text: "Salvar", onPress: (value?: string) => { if (value?.trim() && value.trim() !== session.title) run({ type: "rename", title: value }, labels.rename!); } },
    ], "plain-text", session.title);
    else if (id === "delete") Alert.alert(`Excluir "${session.title || "conversa"}"?`, "A conversa some deste computador. Não dá para desfazer.", [
      { text: "Cancelar", style: "cancel" },
      ...(session.worktree?.isolated ? [{ text: "Excluir e remover o worktree", style: "destructive" as const, onPress: () => run({ type: "delete", removeWorktree: true }, labels.delete!) }] : []),
      { text: "Excluir", style: "destructive", onPress: () => run({ type: "delete", removeWorktree: false }, labels.delete!) },
    ]);
  };
  return <ControlPillMenu actions={actions} shouldOpenOnLongPress onPressAction={({ nativeEvent }) => onAction(nativeEvent.event)}>
    <Pressable onPress={props.onPress} className={`${props.grouped ? "bg-grouped-card" : "bg-thread-canvas"} active:bg-row-hover`}>
      <View className="flex-row items-center gap-3 py-2.5 pl-4 pr-4">
        {props.robot ? <ProjectRobot project={project} size={30} working={status === "working"} /> : <View className="size-7 items-center justify-center">
          <ProviderIcon provider={session.agent} size={20} />
        </View>}
        <View className="min-w-0 flex-1 gap-0.5">
          <View className="flex-row items-center gap-2">
            <AppText numberOfLines={1} className="flex-1 font-app-medium text-base">{session.title || "Nova conversa"}</AppText>
            {pinned ? <SymbolView name="pin.fill" size={11} tintColorClassName="accent-foreground-tertiary" /> : null}
            <AppText className="text-xs tabular-nums text-foreground-tertiary">{status && statusSince(status, turn) !== null ? "" : time === "now" ? "agora" : time}</AppText>
          </View>
          <View className="flex-row items-center gap-2">
            {props.robot && !status ? <ProviderIcon provider={session.agent} size={13} /> : null}
            {status ? <StatusLabel status={status} since={statusSince(status, turn)} /> : null}
            {details ? <AppText numberOfLines={1} className="shrink text-xs text-foreground-muted">{details}</AppText> : null}
          </View>
        </View>
      </View>
      {props.separated ? <View className="absolute left-[56px] right-0 top-0 bg-separator" style={{ height: StyleSheet.hairlineWidth }} /> : null}
    </Pressable>
  </ControlPillMenu>;
});

