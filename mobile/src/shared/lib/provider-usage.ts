import type { AgentProviderId, ProviderUsage, UsageWindow } from "../client/types";

/** Providers whose quota native code can read (matches `provider_usage::reports_usage`). */
export const USAGE_PROVIDER_IDS: readonly AgentProviderId[] = ["codex", "claude", "cursor", "opencode"];

/** The provider's main quota first (Codex `codex/…`), extra buckets after. */
export function orderedUsageWindows(usage: ProviderUsage | undefined): UsageWindow[] {
  const windows = usage?.windows ?? [];
  const main = (window: UsageWindow) => window.id.startsWith("base_model_inference/") ? 1 : 0;
  return [...windows].sort((a, b) => main(a) - main(b));
}

export function resetDuration(resetsAt: number | null, now: number): string | null {
  if (resetsAt == null || !Number.isFinite(resetsAt)) return null;
  const minutes = Math.max(0, Math.ceil((resetsAt - now) / 60_000));
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor(minutes % 1440 / 60);
  if (days) return `${days}d ${hours}h`;
  if (hours) return `${hours}h ${minutes % 60}m`;
  return `${minutes}m`;
}

export function usageWindowLabel(window: UsageWindow): string {
  const labels: Record<string, string> = {
    five_hour: "5-hour usage", seven_day: "Weekly usage", seven_day_opus: "Weekly · Opus",
    seven_day_sonnet: "Weekly · Sonnet", seven_day_oauth_apps: "Weekly · OAuth apps",
    "opencode-go/rolling": "5-hour usage", "opencode-go/weekly": "Weekly usage", "opencode-go/monthly": "Monthly usage",
    "cursor/models": "Cursor models", "cursor/other-models": "Other models", "cursor/included": "Included usage",
    "cursor/member": "Individual limit", "cursor/team": "Team pool",
  };
  if (labels[window.id]) return labels[window.id];
  // Codex also reports a separate limit for its reserve model ("gpt-reserve"); it is not the main quota.
  if (window.id.startsWith("base_model_inference/")) return window.durationMinutes === 300 ? "5-hour · reserve model" : "Weekly · reserve model";
  if (window.durationMinutes === 300) return "5-hour usage";
  if (window.durationMinutes === 10_080) return "Weekly usage";
  return window.id;
}
