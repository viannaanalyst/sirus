import * as SecureStore from "expo-secure-store";
import { useEffect, useSyncExternalStore } from "react";

import { useSirus } from "./store";

/** Shown until the computer answers (and when it is too old to know `phone_look_get`). */
export const DEFAULT_MAC_ACCENT = "#bdb4f2";
const KEY = "sirus.macAccent";
const HEX = /^#[0-9a-f]{6}$/i;

/** The accent each computer reported, by computer id; shared by every screen that tints with it. */
let accents: Readonly<Record<string, string>> = {};
let restored = false;
const listeners = new Set<() => void>();
const inflight = new Set<string>();

function publish(next: Record<string, string>) {
  accents = next;
  listeners.forEach((listener) => listener());
  void SecureStore.setItemAsync(KEY, JSON.stringify(next)).catch(() => undefined);
}

function restore() {
  if (restored) return;
  restored = true;
  void SecureStore.getItemAsync(KEY).then((raw) => {
    if (!raw) return;
    const value = JSON.parse(raw) as Record<string, unknown>;
    const saved = Object.fromEntries(Object.entries(value).filter((entry): entry is [string, string] => typeof entry[1] === "string" && HEX.test(entry[1])));
    // What the computer already answered this run wins over the saved copy.
    accents = { ...saved, ...accents };
    listeners.forEach((listener) => listener());
  }).catch(() => undefined);
}

/** Asks one computer for its accent (`phone_look_get {}` → `{ accent }`); a failure keeps what was known. */
export async function refreshMacAccent(computerId: string): Promise<void> {
  if (inflight.has(computerId)) return;
  inflight.add(computerId);
  try {
    const look = await useSirus.getState().invoke<{ accent?: unknown }>(computerId, "phone_look_get", {});
    const accent = typeof look?.accent === "string" && HEX.test(look.accent) ? look.accent.toLowerCase() : null;
    if (accent && accents[computerId] !== accent) publish({ ...accents, [computerId]: accent });
  } catch {
    // An older host has no such command: the fallback stays.
  } finally {
    inflight.delete(computerId);
  }
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => { listeners.delete(listener); };
}

/**
 * The Mac's accent colour as `#rrggbb`, for a computer (the first paired one when no id is
 * given). It is fetched when the computer connects, again after every reconnect, cached on
 * the phone, and `DEFAULT_MAC_ACCENT` while unknown.
 */
export function useMacAccent(computerId?: string): string {
  const id = useSirus((state) => computerId ?? state.computers[0]?.id);
  const connection = useSirus((state) => (id ? state.connection[id] : undefined));
  const accent = useSyncExternalStore(subscribe, () => (id ? accents[id] : undefined));
  useEffect(() => { restore(); }, []);
  useEffect(() => {
    if (id && connection === "open") void refreshMacAccent(id);
  }, [id, connection]);
  return accent ?? DEFAULT_MAC_ACCENT;
}

/** The ink that reads on a fill of this colour: near-black on light fills, white on dark ones. */
export function inkOn(hex: string): string {
  const value = HEX.test(hex) ? parseInt(hex.slice(1), 16) : parseInt(DEFAULT_MAC_ACCENT.slice(1), 16);
  const luminance = (0.299 * (value >> 16) + 0.587 * ((value >> 8) & 255) + 0.114 * (value & 255)) / 255;
  return luminance > 0.58 ? "#0d0d12" : "#ffffff";
}
