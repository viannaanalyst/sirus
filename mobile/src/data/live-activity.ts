import { requireOptionalNativeModule } from "expo";
import { useEffect, useRef } from "react";

import { activityChoice } from "../lib/activity-choice";
import { useSirus } from "./store";

interface Controls {
  readonly showAgentActivity?: (sessionId: string, title: string, project: string, startedAt: number) => Promise<void>;
  readonly finishAgentActivities?: (kept: string[]) => Promise<void>;
}

// A build from before the Live Activity has neither function.
const controls = requireOptionalNativeModule<Controls>("SirusNativeControls");

/**
 * A working agent on the lock screen and in the Dynamic Island. It starts when an agent is
 * working and ends, as "Terminou", when the app next sees that it stopped: the app has no server
 * push, so a suspended app can't update it, and it goes stale instead of claiming too much.
 */
export function useAgentLiveActivity(): void {
  const data = useSirus((state) => state.data);
  const started = useRef(new Map<string, number>());
  useEffect(() => {
    const everywhere = Object.values(data).flatMap((computer) => (computer?.sessions ?? []).map((session) => ({
      ...session, project: computer?.projects.find((project) => project.id === session.projectId)?.name ?? "",
    })));
    const chosen = activityChoice(everywhere);
    void controls?.finishAgentActivities?.(chosen ? [chosen.id] : []).catch(() => undefined);
    if (!chosen) {
      started.current.clear();
      return;
    }
    if (!started.current.has(chosen.id)) started.current.set(chosen.id, Date.now());
    void controls?.showAgentActivity?.(chosen.id, chosen.title || "Sirus", chosen.project, started.current.get(chosen.id)!).catch(() => undefined);
  }, [data]);
}
