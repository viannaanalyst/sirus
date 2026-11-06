import { useNavigation, type NavigationProp, type ParamListBase } from "@react-navigation/native";
import { useEffect } from "react";
import { Pressable, View } from "react-native";

import { useSirus } from "../data/store";
import { useProjectDiffs, useProjectSummaries } from "../screens/ProjectsScreen";
import { AppText } from "./AppText";
import { SymbolView } from "./AppSymbol";

/**
 * The project chosen in the drawer, under the top bar: its name, its checkout's branch and the
 * uncommitted lines. A tap opens the project's page. Shows nothing for "Todos os projetos".
 */
export function ProjectChip(props: { readonly projectKey: string | null }) {
  return props.projectKey ? <Chip projectKey={props.projectKey} /> : null;
}

function Chip(props: { readonly projectKey: string }) {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const summaries = useProjectSummaries();
  const diffs = useProjectDiffs((state) => state.diffs);
  const item = summaries.find((entry) => `${entry.computerId}:${entry.project.id}` === props.projectKey);
  const computerId = item?.computerId;
  const connected = useSirus((state) => computerId ? state.connection[computerId] === "open" : false);
  useEffect(() => { if (computerId && connected) void useProjectDiffs.getState().load(computerId); }, [computerId, connected]);
  if (!item) return null;
  const diff = diffs[`${item.computerId}:${item.project.id}`];
  return <Pressable onPress={() => navigation.navigate("Project", { computerId: item.computerId, projectId: item.project.id })} accessibilityRole="button" accessibilityLabel={`Abrir ${item.project.name}`}
    className="mt-2.5 flex-row items-center justify-center gap-1.5 self-center active:opacity-60">
    <SymbolView name="folder" size={14} tintColorClassName="accent-foreground-muted" />
    <AppText className="font-app-medium text-[13.5px] text-foreground-secondary">{item.project.name}</AppText>
    {item.branch ? <View className="flex-row items-center gap-1">
      <AppText className="text-[13.5px] text-foreground-tertiary">·</AppText>
      <AppText numberOfLines={1} className="max-w-[150px] text-[13.5px] text-foreground-muted">{item.branch}</AppText>
    </View> : null}
    {diff?.additions ? <AppText className="text-[13.5px] tabular-nums text-adaptive-emerald-600-400">+{diff.additions}</AppText> : null}
    {diff?.deletions ? <AppText className="text-[13.5px] tabular-nums text-danger-foreground">−{diff.deletions}</AppText> : null}
  </Pressable>;
}
