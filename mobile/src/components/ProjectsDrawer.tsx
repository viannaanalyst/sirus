import { useNavigation, type NavigationProp, type ParamListBase } from "@react-navigation/native";
import * as Haptics from "expo-haptics";
import { useEffect, useMemo, useState } from "react";
import { Image, Keyboard, Pressable, ScrollView, useWindowDimensions, View } from "react-native";
import { Gesture, GestureDetector } from "react-native-gesture-handler";
import Animated, { cancelAnimation, Easing, interpolate, runOnJS, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import SIRUS_GLYPH from "../../assets/sirus-glyph.png";
import { computerName } from "../data/computers";
import { useSirus } from "../data/store";
import { useUniwindTheme } from "../lib/useUniwindTheme";
import { addProject, useProjects } from "../screens/NewThreadScreen";
import { useProjectSummaries, type ProjectSummary } from "../screens/ProjectsScreen";
import { AppText, AppTextInput } from "./AppText";
import { SymbolView, type AppSymbolName } from "./AppSymbol";
import { ControlPillMenu } from "./ControlPillMenu";
import { ProjectRobot } from "./ProjectRobot";
import { RoundButton } from "./TopBar";

type Show = "all" | "active" | "waiting";
const SHOW: readonly { id: Show; title: string }[] = [
  { id: "all", title: "Todos os projetos" },
  { id: "active", title: "Com atividade" },
  { id: "waiting", title: "Esperando você" },
];

/** A dot that pulses while an agent works in the project. */
function PulseDot() {
  const reduced = useReducedMotion();
  const pulse = useSharedValue(0);
  useEffect(() => {
    if (reduced) return;
    pulse.value = withRepeat(withTiming(1, { duration: 1600, easing: Easing.out(Easing.quad) }), -1);
    return () => cancelAnimation(pulse);
  }, [pulse, reduced]);
  const ring = useAnimatedStyle(() => ({ opacity: interpolate(pulse.value, [0, 1], [0.55, 0]), transform: [{ scale: interpolate(pulse.value, [0, 1], [1, 2.4]) }] }));
  return <View className="size-[7px] items-center justify-center" accessibilityLabel="Trabalhando">
    <Animated.View className="absolute size-[7px] rounded-full bg-[#7fb4ff]" style={ring} />
    <View className="size-[7px] rounded-full bg-[#7fb4ff]" />
  </View>;
}

function ProjectRow(props: { readonly item: ProjectSummary; readonly selected: boolean; readonly several: boolean; readonly onPress: () => void; readonly onLong: () => void }) {
  const { item } = props;
  const computer = useSirus((state) => props.several ? state.computers.find((entry) => entry.id === item.computerId) : undefined);
  const working = item.rows.filter((row) => row.bucket === "working").length;
  const waiting = item.rows.filter((row) => row.bucket === "attention").length;
  return <Pressable onPress={props.onPress} onLongPress={props.onLong} delayLongPress={350} accessibilityRole="button" accessibilityState={{ selected: props.selected }}
    accessibilityLabel={`${item.project.name}${waiting ? `, ${waiting} esperando` : working ? ", trabalhando" : ""}`}
    className={`flex-row items-center gap-3 rounded-[14px] px-2.5 py-[9px] active:opacity-70 ${props.selected ? "bg-subtle-strong" : ""}`}>
    <ProjectRobot project={item.project} size={30} working={working > 0} />
    <View className="min-w-0 flex-1">
      <AppText numberOfLines={1} className="font-app-medium text-[15.5px]">{item.project.name}</AppText>
      {computer ? <AppText numberOfLines={1} className="text-xs text-foreground-muted">{computerName(computer)}</AppText> : null}
    </View>
    {waiting ? <View className="min-w-[22px] items-center rounded-[7px] bg-[#f0a35e22] px-[7px] py-0.5"><AppText className="font-app-bold text-[11px] text-[#f0a35e]">{waiting}</AppText></View> : working ? <PulseDot /> : null}
  </Pressable>;
}

/**
 * The Projetos drawer: each project's robot with a pulsing dot while an agent works in it and
 * an amber count for what waits on you; a filter and an add button on top, settings and a
 * search at the bottom. Choosing a project filters Conversas and starts Novo there; a long
 * press opens the project's page (changes, pull requests, files, terminal).
 */
export function ProjectsDrawer(props: { readonly open: boolean; readonly onClose: () => void; readonly selected: string | null; readonly onSelect: (key: string | null) => void }) {
  const insets = useSafeAreaInsets();
  const { width } = useWindowDimensions();
  const reduced = useReducedMotion();
  const drawerWidth = Math.min(318, Math.round(width * 0.86));
  const [mounted, setMounted] = useState(props.open);
  const progress = useSharedValue(props.open ? 1 : 0);
  const drag = useSharedValue(0);

  useEffect(() => {
    if (props.open) {
      setMounted(true);
      progress.value = reduced ? 1 : withTiming(1, { duration: 260, easing: Easing.out(Easing.cubic) });
    } else if (mounted) {
      Keyboard.dismiss();
      if (reduced) { progress.value = 0; setMounted(false); }
      else progress.value = withTiming(0, { duration: 200, easing: Easing.in(Easing.cubic) }, (done) => { if (done) runOnJS(setMounted)(false); });
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [props.open, reduced]);

  const close = props.onClose;
  const swipe = useMemo(() => Gesture.Pan().activeOffsetX([-10, 10]).failOffsetY([-16, 16])
    .onUpdate((event) => { drag.value = Math.min(0, event.translationX); })
    .onEnd((event) => {
      if (event.translationX < -drawerWidth * 0.3 || event.velocityX < -600) { runOnJS(close)(); }
      drag.value = withTiming(0, { duration: 160 });
    }), [close, drag, drawerWidth]);
  const panel = useAnimatedStyle(() => ({ transform: [{ translateX: -drawerWidth * (1 - progress.value) + drag.value }] }));
  const dim = useAnimatedStyle(() => ({ opacity: progress.value }));

  // Closed, the drawer is not mounted at all: it reads no conversations until it opens.
  if (!mounted) return null;
  return <View style={{ position: "absolute", top: 0, bottom: 0, left: 0, right: 0, zIndex: 20 }} pointerEvents="box-none">
    <Animated.View style={[{ position: "absolute", top: 0, bottom: 0, left: 0, right: 0, backgroundColor: "#000a" }, dim]}>
      <Pressable style={{ flex: 1 }} onPress={props.onClose} accessibilityLabel="Fechar projetos" accessibilityRole="button" />
    </Animated.View>
    <GestureDetector gesture={swipe}>
      <Animated.View className="absolute inset-y-0 left-0 bg-sheet-solid" style={[{ width: drawerWidth, paddingTop: insets.top + 8, paddingBottom: Math.max(insets.bottom, 12) + 8, paddingHorizontal: 14, shadowColor: "#000", shadowOpacity: 0.5, shadowRadius: 30, shadowOffset: { width: 12, height: 0 } }, panel]}>
        <DrawerBody onClose={props.onClose} selected={props.selected} onSelect={props.onSelect} />
      </Animated.View>
    </GestureDetector>
  </View>;
}

function DrawerBody(props: { readonly onClose: () => void; readonly selected: string | null; readonly onSelect: (key: string | null) => void }) {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const theme = useUniwindTheme();
  const summaries = useProjectSummaries();
  const projects = useProjects();
  const computers = useSirus((state) => state.computers);
  const [show, setShow] = useState<Show>("all");
  const [query, setQuery] = useState("");
  const needle = query.trim().toLocaleLowerCase();
  const shown = useMemo(() => summaries.filter((item) => {
    const working = item.rows.some((row) => row.bucket === "working");
    const waiting = item.rows.some((row) => row.bucket === "attention");
    return (show === "all" || (show === "active" ? working || waiting : waiting)) && (!needle || item.project.name.toLocaleLowerCase().includes(needle));
  }), [summaries, show, needle]);
  const pick = (key: string | null) => { void Haptics.selectionAsync().catch(() => undefined); props.onSelect(key); props.onClose(); };
  const several = computers.length > 1;

  return <>
    <View className="flex-row items-center justify-between">
      <Image source={SIRUS_GLYPH} style={{ width: 30, height: 30, tintColor: theme["--color-foreground"] }} />
      <View className="flex-row gap-2">
        <ControlPillMenu accessibilityLabel="Filtrar projetos" actions={SHOW.map((item) => ({ id: item.id, title: item.title, state: item.id === show ? "on" as const : "off" as const }))}
          onPressAction={({ nativeEvent }) => setShow(nativeEvent.event as Show)}>
          <View className="size-[38px] items-center justify-center rounded-full border border-border-subtle bg-glass-fallback">
            <SymbolView name={(show === "all" ? "line.3.horizontal.decrease" : "line.3.horizontal.decrease.circle.fill") as AppSymbolName} size={19} weight="medium" tintColorClassName="accent-foreground" />
          </View>
        </ControlPillMenu>
        <ControlPillMenu accessibilityLabel="Adicionar projeto" actions={[{ id: "clone", title: "Clonar do GitHub", image: "arrow.down.circle" }, { id: "folder", title: "Adicionar uma pasta", image: "folder.badge.plus" }]}
          onPressAction={({ nativeEvent }) => addProject(nativeEvent.event as "clone" | "folder", projects, pick)}>
          <View className="size-[38px] items-center justify-center rounded-full border border-border-subtle bg-glass-fallback">
            <SymbolView name="plus" size={19} weight="medium" tintColorClassName="accent-foreground" />
          </View>
        </ControlPillMenu>
      </View>
    </View>
    <AppText className="mb-1.5 ml-0.5 mt-[18px] text-[13px] text-foreground-muted">Projetos</AppText>
    <ScrollView className="flex-1" showsVerticalScrollIndicator={false} keyboardShouldPersistTaps="handled" contentContainerClassName="gap-0.5 pb-4">
      <Pressable onPress={() => pick(null)} accessibilityRole="button" className={`flex-row items-center gap-3 rounded-[14px] px-2.5 py-[9px] active:opacity-70 ${props.selected === null ? "bg-subtle-strong" : ""}`}>
        <View className="size-[30px] items-center justify-center"><SymbolView name="square.grid.2x2" size={21} tintColorClassName="accent-foreground-muted" /></View>
        <AppText className="font-app-medium text-[15.5px]">Todos os projetos</AppText>
      </Pressable>
      {shown.map((item) => {
        const key = `${item.computerId}:${item.project.id}`;
        return <ProjectRow key={key} item={item} selected={props.selected === key} several={several} onPress={() => pick(key)}
          onLong={() => { void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => undefined); props.onClose(); navigation.navigate("Project", { computerId: item.computerId, projectId: item.project.id }); }} />;
      })}
      {!shown.length ? <AppText className="mt-6 text-center text-sm text-foreground-muted">{summaries.length ? "Nenhum projeto aqui." : "Nenhum projeto ainda. Adicione um com o botão +."}</AppText> : null}
    </ScrollView>
    {shown.length ? <AppText className="mb-2.5 mt-1 px-2 text-xs text-foreground-tertiary">Toque e segure um projeto para abrir a página dele.</AppText> : null}
    <View className="flex-row items-center gap-2">
      <RoundButton icon="gearshape" label="Ajustes" onPress={() => { props.onClose(); navigation.navigate("Settings"); }} />
      <View className="h-11 flex-1 flex-row items-center gap-2 rounded-[22px] border border-border-subtle bg-glass-fallback px-3.5">
        <SymbolView name="magnifyingglass" size={18} tintColorClassName="accent-foreground-muted" />
        <AppTextInput value={query} onChangeText={setQuery} placeholder="Buscar" autoCapitalize="none" autoCorrect={false} returnKeyType="search"
          className="min-h-0 flex-1 rounded-none border-0 bg-transparent px-0 py-0 text-base" />
      </View>
    </View>
  </>;
}
