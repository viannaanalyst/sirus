import { memo } from "react";
import * as Haptics from "expo-haptics";
import { Pressable, View } from "react-native";

import type { TurnActivity } from "@sirus/client/types";
import { activityElapsed, formatActivityDuration } from "@sirus/lib/agent-activity";
import { teamPhases, teamProgress } from "@sirus/lib/subagent-team";

import { SymbolView } from "../AppSymbol";
import { AppText } from "../AppText";
import { GlassSurface } from "../GlassSurface";
import { ShimmerText } from "./Shimmer";
import { useNow } from "./use-now";

/** The pill's height; the list reserves this much (plus a gap) above the composer while it shows. */
export const WORKING_PILL_HEIGHT = 38;

/**
 * "Trabalhando 15m 48s | 2/3": floats above the composer while the agent works, ticking each
 * second. The subagent part (running of all launched) shows only when the turn launched some.
 * A tap jumps to the current work (the thread decides where), with a light haptic tick.
 */
export const WorkingPill = memo(function WorkingPill(props: { readonly activity: TurnActivity | null | undefined; readonly onPress?: () => void }) {
  const now = useNow(true);
  const { activity } = props;
  if (!activity) return null;
  const progress = teamProgress(teamPhases(activity.items));
  const elapsed = formatActivityDuration(activityElapsed(activity, now));
  const label = progress.total ? `Trabalhando, ${progress.running} de ${progress.total} subagentes em execução` : "Trabalhando";
  return <Pressable onPress={() => { void Haptics.selectionAsync().catch(() => undefined); props.onPress?.(); }} accessibilityRole="button" accessibilityLabel={label} accessibilityValue={{ text: elapsed }} accessibilityHint="Vai ao trabalho atual"
    style={({ pressed }) => ({ opacity: pressed ? 0.7 : 1, transform: [{ scale: pressed ? 0.96 : 1 }] })}>
  <GlassSurface style={{ borderRadius: 19 }} className="flex-row items-center gap-2 px-[15px]" pointerEvents="none">
    <View style={{ height: WORKING_PILL_HEIGHT, justifyContent: "center" }}><ShimmerText className="font-app-semibold text-[13.5px] tabular-nums">{`Trabalhando ${elapsed}`}</ShimmerText></View>
    {progress.total ? <>
      <View className="h-4 w-px bg-border-subtle" />
      <SymbolView name="person.2" size={16} tintColorClassName="accent-foreground-muted" />
      <AppText className="text-[13.5px] tabular-nums text-foreground-muted">{`${progress.running}/${progress.total}`}</AppText>
    </> : null}
  </GlassSurface>
  </Pressable>;
});
