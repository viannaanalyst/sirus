import type { ProviderUsage, UsageWindow } from "@sirus/client/types";

/** A port of the Mac's weekly-limit pace (`native-spike/src/usage_page.rs`). Pure, no storage. */
export const WEEK_MINUTES = 7 * 24 * 60;
const HOUR = 3_600_000;
/** A baseline taken in an earlier week is no use once the reset has passed. */
const RESET_SLACK = HOUR;

/** Windows with a percentage, as the Mac reads them (`resetsAt` in seconds or milliseconds). */
export interface Win { readonly id: string; readonly used: number | null; readonly resetsAt: number | null; readonly minutes: number | null }

export function parseWindows(usage: Pick<ProviderUsage, "windows"> | null | undefined): Win[] {
  return (usage?.windows ?? []).map((window: UsageWindow) => ({
    id: window.id,
    used: typeof window.usedPercent === "number" ? window.usedPercent : null,
    resetsAt: typeof window.resetsAt === "number" ? (window.resetsAt > 100_000_000_000 ? window.resetsAt : window.resetsAt * 1000) : null,
    minutes: typeof window.durationMinutes === "number" ? window.durationMinutes : null,
  }));
}

/** The account's weekly window, with a percentage and a reset time; none means no pace. */
export function weeklyWindow(windows: readonly Win[]): Win | undefined {
  return windows.find((window) => !window.id.startsWith("base_model_inference/")
    && !["_opus", "_sonnet", "_oauth_apps"].some((suffix) => window.id.endsWith(suffix))
    && (window.minutes === WEEK_MINUTES || window.id === "seven_day" || window.id === "opencode-go/weekly")
    && window.used !== null && window.resetsAt !== null);
}

/** The weekly percentage at the first reading of a local day. */
export interface Reading { readonly used: number; readonly resetsAt: number; readonly at: number }

export interface Pace {
  /** Percentage used at the start of the local day. */
  readonly start: number;
  /** Used since then. */
  readonly today: number;
  /** What today may use: the rest of the week spread over the days left. */
  readonly allowance: number;
  readonly over: boolean;
  readonly daysLeft: number;
  /** Share of the week gone, 0 to 1: where the even-pace marker sits. */
  readonly elapsed: number;
  readonly ahead: boolean;
}

const localDay = (ms: number) => { const date = new Date(ms); return Date.UTC(date.getFullYear(), date.getMonth(), date.getDate()) / 86_400_000; };

/** Days from today to the reset, counting today (at least 1). A reset at midnight sharp belongs to the day before. */
export function daysLeft(now: number, resetsAt: number): number {
  return Math.max(1, localDay(resetsAt - 1) - localDay(now) + 1);
}

export function pace(used: number, baseline: Reading | undefined, resetsAt: number, durationMinutes: number | null, now: number): Pace {
  const left = daysLeft(now, resetsAt);
  const start = baseline ? (resetsAt - baseline.resetsAt > RESET_SLACK ? 0 : Math.min(baseline.used, used)) : used;
  const today = Math.max(0, used - start);
  const allowance = Math.max(0, 100 - start) / left;
  const span = (durationMinutes && durationMinutes > 0 ? durationMinutes : WEEK_MINUTES) * 60_000;
  const elapsed = Math.min(1, Math.max(0, 1 - (resetsAt - now) / span));
  return { start, today, allowance, over: today > allowance + 1e-9, daysLeft: left, elapsed, ahead: used > elapsed * 100 };
}

/** "3 dias", "5 h", "20 min": how long until a reset, for the limits card. */
export function resetsIn(resetsAt: number, now: number): string {
  const minutes = Math.max(0, Math.ceil((resetsAt - now) / 60_000));
  const days = Math.floor(minutes / 1440);
  if (days >= 1) return days === 1 ? "1 dia" : `${days} dias`;
  const hours = Math.floor(minutes / 60);
  if (hours >= 1) return `${hours} h`;
  return `${Math.max(1, minutes)} min`;
}
