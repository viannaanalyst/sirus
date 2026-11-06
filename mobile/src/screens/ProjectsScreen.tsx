import { useNavigation, useRoute, type NavigationProp, type ParamListBase, type RouteProp } from "@react-navigation/native";
import type { Project } from "@sirus/client/types";
import { useCallback, useEffect, useMemo, useState } from "react";
import { Pressable, RefreshControl, ScrollView, StyleSheet, View } from "react-native";
import { create } from "zustand";

import { SymbolView, type AppSymbolName } from "../components/AppSymbol";
import { AppText } from "../components/AppText";
import { ListSection } from "../components/ListSection";
import { ProjectRobot } from "../components/ProjectRobot";
import { ThreadRow } from "../components/ThreadRow";
import { computerName } from "../data/computers";
import { prKey, projectRepositories, usePullRequests } from "../data/pull-requests";
import { isScratch } from "../data/scratch";
import { useSirus } from "../data/store";
import { useConversationRows, type ConversationRow } from "../data/use-conversations";
import { NativeHeaderToolbar, NativeStackScreenOptions } from "../native/StackHeader";
import { EmptyState } from "./ConversationsScreen";
import { ICON } from "../lib/icon-sizes";

type Diff = { additions: number; deletions: number };

/** Uncommitted line counts by `computerId:projectId` (`project_diff_stats`), refreshed on demand. */
export const useProjectDiffs = create<{ readonly diffs: Record<string, Diff>; load: (computerId: string) => Promise<void> }>()((set) => ({
  diffs: {},
  load: async (computerId) => {
    try {
      const stats = await useSirus.getState().invoke<{ projectId: string; additions: number; deletions: number }[]>(computerId, "project_diff_stats");
      set((state) => ({ diffs: { ...state.diffs, ...Object.fromEntries(stats.map((stat) => [`${computerId}:${stat.projectId}`, { additions: stat.additions, deletions: stat.deletions }])) } }));
    } catch { /* an older computer without the command shows no counts */ }
  },
}));

export interface ProjectSummary {
  readonly computerId: string;
  readonly project: Project;
  readonly rows: ConversationRow[];
  readonly branch: string | null;
}

/** Every computer's projects (not the "Sem projeto" folder), most recently opened first. */
export function useProjectSummaries(): ProjectSummary[] {
  const data = useSirus((state) => state.data);
  const computers = useSirus((state) => state.computers);
  const rows = useConversationRows();
  return useMemo(() => computers.flatMap((computer) => (data[computer.id]?.projects ?? []).filter((project) => !isScratch(project)).map((project) => {
    const own = rows.filter((row) => row.computerId === computer.id && row.session.projectId === project.id);
    // The branch the project's own checkout is on: its latest conversation that is not in a worktree of its own.
    const checkout = own.find((row) => !row.session.worktree?.isolated) ?? own[0];
    return { computerId: computer.id, project, rows: own, branch: checkout?.session.worktree?.branch ?? null };
  })).sort((a, b) => (b.rows[0]?.session.lastActivityAt ?? b.project.lastOpenedAt).localeCompare(a.rows[0]?.session.lastActivityAt ?? a.project.lastOpenedAt)), [computers, data, rows]);
}

function DiffText(props: { readonly diff: Diff | undefined; readonly className?: string }) {
  if (!props.diff || (!props.diff.additions && !props.diff.deletions)) return null;
  return <AppText className={`tabular-nums ${props.className ?? "text-xs"}`}>
    {props.diff.additions ? <AppText className="text-adaptive-emerald-600-400">+{props.diff.additions}</AppText> : null}
    {props.diff.additions && props.diff.deletions ? " " : null}
    {props.diff.deletions ? <AppText className="text-danger-foreground">−{props.diff.deletions}</AppText> : null}
  </AppText>;
}

/**
 * Projetos: each computer's projects with what is happening in them (branch, uncommitted lines,
 * agents working, approvals waiting, open pull requests). A row opens the project's page.
 */
export function ProjectsScreen() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const computers = useSirus((state) => state.computers);
  const summaries = useProjectSummaries();
  const diffs = useProjectDiffs((state) => state.diffs);
  const prs = usePullRequests((state) => state.entries);
  const [refreshing, setRefreshing] = useState(false);
  const refresh = useCallback(async (force = false) => {
    await Promise.all(computers.flatMap((computer) => [useProjectDiffs.getState().load(computer.id), usePullRequests.getState().load(computer.id, "open", force)]));
  }, [computers]);
  useEffect(() => { void refresh(); }, [refresh]);
  const several = computers.length > 1;
  const groups = several ? computers.map((computer) => ({ computer, items: summaries.filter((item) => item.computerId === computer.id) })).filter((group) => group.items.length) : [{ computer: null, items: summaries }];

  return <>
    <NativeStackScreenOptions options={{ title: "Projetos", headerTitle: undefined, headerSearchBarOptions: undefined, unstable_headerLeftItems: () => [] }} />
    <NativeHeaderToolbar placement="right">
      <NativeHeaderToolbar.Button accessibilityLabel="Nova conversa" icon="square.and.pencil" onPress={() => navigation.navigate("NewThread")} />
    </NativeHeaderToolbar>
    <ScrollView className="flex-1 bg-screen" contentInsetAdjustmentBehavior="automatic" contentContainerClassName="gap-6 px-4 pb-8 pt-3"
      refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => { setRefreshing(true); void refresh(true).finally(() => setRefreshing(false)); }} />}>
      {summaries.length ? groups.map((group) => <ListSection key={group.computer?.id ?? "all"} title={group.computer ? computerName(group.computer) : undefined}>
        {group.items.map((item, index) => {
          const working = item.rows.filter((row) => row.bucket === "working").length;
          const waiting = item.rows.filter((row) => row.bucket === "attention").length;
          const open = projectRepositories(prs[prKey(item.computerId)]?.inbox, item.project.id);
          const pulls = prs[prKey(item.computerId)]?.inbox?.items.filter((pr) => open.includes(pr.repository)).length ?? 0;
          return <Pressable key={`${item.computerId}:${item.project.id}`} onPress={() => navigation.navigate("Project", { computerId: item.computerId, projectId: item.project.id })}
            accessibilityRole="button" accessibilityLabel={item.project.name} className="bg-grouped-card active:bg-row-hover">
            {index > 0 ? <View className="absolute left-[58px] right-0 top-0 bg-separator" style={{ height: StyleSheet.hairlineWidth }} /> : null}
            <View className="flex-row items-center gap-3 px-4 py-3">
              <ProjectRobot project={item.project} size={30} working={working > 0} />
              <View className="min-w-0 flex-1 gap-0.5">
                <AppText numberOfLines={1} className="font-app-bold text-base">{item.project.name}</AppText>
                <View className="flex-row flex-wrap items-center gap-x-2.5 gap-y-0.5">
                  {item.branch ? <Meta icon="arrow.triangle.branch" text={item.branch} /> : null}
                  <DiffText diff={diffs[`${item.computerId}:${item.project.id}`]} />
                  {waiting ? <AppText className="font-app-medium text-xs text-warning-foreground">{waiting === 1 ? "1 aprovação" : `${waiting} esperando`}</AppText> : null}
                  {working ? <AppText className="font-app-medium text-xs text-adaptive-sky-600-400">{working} trabalhando</AppText> : null}
                  {pulls ? <Meta icon="arrow.triangle.pull" text={pulls === 1 ? "1 PR" : `${pulls} PRs`} /> : null}
                </View>
              </View>
              <SymbolView name="chevron.right" size={13} weight="semibold" tintColorClassName="accent-chevron" />
            </View>
          </Pressable>;
        })}
      </ListSection>) : <EmptyState icon="folder" text="Nenhum projeto ainda. Adicione um projeto no Sirus do computador, ou ao começar uma conversa." />}
    </ScrollView>
  </>;
}

function Meta(props: { readonly icon: string; readonly text: string }) {
  return <View className="flex-row items-center gap-1">
    <SymbolView name={props.icon as AppSymbolName} size={10} tintColorClassName="accent-foreground-tertiary" />
    <AppText numberOfLines={1} className="max-w-[140px] text-xs text-foreground-muted">{props.text}</AppText>
  </View>;
}

type ProjectParams = { Project: { computerId: string; projectId: string } };

/**
 * A project's page: its tile and path, three counters (conversations, uncommitted lines, open
 * pull requests), its conversations, and the project's tools (changes and commits, pull requests,
 * files, terminal). The tools work through the project's latest conversation in its own checkout.
 */
export function ProjectScreen() {
  const { computerId, projectId } = useRoute<RouteProp<ProjectParams, "Project">>().params;
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const summary = useProjectSummaries().find((item) => item.computerId === computerId && item.project.id === projectId);
  const diff = useProjectDiffs((state) => state.diffs[`${computerId}:${projectId}`]);
  const entry = usePullRequests((state) => state.entries[prKey(computerId)]);
  const several = useSirus((state) => state.computers.length > 1);
  const computer = useSirus((state) => state.computers.find((item) => item.id === computerId));
  const [refreshing, setRefreshing] = useState(false);
  const refresh = useCallback(async (force = false) => {
    await Promise.all([useProjectDiffs.getState().load(computerId), usePullRequests.getState().load(computerId, "open", force)]);
  }, [computerId]);
  useEffect(() => { void refresh(); }, [refresh]);
  if (!summary) return <EmptyState icon="folder" text="Este projeto não está mais neste computador." />;
  const { project, rows } = summary;
  const repositories = projectRepositories(entry?.inbox, projectId);
  const pulls = entry?.inbox?.items.filter((pr) => repositories.includes(pr.repository)) ?? [];
  // The tools need a conversation in the project's own checkout (its Git, files and terminal).
  const anchor = rows.find((row) => !row.session.worktree?.isolated)?.session ?? rows[0]?.session;
  const tool = (screen: string, extra: Record<string, unknown> = {}) => anchor ? () => navigation.navigate(screen, { computerId, sessionId: anchor.id, ...extra }) : undefined;

  return <>
    <NativeStackScreenOptions options={{ title: "" }} />
    <NativeHeaderToolbar placement="right">
      <NativeHeaderToolbar.Button accessibilityLabel="Nova conversa neste projeto" icon="square.and.pencil" onPress={() => navigation.navigate("NewThread", { projectKey: `${computerId}:${projectId}` })} />
    </NativeHeaderToolbar>
    <ScrollView className="flex-1 bg-screen" contentInsetAdjustmentBehavior="automatic" contentContainerClassName="gap-6 px-4 pb-10 pt-2"
      refreshControl={<RefreshControl refreshing={refreshing} onRefresh={() => { setRefreshing(true); void refresh(true).finally(() => setRefreshing(false)); }} />}>
      <View className="flex-row items-center gap-3.5 px-1">
        <ProjectRobot project={project} size={48} working={rows.some((row) => row.bucket === "working")} />
        <View className="min-w-0 flex-1 gap-0.5">
          <AppText numberOfLines={1} className="font-app-bold text-2xl">{project.name}</AppText>
          <AppText numberOfLines={1} className="text-sm text-foreground-muted">{several && computer ? `${computerName(computer)} · ` : ""}{project.path.replace(/^\/(Users|home)\/[^/]+/, "~")}</AppText>
          {summary.branch ? <View className="mt-0.5"><Meta icon="arrow.triangle.branch" text={summary.branch} /></View> : null}
        </View>
      </View>
      <View className="flex-row gap-2.5">
        <Counter value={String(rows.length)} label={rows.length === 1 ? "conversa" : "conversas"} />
        <Counter value={diff && (diff.additions || diff.deletions) ? <DiffText diff={diff} className="text-xl" /> : "0"} label="pendentes" onPress={tool("Git")} />
        <Counter value={String(pulls.length)} label={pulls.length === 1 ? "PR aberto" : "PRs abertos"} onPress={() => navigation.navigate("PullRequests", { computerId, projectId })} />
      </View>
      <ListSection title="Projeto">
        <ToolRow icon="plusminus" label="Alterações e commits" detail={diff && (diff.additions || diff.deletions) ? [diff.additions ? `+${diff.additions}` : "", diff.deletions ? `−${diff.deletions}` : ""].filter(Boolean).join(" ") : undefined} onPress={tool("Git")} />
        <ToolRow separated icon="arrow.triangle.pull" label="Pull requests" detail={entry?.loading && !entry.inbox ? "…" : pulls.length ? String(pulls.length) : undefined} onPress={() => navigation.navigate("PullRequests", { computerId, projectId })} />
        <ToolRow separated icon="doc.text" label="Arquivos" onPress={tool("Files", { title: project.name })} />
        <ToolRow separated icon="terminal" label="Terminal" onPress={tool("Terminal")} />
      </ListSection>
      {!anchor ? <AppText className="-mt-3 px-3 text-sm text-foreground-muted">Alterações, arquivos e terminal abrem depois da primeira conversa neste projeto.</AppText> : null}
      <ListSection title={rows.length ? `Conversas · ${rows.length}` : "Conversas"}>
        {rows.length ? rows.map((row, index) => <ThreadRow key={row.session.id} robot row={row} grouped separated={index > 0} computerLabel={null}
          onPress={() => navigation.navigate("Thread", { computerId, sessionId: row.session.id })} />)
          : <Pressable onPress={() => navigation.navigate("NewThread", { projectKey: `${computerId}:${projectId}` })} className="bg-grouped-card px-4 py-4 active:bg-row-hover">
            <AppText className="font-app-medium text-base text-adaptive-blue-600-400">Começar uma conversa</AppText>
          </Pressable>}
      </ListSection>
    </ScrollView>
  </>;
}

function Counter(props: { readonly value: React.ReactNode; readonly label: string; readonly onPress?: () => void }) {
  return <Pressable disabled={!props.onPress} onPress={props.onPress} className="flex-1 gap-0.5 rounded-[20px] border-continuous bg-grouped-card px-3.5 py-3 active:opacity-70">
    {typeof props.value === "string" ? <AppText className="font-app-bold text-xl tabular-nums">{props.value}</AppText> : props.value}
    <AppText numberOfLines={1} className="text-xs text-foreground-muted">{props.label}</AppText>
  </Pressable>;
}

/** A row of the project page: a neutral line icon, the label, a detail and a chevron (as `ListRow`). */
export function ToolRow(props: { readonly icon: string; readonly label: string; readonly detail?: string; readonly separated?: boolean; readonly onPress?: () => void }) {
  return <Pressable disabled={!props.onPress} onPress={props.onPress} accessibilityRole="button" accessibilityLabel={props.label}
    className={`bg-grouped-card active:bg-row-hover ${props.onPress ? "" : "opacity-45"}`}>
    {props.separated ? <View className="absolute left-[56px] right-0 top-0 bg-separator" style={{ height: StyleSheet.hairlineWidth }} /> : null}
    <View className="min-h-[52px] flex-row items-center gap-3 px-4 py-2.5">
      <View className="size-7 items-center justify-center"><SymbolView name={props.icon as AppSymbolName} size={ICON.row} tintColorClassName="accent-icon" type="monochrome" weight="regular" /></View>
      <AppText className="flex-1 text-base">{props.label}</AppText>
      {props.detail ? <AppText className="text-sm tabular-nums text-foreground-muted">{props.detail}</AppText> : null}
      <SymbolView name="chevron.right" size={13} weight="semibold" tintColorClassName="accent-chevron" />
    </View>
  </Pressable>;
}

