import * as Haptics from "expo-haptics";
import { AppState } from "react-native";
import { create } from "zustand";

import { advance, DONE_DISMISS_MS, INITIAL_TRANSITIONS, observe, pushBanners, stillRelevant, type Banner, type Observed, type TransitionState } from "./banner-transitions";
import { selectRows, useSirus } from "./store";

/** After a socket opens, its reload settles for this long before changes count as news. */
const SETTLE_MS = 2_500;

interface BannerStore {
  readonly shown: Banner[];
  dismiss: (id: string) => void;
}
export const useBanners = create<BannerStore>()((set) => ({ shown: [], dismiss: (id) => set((state) => ({ shown: state.shown.filter((item) => item.id !== id) })) }));

let onScreen: { computerId: string; sessionId: string } | null = null;
/** Told by the navigator which conversation is open; its banners go away at once. */
export function setBannerScreen(next: { computerId: string; sessionId: string } | null) {
  onScreen = next;
  if (next) useBanners.setState((state) => ({ shown: state.shown.filter((item) => !(item.computerId === next.computerId && item.sessionId === next.sessionId)) }));
}

/** Every conversation the lists would show, as the banner sees it, by computer. */
function observeAll(state: ReturnType<typeof useSirus.getState>): Map<string, Observed[]> {
  const result = new Map<string, Observed[]>(Object.keys(state.data).map((id) => [id, []]));
  for (const row of selectRows({ data: state.data })) result.get(row.computerId)?.push(observe(row.computerId, row.session, row.project?.name ?? ""));
  return result;
}

/**
 * Watches the store for transitions across every computer and feeds `useBanners`. Returns the
 * stop function. Outside the app being open (background) nothing is shown: there is no system
 * push, by decision.
 */
export function startBanners(): () => void {
  let transitions: TransitionState = INITIAL_TRANSITIONS;
  const opened = new Map<string, number>();
  const timers = new Map<string, ReturnType<typeof setTimeout>>();
  let settle: ReturnType<typeof setTimeout> | null = null;
  let last: ReturnType<typeof useSirus.getState> | null = null;

  const run = () => {
    const state = useSirus.getState();
    const now = Date.now();
    const observed = observeAll(state);
    const computers = new Map<string, readonly Observed[] | null>();
    let waitingForSettle = false;
    for (const computer of state.computers) {
      const open = state.connection[computer.id] === "open";
      if (open && !opened.has(computer.id)) opened.set(computer.id, now);
      if (!open) opened.delete(computer.id);
      const since = opened.get(computer.id);
      const settled = open && since !== undefined && now - since >= SETTLE_MS;
      if (open && !settled) waitingForSettle = true;
      computers.set(computer.id, settled ? observed.get(computer.id) ?? null : null);
    }
    const result = advance(transitions, { computers, onScreen, now });
    transitions = result.state;
    const foreground = AppState.currentState === "active";
    const flat = new Map([...observed.values()].flat().map((item) => [item.key, item]));
    const current = useBanners.getState().shown;
    // Asks answered anywhere else disappear; a computer that is out of reach keeps what it showed.
    const kept = current.filter((item) => stillRelevant(item, flat) || state.connection[item.computerId] !== "open");
    const fired = foreground ? result.fire : [];
    const next = pushBanners(kept, fired);
    if (next.length !== current.length || next.some((item, index) => item !== current[index])) useBanners.setState({ shown: next });
    if (fired.length) void Haptics.selectionAsync().catch(() => undefined);
    for (const banner of fired) if (banner.kind === "done" && !timers.has(banner.id)) {
      timers.set(banner.id, setTimeout(() => { timers.delete(banner.id); useBanners.getState().dismiss(banner.id); }, DONE_DISMISS_MS));
    }
    if (waitingForSettle && !settle) settle = setTimeout(() => { settle = null; run(); }, SETTLE_MS + 50);
  };

  const unsubscribe = useSirus.subscribe((state) => {
    // Only conversations, computers and their sockets matter; transcripts stream constantly.
    if (last && last.data === state.data && last.connection === state.connection && last.computers === state.computers) return;
    last = state;
    run();
  });
  run();
  return () => {
    unsubscribe();
    if (settle) clearTimeout(settle);
    timers.forEach(clearTimeout);
  };
}
