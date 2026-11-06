/**
 * The host's `usage_summary {days}`: cost and tokens per bucket by provider, per model, and
 * totals. Pure helpers to merge computers' answers, name buckets and format money.
 */
export type CostProvider = "claude" | "codex" | "opencode" | "other";
export const COST_PROVIDERS: readonly CostProvider[] = ["claude", "codex", "opencode", "other"];
export type PerProvider = Partial<Record<CostProvider, number>>;

export interface UsageBucket { readonly at: number | string; readonly costs: PerProvider; readonly tokens: PerProvider }
export interface UsageModel { readonly model: string; readonly provider: CostProvider; readonly cost: number; readonly tokens: number }
export interface UsageSummary {
  readonly buckets: readonly UsageBucket[];
  readonly models: readonly UsageModel[];
  readonly totalCost: number;
  readonly totalTokens: number;
  readonly previousCost: number;
}

/** Provider colours (the Mac's cost page): claude, codex, opencode, other gray. */
export const PROVIDER_COLOR: Record<CostProvider, string> = { claude: "#c9774a", codex: "#1ea3b8", opencode: "#9180ee", other: "#8e8e93" };
export const PROVIDER_NAME: Record<CostProvider, string> = { claude: "Claude", codex: "Codex", opencode: "OpenCode", other: "Outros" };

/** A bucket's instant in ms, whether the host sends seconds, milliseconds or an ISO string. */
export function bucketTime(at: number | string): number {
  if (typeof at === "string") return Date.parse(at);
  return at > 100_000_000_000 ? at : at * 1000;
}

const sum = (a: PerProvider, b: PerProvider): PerProvider => {
  const result: PerProvider = { ...a };
  for (const provider of COST_PROVIDERS) if (b[provider] !== undefined) result[provider] = (result[provider] ?? 0) + (b[provider] ?? 0);
  return result;
};

/** Adds several computers' summaries into one: buckets by instant, models by name and provider. */
export function mergeSummaries(summaries: readonly UsageSummary[]): UsageSummary | null {
  if (!summaries.length) return null;
  if (summaries.length === 1) return summaries[0]!;
  const buckets = new Map<number, UsageBucket>();
  for (const summary of summaries) for (const bucket of summary.buckets) {
    const time = bucketTime(bucket.at);
    const existing = buckets.get(time);
    buckets.set(time, existing ? { at: existing.at, costs: sum(existing.costs, bucket.costs), tokens: sum(existing.tokens, bucket.tokens) } : bucket);
  }
  const models = new Map<string, UsageModel>();
  for (const summary of summaries) for (const model of summary.models) {
    const id = `${model.provider}:${model.model}`;
    const existing = models.get(id);
    models.set(id, existing ? { ...existing, cost: existing.cost + model.cost, tokens: existing.tokens + model.tokens } : model);
  }
  return {
    buckets: [...buckets.entries()].sort((a, b) => a[0] - b[0]).map(([, bucket]) => bucket),
    models: [...models.values()],
    totalCost: summaries.reduce((total, item) => total + item.totalCost, 0),
    totalTokens: summaries.reduce((total, item) => total + item.totalTokens, 0),
    previousCost: summaries.reduce((total, item) => total + item.previousCost, 0),
  };
}

/** True when the answer has the shape this screen reads (an older host may send anything). */
export function isUsageSummary(value: unknown): value is UsageSummary {
  const item = value as Partial<UsageSummary> | null;
  return !!item && Array.isArray(item.buckets) && Array.isArray(item.models) && typeof item.totalCost === "number";
}

/** "$148,20", "$1.704": two decimals under 1000, none above. */
export function formatCost(value: number): string {
  const fixed = value >= 1000 ? Math.round(value).toString() : value.toFixed(2);
  const [whole, decimals] = fixed.split(".");
  return `$${whole!.replace(/\B(?=(\d{3})+(?!\d))/g, ".")}${decimals ? `,${decimals}` : ""}`;
}

/** "1,2 mi", "340 mil", "812": tokens, short. */
export function formatTokens(value: number): string {
  const short = (n: number, unit: string) => `${(Math.round(n * 10) / 10).toString().replace(".", ",")} ${unit}`;
  if (value >= 1e9) return short(value / 1e9, "bi");
  if (value >= 1e6) return short(value / 1e6, "mi");
  if (value >= 1e3) return short(value / 1e3, "mil");
  return String(Math.round(value));
}

/** The change against the previous period in whole percent, or null without a previous one. */
export function deltaPercent(current: number, previous: number): number | null {
  if (!(previous > 0)) return null;
  return Math.round(((current - previous) / previous) * 100);
}

const WEEKDAYS = ["dom", "seg", "ter", "qua", "qui", "sex", "sáb"];

/** A bucket's axis label for a range: hours for a day, weekdays for a week, day/month beyond. */
export function bucketLabel(at: number | string, days: number): string {
  const date = new Date(bucketTime(at));
  if (days <= 1) return `${String(date.getHours()).padStart(2, "0")}h`;
  if (days <= 7) return WEEKDAYS[date.getDay()]!;
  return `${date.getDate()}/${date.getMonth() + 1}`;
}

/** A drag-to-read label: the full day (or hour) of a bucket. */
export function bucketReadout(at: number | string, days: number): string {
  const date = new Date(bucketTime(at));
  if (days <= 1) return `Às ${String(date.getHours()).padStart(2, "0")}h`;
  return `${WEEKDAYS[date.getDay()]}, ${date.getDate()}/${date.getMonth() + 1}`;
}

/** A provider's total over the buckets for the chosen measure. */
export function providerTotals(summary: UsageSummary, measure: "costs" | "tokens"): Record<CostProvider, number> {
  const totals: Record<CostProvider, number> = { claude: 0, codex: 0, opencode: 0, other: 0 };
  for (const bucket of summary.buckets) for (const provider of COST_PROVIDERS) totals[provider] += bucket[measure][provider] ?? 0;
  return totals;
}
