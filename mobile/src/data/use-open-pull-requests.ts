import { useEffect, useMemo } from "react";

import { prKey, usePullRequests } from "./pull-requests";
import { useSirus } from "./store";

/**
 * Open pull requests across the connected computers, from the same GitHub inbox the PR screens
 * read. Null while no computer has an inbox (GitHub not set up or not loaded yet), so the
 * dashboard hides the tile instead of showing a made-up zero. `computerId` is the one with most.
 */
export function usePullRequestCount(): { readonly count: number; readonly computerId: string } | null {
  const computers = useSirus((state) => state.computers);
  const online = useSirus((state) => state.computers.filter((computer) => state.connection[computer.id] === "open").map((computer) => computer.id).join(","));
  const entries = usePullRequests((state) => state.entries);
  useEffect(() => { for (const id of online.split(",").filter(Boolean)) void usePullRequests.getState().load(id, "open"); }, [online]);
  return useMemo(() => {
    let total = 0, best: { count: number; computerId: string } | null = null, any = false;
    for (const computer of computers) {
      const inbox = entries[prKey(computer.id)]?.inbox;
      if (!inbox) continue;
      any = true;
      total += inbox.items.length;
      if (!best || inbox.items.length > best.count) best = { count: inbox.items.length, computerId: computer.id };
    }
    return any && best ? { count: total, computerId: best.computerId } : null;
  }, [computers, entries]);
}
