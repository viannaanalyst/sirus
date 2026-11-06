import { useNavigation, type NavigationProp, type ParamListBase } from "@react-navigation/native";
import { useCallback, useMemo, useState, type ReactNode } from "react";
import { Keyboard, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import { ProjectsDrawer } from "../components/ProjectsDrawer";
import { TopBar } from "../components/TopBar";
import { bucketCounts, useConversationRows } from "../data/use-conversations";
import { useUniwindTheme } from "../lib/useUniwindTheme";
import { NativeWorkspaceTabActivityContext } from "../native/v5-workspace-context";
import { ConversationsScreen } from "./ConversationsScreen";
import { HomeScreen } from "./HomeScreen";
import { NewThreadTab } from "./NewThreadScreen";
import { MainTabContext, type ConversationFilter, type MainTab } from "./main-tabs-context";

/** Height of the top bar's buttons, and the gap below it before content starts. */
const BAR = 44;
const GAP = 12;

/**
 * A place of the top bar's pill, mounted the first time it is chosen and kept afterwards
 * (switching back keeps its scroll). Hidden places do not draw and are told they are inactive.
 */
function Place(props: { readonly active: boolean; readonly children: ReactNode }) {
  const [mounted, setMounted] = useState(props.active);
  if (props.active && !mounted) setMounted(true);
  const activity = useMemo(() => ({ active: props.active, isActive: () => props.active }), [props.active]);
  if (!mounted) return null;
  return <View style={{ position: "absolute", top: 0, bottom: 0, left: 0, right: 0, display: props.active ? "flex" : "none" }}>
    <NativeWorkspaceTabActivityContext value={activity}>{props.children}</NativeWorkspaceTabActivityContext>
  </View>;
}

/**
 * The phone's main screen (the approved design): a top bar with the sidebar button, a pill with
 * Novo, Conversas and Painel, and the settings button, over the chosen place. The sidebar
 * button opens the Projetos drawer; a conversation, a project's page or a settings page opens
 * over this screen in the main stack.
 */
export function MainTabs() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const insets = useSafeAreaInsets();
  const theme = useUniwindTheme();
  const [selected, setSelected] = useState<MainTab>("dashboard");
  const [filter, setFilter] = useState<ConversationFilter>("all");
  const [project, setProject] = useState<string | null>(null);
  const [drawerOpen, setDrawerOpen] = useState(false);
  // The badge counts what waits on the person: approvals and questions.
  const rows = useConversationRows();
  const waiting = useMemo(() => bucketCounts(rows).attention, [rows]);
  const topInset = insets.top + BAR + GAP + 6;
  const open = useCallback((tab: MainTab, next?: ConversationFilter) => {
    if (next) setFilter(next);
    Keyboard.dismiss();
    setSelected(tab);
  }, []);
  // Choosing a project: on Novo it picks where the conversation starts; anywhere else it shows
  // that project's conversations (so the drawer's tap is never invisible on Painel).
  const chooseProject = useCallback((key: string | null) => {
    setProject(key);
    if (selected !== "new") open("conversations", "all");
  }, [selected, open]);
  const context = useMemo(() => ({ selected, open, filter, setFilter, project, setProject, drawerOpen, setDrawerOpen, topInset }), [selected, open, filter, project, drawerOpen, topInset]);
  const screen = theme["--color-screen"];

  return <MainTabContext value={context}>
    <View className="flex-1 bg-screen">
      <Place active={selected === "new"}><NewThreadTab /></Place>
      <Place active={selected === "conversations"}><ConversationsScreen /></Place>
      <Place active={selected === "dashboard"}><HomeScreen /></Place>
      {/* The bar floats over the content, which scrolls under a fade of the screen's colour. */}
      <View pointerEvents="box-none" style={{ position: "absolute", top: 0, left: 0, right: 0 }}>
        <View pointerEvents="none" style={{ position: "absolute", top: 0, left: 0, right: 0, bottom: -14, experimental_backgroundImage: `linear-gradient(${screen} 72%, transparent)` }} />
        <View style={{ paddingTop: insets.top + 6, paddingHorizontal: 16 }}>
          <TopBar selected={selected} onSelect={open} onSidebar={() => setDrawerOpen(true)} onSettings={() => navigation.navigate("Settings")} waiting={waiting} />
        </View>
      </View>
      <ProjectsDrawer open={drawerOpen} onClose={() => setDrawerOpen(false)} selected={project} onSelect={chooseProject} />
    </View>
  </MainTabContext>;
}
