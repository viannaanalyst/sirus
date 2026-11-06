import { memo, useContext, useEffect, useMemo, useState } from "react";
import { Pressable, View } from "react-native";
import Animated, { Easing, useAnimatedStyle, useSharedValue, withRepeat, withTiming } from "react-native-reanimated";

import type { ActivityItem } from "@sirus/client/types";
import { formatActivityDuration } from "@sirus/lib/agent-activity";
import { firstLaunchOffset, memberElapsed, teamHeadline, teamPhases, teamProgress, type MemberStatus, type TeamMember } from "@sirus/lib/subagent-team";

import { SymbolView } from "../AppSymbol";
import { AppText } from "../AppText";
import { ProviderIcon } from "../ProviderIcon";
import { ShimmerText } from "./Shimmer";
import { modelLabel, STATE_COLOR, stepLine, t } from "./steps";
import { OpenSubagent } from "./subagent-context";
import { useNow } from "./use-now";
import { useWorkAnchor } from "./work-anchors";

/** A pulsing status dot while the subagent runs, a still one otherwise. */
export function StatusDot(props: { readonly status: MemberStatus }) {
  const color = STATE_COLOR[props.status];
  const pulse = useSharedValue(0);
  const running = props.status === "running";
  useEffect(() => {
    if (!running) { pulse.value = 0; return; }
    pulse.value = withRepeat(withTiming(1, { duration: 1500, easing: Easing.out(Easing.quad) }), -1, false);
  }, [running, pulse]);
  const ring = useAnimatedStyle(() => ({ opacity: 0.55 * (1 - pulse.value), transform: [{ scale: 1 + 1.4 * pulse.value }] }));
  return <View className="size-2 items-center justify-center">
    {running ? <Animated.View pointerEvents="none" className="absolute size-2 rounded-full" style={[{ backgroundColor: color }, ring]} /> : null}
    <View className="size-2 rounded-full" style={{ backgroundColor: color }} />
  </View>;
}

export const statusWord = (status: MemberStatus) => t(`team.state.${status}`);
export const stepsText = (count: number) => `${count} ${count === 1 ? "passo" : "passos"}`;

/**
 * The step a subagent is on (the running one, else its last), in the Mac's words ("Executou",
 * "Leu …" as the tree reads), or nothing when no step is known.
 */
export function lastStep(item: ActivityItem | undefined): string {
  const steps = item?.steps;
  const step = steps?.findLast((entry) => entry.state === "running") ?? steps?.at(-1);
  if (!step?.label.trim()) return "";
  const line = stepLine(step);
  return [line.verb, line.target].filter(Boolean).join(" ").trim();
}

const Member = memo(function Member(props: { readonly member: TeamMember; readonly item: ActivityItem | undefined; readonly number: number; readonly now: number; readonly provider: string; readonly onOpen: () => void }) {
  const { member } = props;
  const running = member.status === "running";
  const seconds = memberElapsed(member, props.now);
  const step = lastStep(props.item);
  const summary = member.workflow ? `${member.workflow.done}/${member.workflow.total} agentes` : member.tools > 0 ? stepsText(member.tools) : "";
  const last = [step, summary].filter(Boolean).join(" · ");
  return <Pressable onPress={props.onOpen} accessibilityRole="button" accessibilityLabel={t("team.open", { name: member.label })} className="border-t border-border-subtle px-3 py-[11px] active:bg-row-hover">
    <View className="flex-row items-center gap-[7px]">
      <StatusDot status={member.status} />
      <AppText numberOfLines={1} className="shrink font-app-semibold text-sm">{member.label || t("team.untitled", { number: props.number })}</AppText>
      <AppText className="text-xs" style={{ color: STATE_COLOR[member.status] }}>{statusWord(member.status)}</AppText>
      <View className="flex-1" />
      {seconds != null ? <AppText className="text-xs tabular-nums text-foreground-tertiary">{formatActivityDuration(seconds)}</AppText> : null}
      <SymbolView name="chevron.right" size={11} weight="semibold" tintColorClassName="accent-foreground-tertiary" />
    </View>
    <View className="ml-[15px] mt-1 flex-row items-center gap-1.5">
      <ProviderIcon provider={props.provider} size={13} />
      <AppText numberOfLines={1} className="text-xs text-foreground-muted">{modelLabel(member.model, props.provider)}{member.background ? " · Em segundo plano" : ""}</AppText>
    </View>
    {last ? running
      ? <ShimmerText numberOfLines={1} className="ml-[15px] mt-[3px] text-xs text-foreground-muted">{last}</ShimmerText>
      : <AppText numberOfLines={1} className="ml-[15px] mt-[3px] text-xs text-foreground-muted">{last}</AppText> : null}
  </Pressable>;
});

/**
 * The turn's subagents as one card (`subagent_team`): a headline, "1 de 3 concluídos ·
 * 2 em execução", the elapsed time, and a row per subagent grouped in phases. A tap on a row
 * opens its sheet.
 */
export const SubagentTeam = memo(function SubagentTeam(props: {
  readonly items: readonly ActivityItem[]; readonly messageId: string; readonly provider: string;
  /** The reply so far; a plan title in it before the first launch is the card's headline. */
  readonly reply: string;
  /** Kept for callers; the headline never falls back to the person's prompt. */
  readonly prompt?: string;
}) {
  const open = useContext(OpenSubagent);
  const [collapsed, setCollapsed] = useState(false);
  const phases = useMemo(() => teamPhases(props.items), [props.items]);
  const progress = teamProgress(phases);
  const byId = useMemo(() => new Map(props.items.map((item) => [item.id, item])), [props.items]);
  const headline = useMemo(() => teamHeadline("", props.reply, firstLaunchOffset(props.items)), [props.reply, props.items]);
  const running = progress.running > 0;
  const now = useNow(running);
  const anchor = useWorkAnchor("team", running, () => setCollapsed(false));
  const members = phases.flatMap((phase) => phase.members);
  const started = Math.min(...members.map((member) => member.startedAt ?? Infinity));
  const ended = running || members.some((member) => member.endedAt == null) ? now : Math.max(...members.map((member) => member.endedAt ?? 0));
  const elapsed = Number.isFinite(started) ? formatActivityDuration(Math.max(0, Math.floor((ended - started) / 1000))) : null;
  const subtitle = [
    t("team.progress", { done: progress.done, total: progress.total }),
    running ? t("team.runningCount", { count: progress.running }) : null,
    progress.failed ? t(progress.failed === 1 ? "team.failedCount.one" : "team.failedCount", { count: progress.failed }) : null,
  ].filter(Boolean).join(" · ");
  let number = 0;
  return <View ref={anchor} collapsable={false} className="mt-3 overflow-hidden rounded-[18px] border border-border-subtle bg-card">
    <Pressable onPress={() => setCollapsed((value) => !value)} accessibilityRole="button" accessibilityLabel={collapsed ? t("team.expand") : t("team.collapse")} className="flex-row items-center gap-2.5 px-3 py-[11px] active:opacity-70">
      <View className="flex-row">
        {members.slice(0, 3).map((member, index) => <View key={member.id} className="size-[26px] items-center justify-center rounded-full border-[1.5px] border-card bg-row-hover" style={{ marginLeft: index ? -8 : 0 }}>
          <ProviderIcon provider={props.provider} size={15} />
        </View>)}
      </View>
      <View className="min-w-0 flex-1">
        <AppText numberOfLines={1} className="font-app-semibold text-[14.5px]">{headline || (progress.total === 1 ? "1 subagente" : t("team.count", { count: progress.total }))}</AppText>
        {/* As on the Mac: settled means not running (stopped counts), in a quiet tone. */}
        <AppText numberOfLines={1} className="text-xs text-foreground-muted">{subtitle}</AppText>
      </View>
      {elapsed ? <AppText className="text-xs tabular-nums text-foreground-tertiary">{elapsed}</AppText> : null}
      <SymbolView name={collapsed ? "chevron.right" : "chevron.down"} size={13} weight="semibold" tintColorClassName="accent-foreground-tertiary" />
    </Pressable>
    {collapsed ? null : phases.map((phase) => <View key={phase.index}>
      {phases.length > 1 ? <AppText className="border-t border-border-subtle px-3 pb-0.5 pt-2 text-xs text-foreground-tertiary">{t("team.phase", { number: phase.index })}</AppText> : null}
      {phase.members.map((member) => {
        number += 1;
        return <Member key={member.id} member={member} item={byId.get(member.id)} number={number} now={now} provider={props.provider} onOpen={() => open(props.messageId, member.id)} />;
      })}
    </View>)}
  </View>;
});
