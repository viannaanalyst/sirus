import * as Haptics from "expo-haptics";
import { useEffect } from "react";
import { Alert, Pressable, View } from "react-native";
import { Gesture, GestureDetector } from "react-native-gesture-handler";
import Animated, { FadeIn, FadeOut, runOnJS, SlideInUp, SlideOutUp, useAnimatedStyle, useReducedMotion, useSharedValue, withSpring, withTiming } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { Banner, BannerKind } from "../data/banner-transitions";
import { startBanners, useBanners } from "../data/banners";
import { inkOn, useMacAccent } from "../data/mac-accent";
import { navigationRef } from "../data/navigation-ref";
import { useSirus } from "../data/store";
import { AppText } from "./AppText";
import { ProjectRobot } from "./ProjectRobot";

const LABEL: Record<BannerKind, { text: string; color: string | null }> = {
  approval: { text: "Precisa de você", color: "#f0a35e" },
  input: { text: "Tem uma pergunta", color: null },
  done: { text: "Terminou", color: "#5fce8c" },
  failed: { text: "Falhou", color: "#ee8a96" },
};

/**
 * Banners for the other conversations (option C1 of the avisos design): while the app is open,
 * a conversation that is not on screen and asks for a permission, asks a question, finishes or
 * fails drops a banner from the top with its actions. Swipe up to dismiss; finished ones leave
 * on their own; asks stay until answered, here or anywhere. The newest shows, with a pill
 * counting the others. There is no system push, by decision.
 */
export function NotificationBanners() {
  const shown = useBanners((state) => state.shown);
  const insets = useSafeAreaInsets();
  useEffect(() => startBanners(), []);
  const newest = shown[shown.length - 1];
  const more = shown.length - 1;
  return <View pointerEvents="box-none" style={{ position: "absolute", top: 6, left: 10, right: 10, zIndex: 60, gap: 6 }}>
    {newest ? <BannerCard key={newest.id} banner={newest} topPad={Math.max(insets.top - 6, 10)} /> : null}
    {more > 0 ? <Animated.View entering={FadeIn.duration(160)} exiting={FadeOut.duration(120)} className="self-center rounded-xl bg-sheet-solid px-[11px] py-1" accessibilityLiveRegion="polite">
      <AppText className="text-[12.5px] text-foreground-secondary">+{more} {more === 1 ? "aviso" : "avisos"}</AppText>
    </Animated.View> : null}
  </View>;
}

function BannerCard(props: { readonly banner: Banner; readonly topPad: number }) {
  const { banner } = props;
  const accent = useMacAccent(banner.computerId);
  const reduced = useReducedMotion();
  const project = useSirus((state) => state.data[banner.computerId]?.projects.find((item) => item.id === banner.projectId));
  const dismiss = useBanners((state) => state.dismiss);
  const drag = useSharedValue(0);
  const label = LABEL[banner.kind];
  const color = label.color ?? accent;
  const close = () => dismiss(banner.id);
  const open = () => { close(); navigationRef.navigate("Thread", { computerId: banner.computerId, sessionId: banner.sessionId }); };
  const answer = (decision: "accept" | "decline") => {
    if (!banner.request) return open();
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light).catch(() => undefined);
    close();
    useSirus.getState().respondRequest(banner.computerId, banner.sessionId, banner.request, { type: "approval", decision })
      .catch((reason: unknown) => Alert.alert("Não foi possível responder", reason instanceof Error ? reason.message : String(reason)));
  };
  const swipe = Gesture.Pan().activeOffsetY([-8, 8]).failOffsetX([-24, 24])
    .onUpdate((event) => { drag.value = Math.min(0, event.translationY); })
    .onEnd((event) => {
      if (event.translationY < -36 || event.velocityY < -500) { drag.value = withTiming(-220, { duration: 160 }); runOnJS(close)(); }
      else drag.value = withSpring(0, { damping: 18, stiffness: 260 });
    });
  const style = useAnimatedStyle(() => ({ transform: [{ translateY: drag.value }] }));
  const ask = banner.kind === "approval" && !!banner.request;
  return <GestureDetector gesture={swipe}>
    <Animated.View entering={reduced ? FadeIn.duration(120) : SlideInUp.springify().damping(16).stiffness(190)} exiting={reduced ? FadeOut.duration(100) : SlideOutUp.duration(180)}
      accessibilityLiveRegion="assertive"
      className="rounded-[26px] border bg-sheet-solid px-3 pb-3" style={[{ paddingTop: props.topPad, borderColor: `${color}66`, shadowColor: "#000", shadowOpacity: 0.45, shadowRadius: 24, shadowOffset: { width: 0, height: 14 } }, style]}>
      <Pressable onPress={open} accessibilityRole="button" accessibilityLabel={`${banner.title}, ${label.text}, ${banner.projectName}`} className="flex-row items-center gap-2.5">
        <ProjectRobot project={project} name={banner.projectName} size={30} />
        <View className="min-w-0 flex-1">
          <AppText numberOfLines={1} className="font-app-bold text-[13.5px]">{banner.title}</AppText>
          <AppText numberOfLines={1} className="text-xs text-foreground-muted"><AppText className="text-xs" style={{ color }}>{label.text}</AppText> · {banner.projectName}</AppText>
        </View>
        <Pressable onPress={close} hitSlop={10} accessibilityRole="button" accessibilityLabel="Dispensar" className="size-6 items-center justify-center rounded-full active:opacity-60">
          <AppText className="text-base leading-[18px] text-foreground-muted">×</AppText>
        </Pressable>
      </Pressable>
      <AppText numberOfLines={3} className="mt-2 text-[13px] leading-[18px] text-foreground-secondary">
        {banner.message.text}{banner.message.code ? <AppText className="font-mono text-xs"> {banner.message.code}</AppText> : null}
      </AppText>
      <View className="mt-2.5 flex-row gap-[7px]">
        {ask ? <>
          <ActionButton label="Negar" onPress={() => answer("decline")} />
          <ActionButton label="Aprovar" primary accent={accent} onPress={() => answer("accept")} />
        </> : banner.kind === "input" ? <>
          <ActionButton label="Abrir" onPress={open} />
          <ActionButton label="Responder" primary accent={accent} onPress={open} />
        </> : <ActionButton label="Abrir" onPress={open} />}
      </View>
    </Animated.View>
  </GestureDetector>;
}

function ActionButton(props: { readonly label: string; readonly onPress: () => void; readonly primary?: boolean; readonly accent?: string }) {
  return <Pressable onPress={props.onPress} accessibilityRole="button" accessibilityLabel={props.label}
    className={`h-9 flex-1 items-center justify-center rounded-[10px] active:opacity-70 ${props.primary ? "" : "bg-subtle-strong"}`} style={props.primary ? { backgroundColor: props.accent } : undefined}>
    <AppText className="font-app-medium text-[13px]" style={props.primary && props.accent ? { color: inkOn(props.accent) } : undefined}>{props.label}</AppText>
  </Pressable>;
}
