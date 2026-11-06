import { useSyncExternalStore } from "react";

/** One shared one-second clock for every ticking label, instead of a timer per row. */
let now = Date.now();
let timer: ReturnType<typeof setInterval> | null = null;
const listeners = new Set<() => void>();

function subscribe(listener: () => void) {
  listeners.add(listener);
  if (!timer) {
    now = Date.now();
    timer = setInterval(() => { now = Date.now(); listeners.forEach((run) => run()); }, 1000);
  }
  return () => {
    listeners.delete(listener);
    if (!listeners.size && timer) { clearInterval(timer); timer = null; }
  };
}
const idle = () => () => undefined;

/** The current time in ms, refreshed every second while `active`; frozen (and free) otherwise. */
export function useNow(active: boolean): number {
  const live = useSyncExternalStore(active ? subscribe : idle, () => now);
  return active ? live : Date.now();
}
