import { useNavigation, type NavigationProp, type ParamListBase } from "@react-navigation/native";
import { useMemo, useState } from "react";
import { ActivityIndicator, FlatList, Pressable, View } from "react-native";
import { KeyboardStickyView } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { SymbolView } from "../components/AppSymbol";
import { AppText, AppTextInput } from "../components/AppText";
import { ControlPillMenu } from "../components/ControlPillMenu";
import { GlassSurface } from "../components/GlassSurface";
import { ProjectChip } from "../components/ProjectChip";
import { ThreadRow } from "../components/ThreadRow";
import { computerName } from "../data/computers";
import { useSirus } from "../data/store";
import { useConversationRows, type ConversationRow } from "../data/use-conversations";
import { NativeStackScreenOptions } from "../native/StackHeader";
import { useMainTabs, type ConversationFilter } from "./main-tabs-context";

const FILTERS: readonly { id: ConversationFilter; label: string }[] = [
  { id: "all", label: "Todas" },
  { id: "attention", label: "Esperando você" },
  { id: "working", label: "Trabalhando" },
  { id: "review", label: "Prontas" },
];

const EMPTY: Record<ConversationFilter, string> = {
  all: "Nenhuma conversa ainda.",
  attention: "Nada esperando por você.",
  working: "Nenhum agente trabalhando agora.",
  review: "Nada terminou nas últimas 24 horas.",
};

function matches(row: ConversationRow, needle: string) {
  return !needle || row.session.title.toLocaleLowerCase().includes(needle) || (row.project?.name ?? "").toLocaleLowerCase().includes(needle);
}

type Entry = { readonly type: "header"; readonly title: string } | { readonly type: "row"; readonly row: ConversationRow; readonly first: boolean };

/** Working first, then what waits on you, then the rest by day (Hoje, Ontem, Esta semana, Antes). */
export function sectionConversations(rows: readonly ConversationRow[], now: number): Entry[] {
  const startOfToday = new Date(now);
  startOfToday.setHours(0, 0, 0, 0);
  const today = startOfToday.getTime(), day = 86_400_000;
  const groups = new Map<string, ConversationRow[]>();
  const add = (title: string, row: ConversationRow) => { const list = groups.get(title); if (list) list.push(row); else groups.set(title, [row]); };
  for (const row of rows) {
    if (row.bucket === "working") { add("Trabalhando", row); continue; }
    if (row.bucket === "attention") { add("Te esperando", row); continue; }
    const at = Date.parse(row.session.lastActivityAt);
    add(at >= today ? "Hoje" : at >= today - day ? "Ontem" : at >= today - 6 * day ? "Esta semana" : "Antes", row);
  }
  const entries: Entry[] = [];
  for (const title of ["Trabalhando", "Te esperando", "Hoje", "Ontem", "Esta semana", "Antes"]) {
    const list = groups.get(title);
    if (!list) continue;
    entries.push({ type: "header", title });
    list.forEach((row, index) => entries.push({ type: "row", row, first: index === 0 }));
  }
  return entries;
}

/**
 * Conversas: every computer's conversations in sections, filtered by the project chosen in the
 * drawer; a search and a filter menu (what waits, works or is ready; the computer) in a pill
 * at the bottom. In `search` mode (the Search route) it is the search over every conversation.
 */
export function ConversationsScreen(props: { readonly mode?: "tab" | "search" }) {
  const search = props.mode === "search";
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const tabs = useMainTabs();
  const insets = useSafeAreaInsets();
  const [localFilter, setLocalFilter] = useState<ConversationFilter>("all");
  const filter = search ? "all" : tabs?.filter ?? localFilter;
  const setFilter = tabs?.setFilter ?? setLocalFilter;
  const computers = useSirus((state) => state.computers);
  const data = useSirus((state) => state.data);
  const [query, setQuery] = useState("");
  const [computerFilter, setComputerFilter] = useState<string | null>(null);
  const projectFilter = search ? null : tabs?.project ?? null;
  const rows = useConversationRows();
  const shown = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return rows.filter((row) => (filter === "all" || row.bucket === filter)
      && (!computerFilter || row.computerId === computerFilter)
      && (!projectFilter || `${row.computerId}:${row.session.projectId}` === projectFilter)
      && matches(row, needle));
  }, [rows, filter, query, computerFilter, projectFilter]);
  const entries = useMemo(() => sectionConversations(shown, Date.now()), [shown]);
  const several = computers.length > 1;
  const loading = computers.some((computer) => !data[computer.id]) && !rows.length;

  const open = (row: ConversationRow) => navigation.navigate("Thread", { computerId: row.computerId, sessionId: row.session.id });
  const computerLabel = (id: string) => several ? computerName(computers.find((computer) => computer.id === id) ?? { hostName: "", kind: "mac" }) : null;
  const filterActions = [
    { id: "show", title: "Mostrar", displayInline: true, subactions: FILTERS.map((item) => ({ id: `f:${item.id}`, title: item.label, state: item.id === filter ? "on" as const : "off" as const })) },
    ...(several ? [{ id: "computers", title: "Computador", displayInline: true, subactions: [
      { id: "c:", title: "Todos os computadores", state: computerFilter === null ? "on" as const : "off" as const },
      ...computers.map((computer) => ({ id: `c:${computer.id}`, title: computerName(computer), state: computerFilter === computer.id ? "on" as const : "off" as const })),
    ] }] : []),
  ];
  const onFilter = (id: string) => {
    if (id.startsWith("f:")) setFilter(id.slice(2) as ConversationFilter);
    else if (id.startsWith("c:")) setComputerFilter(id.slice(2) || null);
  };
  const filtered = filter !== "all" || computerFilter !== null;

  if (search) return <>
    <NativeStackScreenOptions optionsVersion={[search]} options={{
      title: "Buscar",
      headerSearchBarOptions: { autoFocus: true, autoCapitalize: "none", placeholder: "Conversas e projetos", onChangeText: (event) => setQuery(event.nativeEvent.text), onCancelButtonPress: () => navigation.goBack() },
    }} />
    <FlatList className="flex-1 bg-thread-canvas" contentInsetAdjustmentBehavior="automatic" keyboardShouldPersistTaps="handled" keyboardDismissMode="on-drag"
      data={shown} keyExtractor={(row) => `${row.computerId}:${row.session.id}`}
      ListEmptyComponent={<EmptyState text={query ? "Nada encontrado." : "Busque pelo título da conversa ou pelo projeto."} />}
      renderItem={({ item, index }) => <ThreadRow row={item} separated={index > 0} computerLabel={computerLabel(item.computerId)} onPress={() => open(item)} />}
      contentContainerStyle={{ paddingBottom: 24 }} />
  </>;

  return <View className="flex-1 bg-thread-canvas">
    <FlatList
      className="flex-1"
      keyboardShouldPersistTaps="handled"
      keyboardDismissMode="on-drag"
      data={entries}
      keyExtractor={(entry) => entry.type === "header" ? `h:${entry.title}` : `${entry.row.computerId}:${entry.row.session.id}`}
      ListHeaderComponent={<ProjectChip projectKey={projectFilter} />}
      ListEmptyComponent={loading ? <ActivityIndicator className="mt-16" /> : <EmptyState text={query ? "Nada encontrado." : projectFilter && filter === "all" ? "Nenhuma conversa neste projeto ainda." : EMPTY[filter]} />}
      renderItem={({ item }) => item.type === "header"
        ? <AppText className="mb-1.5 ml-[18px] mt-[18px] text-[13px] text-foreground-muted">{item.title}</AppText>
        : <ThreadRow robot row={item.row} separated={!item.first} computerLabel={computerLabel(item.row.computerId)} onPress={() => open(item.row)} />}
      contentContainerStyle={{ paddingTop: tabs?.topInset ?? 60, paddingBottom: 96 + Math.max(insets.bottom, 12) }}
    />
    <KeyboardStickyView offset={{ closed: 0, opened: Math.max(insets.bottom, 12) - 8 }} style={{ position: "absolute", left: 0, right: 0, bottom: 0 }}>
      <View style={{ paddingHorizontal: 12, paddingBottom: Math.max(insets.bottom, 12) }}>
        <GlassSurface style={{ borderRadius: 29 }} className="h-[58px] flex-row items-center gap-1 px-2">
          <View className="size-[42px] items-center justify-center"><SymbolView name="magnifyingglass" size={18} tintColorClassName="accent-foreground-muted" /></View>
          <AppTextInput value={query} onChangeText={setQuery} placeholder="Buscar conversas" autoCapitalize="none" autoCorrect={false} returnKeyType="search"
            className="min-h-0 flex-1 rounded-none border-0 bg-transparent px-0 py-0 text-base" />
          {query ? <Pressable onPress={() => setQuery("")} hitSlop={8} accessibilityLabel="Limpar busca"><SymbolView name="xmark.circle.fill" size={17} tintColorClassName="accent-foreground-muted" /></Pressable> : null}
          <ControlPillMenu actions={filterActions} onPressAction={({ nativeEvent }) => onFilter(nativeEvent.event)} accessibilityLabel="Filtrar conversas">
            <View className={`size-[42px] items-center justify-center rounded-full ${filtered ? "bg-subtle-strong" : ""}`}>
              <SymbolView name={(filtered ? "line.3.horizontal.decrease.circle.fill" : "slider.horizontal.3") as never} size={18} tintColorClassName="accent-foreground" />
            </View>
          </ControlPillMenu>
        </GlassSurface>
      </View>
    </KeyboardStickyView>
  </View>;
}

export function EmptyState(props: { readonly text: string; readonly icon?: string }) {
  return <View className="mt-20 items-center gap-3 px-10">
    <SymbolView name={(props.icon ?? "bubble.left.and.bubble.right") as never} size={34} weight="light" tintColorClassName="accent-foreground-tertiary" />
    <AppText className="text-center text-sm text-foreground-muted">{props.text}</AppText>
  </View>;
}

/** The search over every conversation, opened by the tab bar's search button. */
export function SearchScreen() {
  return <ConversationsScreen mode="search" />;
}
