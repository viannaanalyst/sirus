import type { AgentProviderId, ProviderUsage } from "@sirus/client/types";
import { isProviderEnabled } from "@sirus/lib/settings";
import * as SecureStore from "expo-secure-store";
import { useEffect, useMemo } from "react";
import { create } from "zustand";

import { useSirus } from "./store";
import { isUsageSummary, mergeSummaries, type UsageSummary } from "./usage-summary";
import { pace, parseWindows, weeklyWindow, type Pace, type Reading, type Win } from "./usage-pace";

// ---------------------------------------------------------------- cost: `usage_summary {days}`

export type SummaryState = { readonly status: "loading" } | { readonly status: "unavailable" } | { readonly status: "ready"; readonly summary: UsageSummary };

const useSummaries = create<{ readonly byDays: Record<number, SummaryState>; load: (days: number, force?: boolean) => Promise<void> }>()((set, get) => ({
  byDays: {},
  load: async (days, force = false) => {
    const current = get().byDays[days];
    if (current?.status === "loading" || (current?.status === "ready" && !force)) return;
    if (current?.status !== "ready") set((state) => ({ byDays: { ...state.byDays, [days]: { status: "loading" } } }));
    const { computers, connection, invoke } = useSirus.getState();
    const answers = await Promise.all(computers.filter((computer) => connection[computer.id] === "open").map((computer) =>
      invoke<unknown>(computer.id, "usage_summary", { days }).then((value) => isUsageSummary(value) ? value : null, () => null)));
    const merged = mergeSummaries(answers.filter((item): item is UsageSummary => !!item));
    // A host without the command (or none reachable) is an unavailable state, not an error.
    set((state) => ({ byDays: { ...state.byDays, [days]: merged ? { status: "ready", summary: merged } : { status: "unavailable" } } }));
  },
}));

/** The cost summary over `days` days, merged across the connected computers. */
export function useUsageSummary(days: number, enabled = true): SummaryState & { readonly reload: () => void } {
  const state = useSummaries((store) => store.byDays[days]) ?? { status: "loading" as const };
  const online = useSirus((store) => store.computers.filter((computer) => store.connection[computer.id] === "open").map((computer) => computer.id).join(","));
  useEffect(() => { if (enabled && online) void useSummaries.getState().load(days, true); }, [days, enabled, online]);
  return { ...state, reload: () => void useSummaries.getState().load(days, true) };
}

// ---------------------------------------------------------------- limits: `provider_usage`

const READINGS_KEY = "sirus.usageDayReadings";
const KEEP_DAYS = 21;
type Readings = Record<string, Record<string, Reading>>;
let readings: Readings = {};
let restored: Promise<void> | null = null;

const dayKey = (now: number) => { const date = new Date(now); return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`; };

function restoreReadings() {
  restored ??= SecureStore.getItemAsync(READINGS_KEY).then((raw) => { if (raw) readings = { ...(JSON.parse(raw) as Readings), ...readings }; }).catch(() => undefined);
  return restored;
}

/** Keeps the first weekly reading of each local day, as the Mac does, so the pace has a baseline. */
async function recordReading(account: string, weekly: Win, now: number): Promise<Reading | undefined> {
  await restoreReadings();
  const days = (readings[account] ??= {});
  const day = dayKey(now);
  if (!days[day]) {
    days[day] = { used: weekly.used ?? 0, resetsAt: weekly.resetsAt ?? 0, at: now };
    for (const old of Object.keys(days).sort().slice(0, Math.max(0, Object.keys(days).length - KEEP_DAYS))) delete days[old];
    void SecureStore.setItemAsync(READINGS_KEY, JSON.stringify(readings)).catch(() => undefined);
  }
  return days[day];
}

export interface LimitEntry {
  readonly key: string;
  readonly computerId: string;
  readonly provider: AgentProviderId;
  /** The bar: the weekly window when there is one, else the longest. */
  readonly window: Win;
  readonly weekly: boolean;
  readonly pace: Pace | null;
}

const useLimitsStore = create<{ readonly entries: Record<string, LimitEntry>; readonly loaded: boolean; load: (force?: boolean) => Promise<void> }>()((set, get) => ({
  entries: {},
  loaded: false,
  load: async (force = false) => {
    if (get().loaded && !force) return;
    const state = useSirus.getState();
    const jobs: Promise<LimitEntry | null>[] = [];
    for (const computer of state.computers) {
      const settings = state.data[computer.id]?.settings;
      if (!settings || state.connection[computer.id] !== "open") continue;
      for (const provider of ["claude", "codex", "opencode", "cursor"] as const) {
        if (!isProviderEnabled(settings, provider)) continue;
        jobs.push(state.invoke<ProviderUsage>(computer.id, "provider_usage", { provider, refresh: false }).then(async (usage) => {
          const windows = parseWindows(usage).filter((window) => window.used !== null && !window.id.startsWith("base_model_inference/"));
          const weekly = weeklyWindow(windows);
          const chosen = weekly ?? [...windows].sort((a, b) => (b.minutes ?? 0) - (a.minutes ?? 0))[0];
          if (!chosen) return null;
          const now = Date.now();
          const baseline = weekly ? await recordReading(`${provider}/${computer.id}`, weekly, now) : undefined;
          return { key: `${computer.id}:${provider}`, computerId: computer.id, provider, window: chosen, weekly: !!weekly,
            pace: weekly && weekly.resetsAt ? pace(weekly.used ?? 0, baseline, weekly.resetsAt, weekly.minutes, now) : null };
        }, () => null));
      }
    }
    const done = (await Promise.all(jobs)).filter((entry): entry is LimitEntry => !!entry);
    set({ entries: Object.fromEntries(done.map((entry) => [entry.key, entry])), loaded: true });
  },
}));

/** Each provider's quota on each connected computer, loaded once the computers answer. */
export function useLimits(): { readonly entries: LimitEntry[]; readonly reload: () => void } {
  const entries = useLimitsStore((store) => store.entries);
  const ready = useSirus((store) => store.computers.some((computer) => store.connection[computer.id] === "open" && !!store.data[computer.id]?.settings));
  useEffect(() => { if (ready) void useLimitsStore.getState().load(); }, [ready]);
  const list = useMemo(() => Object.values(entries).sort((a, b) => a.key.localeCompare(b.key)), [entries]);
  return { entries: list, reload: () => void useLimitsStore.getState().load(true) };
}
