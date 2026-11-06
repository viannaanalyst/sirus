import { useNavigation, type NavigationProp, type ParamListBase } from "@react-navigation/native";
import * as Haptics from "expo-haptics";
import { use, useCallback, useEffect, useMemo, useState } from "react";
import { Alert, Pressable, RefreshControl, ScrollView, View } from "react-native";
import Animated, { FadeIn, FadeOut, LinearTransition } from "react-native-reanimated";
import Svg, { Defs, LinearGradient, Path, Stop } from "react-native-svg";

import type { PendingRequest } from "@sirus/client/types";
import { AreaChart } from "../components/AreaChart";
import { SymbolView, type AppSymbolName } from "../components/AppSymbol";
import { AppText } from "../components/AppText";
import { ProjectRobot } from "../components/ProjectRobot";
import { ShimmerText } from "../components/ShimmerText";
import { computerName } from "../data/computers";
import { buildFeed, currentStep, dateLine, describeRequest, greetingFor, hourlyActivity, agoShort, turnLines, type FeedEvent } from "../data/dashboard";
import { useMacAccent, inkOn } from "../data/mac-accent";
import { prKey, usePullRequests } from "../data/pull-requests";
import { useSirus, transcriptKey } from "../data/store";
import { shortDuration, statusSince } from "../data/thread-status";
import { useLimits, useUsageSummary, type LimitEntry } from "../data/usage";
import { usePullRequestCount } from "../data/use-open-pull-requests";
import { bucketCounts, useConversationRows, type ConversationRow } from "../data/use-conversations";
import { useNow } from "../data/use-now";
import { deltaPercent, formatCost, PROVIDER_COLOR, type CostProvider, type UsageSummary } from "../data/usage-summary";
import { resetsIn } from "../data/usage-pace";
import { smoothPath, type Point } from "../lib/chart-path";
import { useUniwindTheme } from "../lib/useUniwindTheme";
import { NativeWorkspaceTabActivityContext } from "../native/v5-workspace-context";
import { useMainTabs } from "./main-tabs-context";

const GREEN = "#5fce8c";
const AMBER = "#f0a35e";

const tick = () => void Haptics.selectionAsync().catch(() => undefined);

function Card(props: { readonly children: React.ReactNode; readonly onPress?: () => void; readonly tint?: string; readonly label?: string }) {
  const style = props.tint ? { borderColor: `${props.tint}44`, backgroundColor: `${props.tint}10` } : undefined;
  const className = "mt-3 rounded-[20px] border border-border-subtle bg-grouped-card p-3.5";
  return props.onPress
    ? <Pressable onPress={() => { tick(); props.onPress!(); }} accessibilityRole="button" accessibilityLabel={props.label} className={`${className} active:opacity-80`} style={style}>{props.children}</Pressable>
    : <View className={className} style={style}>{props.children}</View>;
}

function CardHeader(props: { readonly title: string; readonly right?: React.ReactNode; readonly color?: string }) {
  return <View className="flex-row items-center justify-between">
    <AppText className="font-app-medium text-[13px] text-foreground-muted" style={props.color ? { color: props.color } : undefined}>{props.title}</AppText>
    {props.right}
  </View>;
}

/**
 * Painel: the dashboard. Greeting and connection, what needs you (answerable right here),
 * what works now, your day by hour, cost, tiles, quota limits and a feed of what is happening.
 * Every block reads real data and hides when the value is not there (no invented numbers).
 */
export function HomeScreen() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const tabs = useMainTabs();
  const theme = useUniwindTheme();
  const accent = useMacAccent();
  const active = use(NativeWorkspaceTabActivityContext)?.active ?? true;
  const computers = useSirus((state) => state.computers);
  const connection = useSirus((state) => state.connection);
  const data = useSirus((state) => state.data);
  const turns = useSirus((state) => state.turns);
  const rows = useConversationRows();
  // The page ticks every 15 s; only the working rows' clocks tick every second.
  const now = useNow(15_000, active);
  const counts = useMemo(() => bucketCounts(rows), [rows]);
  const [refreshing, setRefreshing] = useState(false);

  // ---- data
  const startOfDay = useMemo(() => { const day = new Date(now); day.setHours(0, 0, 0, 0); return day.getTime(); }, [Math.floor(now / 60_000)]); // eslint-disable-line react-hooks/exhaustive-deps
  const turnOf = (row: ConversationRow) => turns[transcriptKey(row.computerId, row.session.id)];
  const requests = useMemo(() => rows.filter((row) => row.status === "approval" || row.status === "input")
    .flatMap((row) => (row.session.pendingRequests ?? []).map((request) => ({ row, request }))), [rows]);
  const asking = useMemo(() => rows.filter((row) => (row.status === "approval" || row.status === "input") && !row.session.pendingRequests?.length), [rows]);
  const working = useMemo(() => rows.filter((row) => row.status === "working"), [rows]);
  const moments = useMemo(() => rows.flatMap((row) => {
    const turn = turns[transcriptKey(row.computerId, row.session.id)];
    return turn?.startedAt ? [Date.parse(row.session.lastActivityAt), turn.startedAt] : [Date.parse(row.session.lastActivityAt)];
  }), [rows, turns]);
  const hours = useMemo(() => hourlyActivity(moments, now), [moments, Math.floor(now / 60_000)]); // eslint-disable-line react-hooks/exhaustive-deps
  const todayConversations = useMemo(() => rows.filter((row) => Date.parse(row.session.lastActivityAt) >= startOfDay).length, [rows, startOfDay]);
  const readyToday = useMemo(() => rows.filter((row) => row.bucket === "review" && Date.parse(row.session.lastActivityAt) >= startOfDay).length, [rows, startOfDay]);
  const lines = useMemo(() => {
    let additions = 0, deletions = 0, seen = false;
    for (const turn of Object.values(turns)) {
      if (turn.startedAt < startOfDay) continue;
      const result = turnLines(turn);
      if (result) { seen = true; additions += result.additions; deletions += result.deletions; }
    }
    return seen ? { additions, deletions } : null;
  }, [turns, startOfDay]);
  const pulls = usePullRequestCount();
  const summary = useUsageSummary(7, active);
  const limits = useLimits();
  const feed = useMemo(() => buildFeed(rows.map((row) => ({
    key: `${row.computerId}:${row.session.id}`, projectName: row.project?.name ?? "Sem projeto", title: row.session.title || "Nova conversa", status: row.status,
    lastActivityAt: row.session.lastActivityAt, turn: turns[transcriptKey(row.computerId, row.session.id)],
    request: row.session.pendingRequests?.[0], done: row.bucket === "review",
  })), Date.now()), [rows, turns, Math.floor(now / 30_000)]); // eslint-disable-line react-hooks/exhaustive-deps

  // ---- the connection line
  const online = computers.filter((computer) => connection[computer.id] === "open");
  const away = computers.filter((computer) => connection[computer.id] === "lost" || connection[computer.id] === "unauthorized");
  const status = !computers.length ? ""
    : away.length ? (away.length === 1 ? `${computerName(away[0]!)} fora do ar` : `${away.length} computadores fora do ar`) + (online.length ? ` · ${online.length} conectado${online.length > 1 ? "s" : ""}` : "")
    : online.length === computers.length ? computers.length === 1 ? `${computerName(computers[0]!)} conectado` : `${computers.length} computadores conectados`
    : "Conectando…";
  const dot = away.length === computers.length ? "#ee8a96" : away.length || online.length < computers.length ? AMBER : GREEN;

  const thread = (row: ConversationRow) => navigation.navigate("Thread", { computerId: row.computerId, sessionId: row.session.id });
  const refresh = useCallback(async () => {
    setRefreshing(true);
    await Promise.all([summary.reload(), limits.reload(), ...computers.map((computer) => usePullRequests.getState().load(computer.id, "open", true))].map((job) => Promise.resolve(job).catch(() => undefined)));
    setRefreshing(false);
  }, [summary, limits, computers]);

  const [reading, setReading] = useState<number | null>(null);
  const todayTotal = hours.reduce((sum, value) => sum + value, 0);
  const date = new Date(now);

  return <ScrollView className="flex-1" contentContainerStyle={{ paddingTop: tabs?.topInset ?? 60, paddingHorizontal: 16, paddingBottom: 40 }} showsVerticalScrollIndicator={false}
    refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => void refresh()} progressViewOffset={tabs?.topInset ?? 60} />}>
    <View className="mt-1.5">
      <AppText className="font-app-bold text-[27px]">{greetingFor(date)}</AppText>
      <View className="mt-[3px] flex-row items-center gap-[7px]">
        {status ? <View className="size-[7px] rounded-full" style={{ backgroundColor: dot }} /> : null}
        <AppText className="text-[13.5px] text-foreground-muted">{status ? `${status} · ` : ""}{dateLine(date)}</AppText>
      </View>
    </View>

    {requests.length || asking.length ? <Animated.View entering={FadeIn.duration(200)} exiting={FadeOut.duration(150)} layout={LinearTransition.springify().damping(20)}>
      <Card tint={AMBER}>
        <CardHeader title="Precisa de você" color={AMBER} right={<AppText className="text-xs text-foreground-muted">{requests.length + asking.length > 1 ? `${requests.length + asking.length} pedidos` : "agora"}</AppText>} />
        {requests.slice(0, 2).map(({ row, request }) => <RequestBlock key={`${row.computerId}:${row.session.id}:${request.requestId}`} row={row} request={request} accent={accent} onOpen={() => thread(row)} />)}
        {asking.slice(0, requests.length ? 1 : 2).map((row) => <Pressable key={`${row.computerId}:${row.session.id}`} onPress={() => thread(row)} className="mt-3 flex-row items-center gap-2.5 active:opacity-70">
          <ProjectRobot project={row.project} size={30} />
          <AppText numberOfLines={2} className="flex-1 text-[14.5px] leading-[19px]"><AppText className="font-app-bold">{row.project?.name ?? "Uma conversa"}</AppText> espera por você</AppText>
          <SymbolView name="chevron.right" size={12} tintColorClassName="accent-chevron" />
        </Pressable>)}
        {requests.length + asking.length > 3 ? <Pressable onPress={() => tabs?.open("conversations", "attention")} className="mt-3 active:opacity-60"><AppText className="text-sm font-app-medium text-adaptive-blue-600-400">Ver tudo</AppText></Pressable> : null}
      </Card>
    </Animated.View> : null}

    {working.length ? <Card>
      <CardHeader title="Trabalhando agora" right={<AppText className="font-app-medium text-[13px]" style={{ color: accent }}>{working.length}</AppText>} />
      <View className="mt-1">
        {working.slice(0, 5).map((row, index) => {
          const turn = turnOf(row);
          const since = statusSince("working", turn);
          return <Pressable key={`${row.computerId}:${row.session.id}`} onPress={() => thread(row)} accessibilityRole="button" className={`flex-row items-start gap-[11px] py-[11px] active:opacity-70 ${index ? "border-t border-separator" : ""}`}>
            <ProjectRobot project={row.project} size={34} working />
            <View className="min-w-0 flex-1">
              <AppText numberOfLines={1} className="font-app-medium text-[14.5px]">{row.session.title || "Nova conversa"}</AppText>
              <ShimmerText text={currentStep(turn) ?? "Trabalhando"} />
            </View>
            {since ? <Elapsed since={since} active={active} /> : null}
          </Pressable>;
        })}
        {working.length > 5 ? <Pressable onPress={() => tabs?.open("conversations", "working")} className="pt-2 active:opacity-60"><AppText className="text-sm font-app-medium text-adaptive-blue-600-400">Ver as {working.length}</AppText></Pressable> : null}
      </View>
    </Card> : null}

    {todayTotal > 0 ? <Card>
      <CardHeader title={reading !== null ? `Às ${String(reading).padStart(2, "0")}h` : "Seu dia"} right={<AppText className="text-xs text-foreground-muted">{counts.working ? `${counts.working} ${counts.working === 1 ? "agente trabalhando" : "agentes trabalhando"}` : "conversas por hora"}</AppText>} />
      <View className="mt-1.5 flex-row items-baseline gap-1.5">
        <AppText className="font-app-bold text-[30px] tabular-nums">{reading !== null ? hours[reading] ?? 0 : todayConversations}</AppText>
        <AppText className="text-sm text-foreground-muted">{reading !== null ? ((hours[reading] ?? 0) === 1 ? "conversa" : "conversas") : todayConversations === 1 ? "conversa hoje" : "conversas hoje"}</AppText>
      </View>
      <View className="mt-2.5">
        <AreaChart series={[{ key: "day", color: accent, values: hours }]} labels={hours.map((_, index) => `${String(index).padStart(2, "0")}h`)} height={140}
          grid={theme["--color-separator"]} labelColor={theme["--color-foreground-tertiary"]} cursorColor={theme["--color-foreground"]} knockout={theme["--color-grouped-card"]}
          revision={`${todayTotal}:${startOfDay}`} drawn={new Date(now).getHours() + 1} onRead={setReading} />
      </View>
    </Card> : null}

    {summary.status === "ready" ? <CostCard summary={summary.summary} onPress={() => navigation.navigate("Cost")} /> : null}

    <Tiles readyToday={readyToday} pulls={pulls} lines={lines} onReady={() => tabs?.open("conversations", "review")} onPulls={() => pulls && navigation.navigate("PullRequests", { computerId: pulls.computerId })} />

    {limits.entries.length ? <LimitsCard entries={limits.entries} now={now} several={computers.length > 1} /> : null}

    {feed.length ? <Card>
      <CardHeader title="Acontecendo" right={<View className="size-[7px] rounded-full" style={{ backgroundColor: GREEN }} />} />
      <View className="mt-1">
        {feed.map((event, index) => <FeedRow key={event.key} event={event} first={index === 0} now={now} rows={rows} />)}
      </View>
    </Card> : null}
  </ScrollView>;
}

function Elapsed(props: { readonly since: number; readonly active: boolean }) {
  const now = useNow(1000, props.active);
  return <AppText className="text-xs tabular-nums text-foreground-muted">{shortDuration(now - props.since)}</AppText>;
}

/** One pending request with its real Approve/Deny (the conversation's `respond_agent_request`). */
function RequestBlock(props: { readonly row: ConversationRow; readonly request: PendingRequest; readonly accent: string; readonly onOpen: () => void }) {
  const { row, request } = props;
  const [busy, setBusy] = useState<"accept" | "decline" | null>(null);
  const ask = describeRequest(request);
  const send = async (decision: "accept" | "decline") => {
    if (busy) return;
    setBusy(decision);
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light).catch(() => undefined);
    try {
      await useSirus.getState().respondRequest(row.computerId, row.session.id, request, { type: "approval", decision });
    } catch (reason) {
      setBusy(null);
      Alert.alert("Não foi possível responder", reason instanceof Error ? reason.message : String(reason));
    }
  };
  return <Animated.View entering={FadeIn.duration(180)} exiting={FadeOut.duration(140)} className="mt-3">
    <Pressable onPress={props.onOpen} accessibilityRole="button" className="flex-row items-center gap-2.5 active:opacity-70">
      <ProjectRobot project={row.project} size={30} />
      <AppText numberOfLines={3} className="flex-1 text-[14.5px] leading-[19px]">
        <AppText className="font-app-bold">{row.project?.name ?? "Uma conversa"}</AppText> {ask.text}
        {ask.code ? <AppText className="font-mono text-[12.5px]"> {ask.code} </AppText> : null}
      </AppText>
    </Pressable>
    {ask.question ? <View className="mt-2.5 flex-row"><PillButton label="Responder" primary accent={props.accent} onPress={props.onOpen} /></View>
      : <View className="mt-2.5 flex-row gap-2">
        <PillButton label="Negar" busy={busy === "decline"} disabled={!!busy} onPress={() => void send("decline")} />
        <PillButton label="Aprovar" primary accent={props.accent} busy={busy === "accept"} disabled={!!busy} onPress={() => void send("accept")} />
      </View>}
  </Animated.View>;
}

export function PillButton(props: { readonly label: string; readonly onPress: () => void; readonly primary?: boolean; readonly accent?: string; readonly busy?: boolean; readonly disabled?: boolean }) {
  const fill = props.primary ? props.accent ?? "#ebebeb" : undefined;
  return <Pressable onPress={props.onPress} disabled={props.disabled} accessibilityRole="button" accessibilityLabel={props.label}
    className={`h-10 flex-1 items-center justify-center rounded-[13px] active:opacity-70 ${props.primary ? "" : "bg-subtle-strong"} ${props.disabled && !props.busy ? "opacity-40" : ""}`}
    style={fill ? { backgroundColor: fill } : undefined}>
    <AppText className="font-app-medium text-sm" style={fill ? { color: inkOn(fill) } : undefined}>{props.busy ? "…" : props.label}</AppText>
  </Pressable>;
}

function Sparkline(props: { readonly values: readonly number[]; readonly color: string }) {
  const W = 120, H = 44;
  const max = Math.max(...props.values, 0.0001);
  const points: Point[] = props.values.map((value, index) => [(index / Math.max(1, props.values.length - 1)) * W, H - 4 - (value / max) * (H - 12)]);
  if (points.length < 2) return null;
  const line = smoothPath(points);
  return <Svg viewBox={`0 0 ${W} ${H}`} style={{ flex: 1, height: 48 }}>
    <Defs><LinearGradient id="spark" x1="0" y1="0" x2="0" y2="1"><Stop offset="0" stopColor={props.color} stopOpacity={0.5} /><Stop offset="1" stopColor={props.color} stopOpacity={0} /></LinearGradient></Defs>
    <Path d={`${line}L${W} ${H}L0 ${H}Z`} fill="url(#spark)" />
    <Path d={line} fill="none" stroke={props.color} strokeWidth={2} strokeLinecap="round" />
  </Svg>;
}

function CostCard(props: { readonly summary: UsageSummary; readonly onPress: () => void }) {
  const { summary } = props;
  const totals = summary.buckets.map((bucket) => Object.values(bucket.costs).reduce<number>((sum, value) => sum + (value ?? 0), 0));
  const delta = deltaPercent(summary.totalCost, summary.previousCost);
  // The sparkline takes the colour of the provider that costs most.
  const top = (Object.keys(PROVIDER_COLOR) as CostProvider[]).reduce<{ provider: CostProvider; cost: number }>((best, provider) => {
    const cost = summary.buckets.reduce((sum, bucket) => sum + (bucket.costs[provider] ?? 0), 0);
    return cost > best.cost ? { provider, cost } : best;
  }, { provider: "claude", cost: -1 });
  return <Card onPress={props.onPress} label="Abrir custo">
    <CardHeader title="Custo · 7 dias" right={<SymbolView name={"chevron.right" as AppSymbolName} size={12} tintColorClassName="accent-chevron" />} />
    <View className="mt-1.5 flex-row items-end gap-3">
      <View>
        <AppText className="font-app-bold text-[30px] tabular-nums">{formatCost(summary.totalCost)}</AppText>
        {delta !== null ? <AppText className="text-[12.5px]"><AppText className="text-[12.5px]" style={{ color: delta <= 0 ? GREEN : AMBER }}>{delta > 0 ? "+" : delta < 0 ? "−" : ""}{Math.abs(delta)}%</AppText> <AppText className="text-[12.5px] text-foreground-muted">que a anterior</AppText></AppText> : null}
      </View>
      <Sparkline values={totals} color={PROVIDER_COLOR[top.provider]} />
    </View>
  </Card>;
}

function Tiles(props: { readonly readyToday: number; readonly pulls: { count: number; computerId: string } | null; readonly lines: { additions: number; deletions: number } | null; readonly onReady: () => void; readonly onPulls: () => void }) {
  const net = props.lines ? props.lines.additions - props.lines.deletions : 0;
  const compact = (value: number) => Math.abs(value) >= 1000 ? `${(Math.abs(value) / 1000).toFixed(1).replace(".", ",")}k` : String(Math.abs(value));
  return <View className="mt-3 flex-row gap-[9px]">
    <Tile value={String(props.readyToday)} label="Prontas hoje" onPress={props.onReady} />
    {props.pulls ? <Tile value={String(props.pulls.count)} label="PRs abertos" onPress={props.onPulls} /> : null}
    {props.lines ? <Tile value={`${net < 0 ? "−" : "+"}${compact(net)}`} label="Linhas hoje" color={net < 0 ? "#ee8a96" : GREEN} /> : null}
  </View>;
}

function Tile(props: { readonly value: string; readonly label: string; readonly color?: string; readonly onPress?: () => void }) {
  return <Pressable disabled={!props.onPress} onPress={() => { tick(); props.onPress?.(); }} accessibilityRole={props.onPress ? "button" : "text"} accessibilityLabel={`${props.label}: ${props.value}`}
    className="flex-1 rounded-[17px] border border-border-subtle bg-grouped-card p-3 active:opacity-80">
    <AppText className="font-app-bold text-2xl tabular-nums" style={props.color ? { color: props.color } : undefined}>{props.value}</AppText>
    <AppText numberOfLines={1} className="text-xs text-foreground-muted">{props.label}</AppText>
  </Pressable>;
}

function LimitsCard(props: { readonly entries: readonly LimitEntry[]; readonly now: number; readonly several: boolean }) {
  const computers = useSirus((state) => state.computers);
  const resets = props.entries.flatMap((entry) => entry.window.resetsAt ? [entry.window.resetsAt] : []);
  const next = resets.length ? Math.min(...resets) : null;
  const marker = props.entries.some((entry) => entry.pace);
  return <Card>
    <CardHeader title="Limites" right={next ? <AppText className="text-xs text-foreground-muted">renovam em {resetsIn(next, props.now)}</AppText> : null} />
    {props.entries.map((entry) => {
      const used = Math.max(0, Math.min(100, Math.round(entry.window.used ?? 0)));
      const color = entry.provider === "claude" ? PROVIDER_COLOR.claude : entry.provider === "codex" ? PROVIDER_COLOR.codex : entry.provider === "opencode" ? PROVIDER_COLOR.opencode : PROVIDER_COLOR.other;
      const computer = props.several ? computers.find((item) => item.id === entry.computerId) : undefined;
      const name = entry.provider === "opencode" ? "OpenCode" : entry.provider.charAt(0).toUpperCase() + entry.provider.slice(1);
      return <View key={entry.key} className="mt-3">
        <View className="flex-row justify-between"><AppText className="text-[13.5px]">{name}{computer ? ` · ${computerName(computer)}` : ""}</AppText><AppText className="text-[13.5px] text-foreground-muted">{used}%</AppText></View>
        <View className="mt-[7px] h-[7px] rounded-full bg-subtle-strong">
          <View className="absolute inset-y-0 left-0 rounded-full" style={{ width: `${used}%`, backgroundColor: color }} />
          {entry.pace ? <View className="absolute -inset-y-[3px] w-0.5 rounded-sm bg-foreground" style={{ left: `${Math.round(entry.pace.elapsed * 100)}%` }} /> : null}
        </View>
      </View>;
    })}
    {marker ? <AppText className="mt-[9px] text-xs text-foreground-tertiary">O traço branco é o ritmo uniforme da semana.</AppText> : null}
  </Card>;
}

function FeedRow(props: { readonly event: FeedEvent; readonly first: boolean; readonly now: number; readonly rows: readonly ConversationRow[] }) {
  const { event } = props;
  const key = event.key.replace(/:(ask|work|fail|done)$/, "");
  const row = props.rows.find((item) => `${item.computerId}:${item.session.id}` === key);
  const tone = (name?: string) => name === "add" ? GREEN : name === "del" ? "#ee8a96" : name === "danger" ? "#ee8a96" : undefined;
  return <View className={`flex-row items-start gap-2.5 py-[9px] ${props.first ? "" : "border-t border-separator"}`}>
    <ProjectRobot project={row?.project} size={22} working={event.working} />
    <AppText className="flex-1 text-[13.5px] leading-[18px]">
      {event.parts.map((part, index) => <AppText key={index} className={`text-[13.5px] ${part.strong ? "font-app-bold" : ""}`} style={tone(part.tone) ? { color: tone(part.tone) } : undefined}>{part.text}</AppText>)}
    </AppText>
    <AppText className="pl-2 text-xs text-foreground-tertiary">{agoShort(event.at, props.now)}</AppText>
  </View>;
}
