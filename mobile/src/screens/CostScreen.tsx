import { SegmentedControl } from "../components/SegmentedControl";
import { useNavigation, type NavigationProp, type ParamListBase } from "@react-navigation/native";
import * as Haptics from "expo-haptics";
import { useMemo, useState } from "react";
import { ActivityIndicator, Pressable, RefreshControl, ScrollView, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { SymbolView, type AppSymbolName } from "../components/AppSymbol";
import { AppText } from "../components/AppText";
import { AreaChart } from "../components/AreaChart";
import { modelDisplayName } from "../data/model-name";
import { RoundButton } from "../components/TopBar";
import { computerName } from "../data/computers";
import { useSirus } from "../data/store";
import { useLimits, useUsageSummary, type LimitEntry } from "../data/usage";
import { resetsIn } from "../data/usage-pace";
import {
  bucketLabel, bucketReadout, COST_PROVIDERS, deltaPercent, formatCost, formatTokens, PROVIDER_COLOR, PROVIDER_NAME, providerTotals,
  type CostProvider, type UsageSummary,
} from "../data/usage-summary";
import { useUniwindTheme } from "../lib/useUniwindTheme";

type Segment = "costs" | "tokens" | "limits";
const SEGMENTS: readonly { id: Segment; label: string }[] = [{ id: "costs", label: "Custo" }, { id: "tokens", label: "Tokens" }, { id: "limits", label: "Limites" }];
const RANGES: readonly { days: number; label: string; title: string; previous: string }[] = [
  { days: 1, label: "24h", title: "Últimas 24 horas", previous: "que o dia anterior" },
  { days: 7, label: "7d", title: "Últimos 7 dias", previous: "que a semana anterior" },
  { days: 30, label: "30d", title: "Últimos 30 dias", previous: "que os 30 dias anteriores" },
  { days: 90, label: "90d", title: "Últimos 90 dias", previous: "que os 90 dias anteriores" },
];
/** Bottom to top, so Claude (usually the biggest) sits on top as in the design. */
const STACK: readonly CostProvider[] = ["opencode", "other", "codex", "claude"];
const AMBER = "#e9b44c";

function SegmentBar<T extends string>(props: { readonly options: readonly { id: T; label: string }[]; readonly value: T; readonly onChange: (id: T) => void; readonly className?: string }) {
  return <SegmentedControl options={props.options.map((option) => ({ value: option.id, label: option.label }))} selected={props.value} onSelect={(id) => { if (id !== props.value) { void Haptics.selectionAsync().catch(() => undefined); props.onChange(id); } }} className={props.className} />;
}

/**
 * Custo: what the agents cost, by provider, from the computers' `usage_summary`. A stacked
 * smooth area you can drag across to read one bucket, 24h to 90d, the models that cost most,
 * and the weekly limit with its pace. Tokens swaps the measure; Limites lists the quotas.
 */
export function CostScreen() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const insets = useSafeAreaInsets();
  const theme = useUniwindTheme();
  const [segment, setSegment] = useState<Segment>("costs");
  const [range, setRange] = useState(1);
  const [reading, setReading] = useState<number | null>(null);
  const [refreshing, setRefreshing] = useState(false);
  const days = RANGES[range]!.days;
  const state = useUsageSummary(days);
  const limits = useLimits();
  const measure = segment === "tokens" ? "tokens" : "costs";

  const summary = state.status === "ready" ? state.summary : null;
  const chart = useMemo(() => summary ? buildChart(summary, measure, days) : null, [summary, measure, days]);
  const totals = useMemo(() => summary ? providerTotals(summary, measure) : null, [summary, measure]);
  const total = summary ? (measure === "costs" ? summary.totalCost : summary.totalTokens) : 0;
  const money = (value: number) => measure === "costs" ? formatCost(value) : formatTokens(value);
  const delta = summary && measure === "costs" ? deltaPercent(summary.totalCost, summary.previousCost) : null;
  const readIndex = reading !== null && chart ? Math.min(reading, chart.totals.length - 1) : null;

  const refresh = async () => { setRefreshing(true); state.reload(); limits.reload(); setTimeout(() => setRefreshing(false), 600); };

  return <View className="flex-1 bg-screen">
    <ScrollView contentContainerStyle={{ paddingTop: insets.top + 6, paddingHorizontal: 16, paddingBottom: insets.bottom + 40 }} showsVerticalScrollIndicator={false}
      refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => void refresh()} />}>
      <View className="flex-row items-center justify-between">
        <RoundButton icon="chevron.left" label="Voltar" onPress={() => navigation.goBack()} />
        <AppText className="font-app-medium text-base">Custo</AppText>
        <View className="size-11" />
      </View>
      <SegmentBar className="mt-3" options={SEGMENTS} value={segment} onChange={(id) => { setSegment(id); setReading(null); }} />

      {segment === "limits" ? <LimitsList entries={limits.entries} /> : state.status === "loading" ? <ActivityIndicator className="mt-24" />
        : state.status === "unavailable" || !summary || !chart || !totals ? <Unavailable onRetry={state.reload} />
        : <>
          <View className="mt-[18px]">
            <AppText className="text-[13px] text-foreground-muted">{readIndex !== null ? bucketReadout(summary.buckets[readIndex]!.at, days) : RANGES[range]!.title}</AppText>
            <AppText className="font-app-bold text-[46px] leading-[54px] tabular-nums">{money(readIndex !== null ? chart.totals[readIndex]! : total)}</AppText>
            {readIndex === null && delta !== null ? <AppText className="mt-[3px] text-[13px]">
              <AppText className="text-[13px]" style={{ color: delta <= 0 ? "#5fce8c" : AMBER }}>{delta > 0 ? "+" : delta < 0 ? "−" : ""}{Math.abs(delta)}%</AppText>
              <AppText className="text-[13px] text-foreground-muted"> {RANGES[range]!.previous}</AppText>
            </AppText> : null}
          </View>
          {chart.series.length && chart.totals.some((value) => value > 0) ? <View className="-mx-4 mt-3 px-4">
            <AreaChart series={chart.series} labels={chart.labels} height={190} grid={theme["--color-separator"]} labelColor={theme["--color-foreground-tertiary"]}
              cursorColor={theme["--color-foreground"]} knockout={theme["--color-screen"]} revision={`${segment === "tokens" ? "t" : "c"}:${days}:${chart.totals.length}:${Math.round(total)}`} onRead={setReading} />
          </View> : <AppText className="mt-10 text-center text-sm text-foreground-muted">Sem uso neste período.</AppText>}
          <SegmentBar className="mt-1.5" options={RANGES.map((item, index) => ({ id: String(index), label: item.label }))} value={String(range)} onChange={(id) => { setRange(Number(id)); setReading(null); }} />
          <View className="mt-2.5 flex-row flex-wrap gap-x-[13px] gap-y-1">
            {COST_PROVIDERS.filter((provider) => totals[provider] > 0).map((provider) => <View key={provider} className="flex-row items-center">
              <View className="mr-[5px] size-2 rounded-[3px]" style={{ backgroundColor: PROVIDER_COLOR[provider] }} />
              <AppText className="text-xs text-foreground-secondary">{PROVIDER_NAME[provider]} {money(readIndex !== null ? chart.byProvider[provider]![readIndex] ?? 0 : totals[provider])}</AppText>
            </View>)}
          </View>
          <ModelsCard summary={summary} measure={measure} />
        </>}

      {segment !== "limits" ? limits.entries.filter((entry) => entry.weekly && entry.pace).map((entry) => <WeeklyCard key={entry.key} entry={entry} />) : null}
    </ScrollView>
  </View>;
}

function buildChart(summary: UsageSummary, measure: "costs" | "tokens", days: number) {
  const byProvider = {} as Record<CostProvider, number[]>;
  for (const provider of COST_PROVIDERS) byProvider[provider] = summary.buckets.map((bucket) => bucket[measure][provider] ?? 0);
  const series = STACK.filter((provider) => byProvider[provider].some((value) => value > 0)).map((provider) => ({ key: provider, color: PROVIDER_COLOR[provider], values: byProvider[provider] }));
  return {
    series, byProvider,
    labels: summary.buckets.map((bucket) => bucketLabel(bucket.at, days)),
    totals: summary.buckets.map((_, index) => COST_PROVIDERS.reduce((sum, provider) => sum + byProvider[provider][index]!, 0)),
  };
}

function ModelsCard(props: { readonly summary: UsageSummary; readonly measure: "costs" | "tokens" }) {
  const value = (model: UsageSummary["models"][number]) => props.measure === "costs" ? model.cost : model.tokens;
  const models = [...props.summary.models].filter((model) => value(model) > 0).sort((a, b) => value(b) - value(a)).slice(0, 6);
  if (!models.length) return null;
  const max = value(models[0]!);
  return <View className="mt-3 rounded-[20px] border border-border-subtle bg-grouped-card p-3.5">
    <View className="flex-row items-center justify-between">
      <AppText className="font-app-medium text-[13px] text-foreground-muted">Modelos</AppText>
      <AppText className="text-xs text-foreground-muted">{props.measure === "costs" ? "por custo" : "por tokens"}</AppText>
    </View>
    {models.map((model) => <View key={`${model.provider}:${model.model}`} className="mt-[11px]">
      <View className="flex-row justify-between"><AppText numberOfLines={1} className="mr-3 flex-1 text-[13.5px]">{modelDisplayName(model.model) || model.model}</AppText><AppText className="text-[13.5px] text-foreground-muted">{props.measure === "costs" ? formatCost(model.cost) : formatTokens(model.tokens)}</AppText></View>
      <View className="mt-[7px] h-[5px] rounded-full bg-subtle-strong"><View className="h-full rounded-full" style={{ width: `${Math.max(2, (value(model) / max) * 100)}%`, backgroundColor: PROVIDER_COLOR[model.provider] }} /></View>
    </View>)}
  </View>;
}

const providerName = (provider: string) => provider === "opencode" ? "OpenCode" : provider.charAt(0).toUpperCase() + provider.slice(1);

/** The weekly limit with its pace: how much of today's share is used, and what is left to spend. */
function WeeklyCard(props: { readonly entry: LimitEntry }) {
  const { entry } = props;
  const computers = useSirus((state) => state.computers);
  const pace = entry.pace!;
  const used = Math.round(entry.window.used ?? 0);
  const left = Math.max(0, Math.round(pace.allowance - pace.today));
  const color = PROVIDER_COLOR[entry.provider === "claude" || entry.provider === "codex" || entry.provider === "opencode" ? entry.provider : "other"];
  const computer = computers.length > 1 ? computers.find((item) => item.id === entry.computerId) : undefined;
  return <View className="mt-3 rounded-[20px] border border-border-subtle bg-grouped-card p-3.5">
    <View className="flex-row items-center justify-between">
      <AppText className="font-app-medium text-[13px] text-foreground-muted">Limite semanal do {providerName(entry.provider)}{computer ? ` · ${computerName(computer)}` : ""}</AppText>
      <AppText className="text-xs" style={pace.ahead ? { color: AMBER } : undefined}>{pace.ahead ? "acima do ritmo" : "abaixo do ritmo"}</AppText>
    </View>
    <View className="mt-1.5 flex-row items-baseline gap-[7px]">
      <AppText className="font-app-bold text-[30px] tabular-nums">{used}%</AppText>
      {entry.window.resetsAt ? <AppText className="text-[13px] text-foreground-muted">renova em {resetsIn(entry.window.resetsAt, Date.now())}</AppText> : null}
    </View>
    <View className="mt-[7px] h-[7px] rounded-full bg-subtle-strong">
      <View className="absolute inset-y-0 left-0 rounded-full" style={{ width: `${Math.min(100, used)}%`, backgroundColor: pace.ahead ? AMBER : color }} />
      <View className="absolute -inset-y-[3px] w-0.5 rounded-sm bg-foreground" style={{ left: `${Math.round(pace.elapsed * 100)}%` }} />
    </View>
    <AppText className="mt-[9px] text-[12.5px] text-foreground-muted">{pace.over ? "Hoje já passou da sua parte; o resto da semana fica mais apertado." : left > 0 ? `Hoje dá para usar cerca de ${left}% sem apertar o fim da semana.` : "Por hoje, o uso já cobre a sua parte."}</AppText>
  </View>;
}

/** The Limites segment: every provider's main window as a bar, with its reset. */
function LimitsList(props: { readonly entries: readonly LimitEntry[] }) {
  const computers = useSirus((state) => state.computers);
  if (!props.entries.length) return <AppText className="mt-16 text-center text-sm text-foreground-muted">Nenhum limite disponível agora.</AppText>;
  return <>{props.entries.map((entry) => {
    const used = Math.max(0, Math.min(100, Math.round(entry.window.used ?? 0)));
    const color = PROVIDER_COLOR[entry.provider === "claude" || entry.provider === "codex" || entry.provider === "opencode" ? entry.provider : "other"];
    const computer = computers.length > 1 ? computers.find((item) => item.id === entry.computerId) : undefined;
    return <View key={entry.key} className="mt-3 rounded-[20px] border border-border-subtle bg-grouped-card p-3.5">
      <View className="flex-row items-center justify-between">
        <AppText className="font-app-medium text-[13px] text-foreground-muted">{providerName(entry.provider)}{computer ? ` · ${computerName(computer)}` : ""}</AppText>
        {entry.window.resetsAt ? <AppText className="text-xs text-foreground-muted">renova em {resetsIn(entry.window.resetsAt, Date.now())}</AppText> : null}
      </View>
      <AppText className="mt-1.5 font-app-bold text-[30px] tabular-nums">{used}%</AppText>
      <View className="mt-[7px] h-[7px] rounded-full bg-subtle-strong">
        <View className="absolute inset-y-0 left-0 rounded-full" style={{ width: `${used}%`, backgroundColor: entry.pace?.ahead ? AMBER : color }} />
        {entry.pace ? <View className="absolute -inset-y-[3px] w-0.5 rounded-sm bg-foreground" style={{ left: `${Math.round(entry.pace.elapsed * 100)}%` }} /> : null}
      </View>
      {entry.pace ? <AppText className="mt-[9px] text-[12.5px] text-foreground-muted">O traço branco marca onde o uso estaria num ritmo uniforme da semana.</AppText> : null}
    </View>;
  })}</>;
}

function Unavailable(props: { readonly onRetry: () => void }) {
  return <View className="mt-24 items-center gap-3 px-8">
    <SymbolView name={"chart.line.uptrend.xyaxis" as AppSymbolName} size={34} weight="light" tintColorClassName="accent-foreground-tertiary" />
    <AppText className="text-center font-app-medium text-base">O custo não está disponível</AppText>
    <AppText className="text-center text-sm text-foreground-muted">Este computador ainda não envia o histórico de custo. Atualize o Sirus nele e volte aqui.</AppText>
    <Pressable onPress={props.onRetry} accessibilityRole="button" className="mt-1 rounded-full bg-subtle-strong px-5 py-2.5 active:opacity-70"><AppText className="font-app-medium text-sm">Tentar de novo</AppText></Pressable>
  </View>;
}
