import { memo } from "react";
import { View } from "react-native";

import type { TurnActivity } from "@sirus/client/types";
import { activityElapsed, formatActivityDuration } from "@sirus/lib/agent-activity";
import { teamPhases, teamProgress } from "@sirus/lib/subagent-team";

import { SymbolView } from "../AppSymbol";
import { AppText } from "../AppText";
import { GlassSurface } from "../GlassSurface";
import { ShimmerText } from "./Shimmer";
import { useNow } from "./use-now";

/**
 * "Trabalhando 15m 48s | 2/3": floats above the composer while the agent works, ticking each
 * second. The subagent part (running of all launched) shows only when the turn launched some.
 */
export const WorkingPill = memo(function WorkingPill(props: { readonly activity: TurnActivity | null | undefined }) {
  const now = useNow(true);
  const { activity } = props;
  if (!activity) return null;
  const progress = teamProgress(teamPhases(activity.items));
  return <GlassSurface style={{ borderRadius: 19 }} className="h-[38px] flex-row items-center gap-2 px-[15px]" pointerEvents="none">
    <ShimmerText className="font-app-semibold text-[13.5px] tabular-nums">{`Trabalhando ${formatActivityDuration(activityElapsed(activity, now))}`}</ShimmerText>
    {progress.total ? <>
      <View className="h-4 w-px bg-border-subtle" />
      <SymbolView name="person.2" size={14} tintColorClassName="accent-foreground-muted" />
      <AppText accessibilityLabel={`${progress.running} de ${progress.total} subagentes em execução`} className="text-[13.5px] tabular-nums text-foreground-muted">{`${progress.running}/${progress.total}`}</AppText>
    </> : null}
  </GlassSurface>;
});
