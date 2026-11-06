import { useEffect, useState } from "react";
import { AccessibilityInfo, Image, StyleSheet, View } from "react-native";
import * as SplashScreen from "expo-splash-screen";
import Animated, {
  Easing,
  cancelAnimation,
  runOnJS,
  useAnimatedStyle,
  useSharedValue,
  withDelay,
  withRepeat,
  withSequence,
  withTiming,
} from "react-native-reanimated";

import { useSirus } from "../data/store";

const FRAME = require("../../assets/launch/frame.png");
const DOTS = require("../../assets/launch/dots.png");
const CHEVRON = require("../../assets/launch/chevron.png");
const CURSOR = require("../../assets/launch/cursor.png");

/** The same dark as the system splash, in light mode too, so the hand-over shows no seam. */
const BACKGROUND = "#0f0f12";
/** Logo drawn at this width; the layers are 556×446. */
const WIDTH = 168;
const HEIGHT = (WIDTH * 446) / 556;
const SCALE = WIDTH / 556;
/** Where the cursor sits in the layer, in its pixels. */
const CURSOR_FROM = 303 * SCALE;
const CURSOR_TO = 435 * SCALE;
/** Shortest time on screen, and the longest it waits for the computers to answer. */
const MIN_MS = 1500;
const MAX_MS = 5000;
const OPEN_MS = 700;
const CLOSE_MS = 320;
const SQUASHED = 0.012;
const SMOOTH = Easing.bezier(0.2, 0.8, 0.2, 1);

/**
 * The app's opening, like a monitor switching on: a line of light opens into the terminal, the
 * cursor blinks until the computers answer (or five seconds pass), then the terminal closes back
 * into the line, which goes out and leaves Home. Reduce Motion skips it.
 */
export function LaunchOverlay() {
  const ready = useSirus((state) => state.ready && (state.computers.length === 0 || state.computers.some((computer) => !!state.data[computer.id])));
  const [phase, setPhase] = useState<"open" | "close" | "done">("open");
  const [started] = useState(() => Date.now());
  const [waited, setWaited] = useState(false);

  const line = useSharedValue(0);
  const lineAlpha = useSharedValue(1);
  const height = useSharedValue(SQUASHED);
  const parts = useSharedValue(0);
  const chevron = useSharedValue(0);
  const cursor = useSharedValue(0);
  const blink = useSharedValue(1);
  const overlay = useSharedValue(1);

  useEffect(() => {
    void SplashScreen.hideAsync().catch(() => undefined);
    void AccessibilityInfo.isReduceMotionEnabled().then((reduced) => { if (reduced) setPhase("done"); });
    line.value = withTiming(1, { duration: 280, easing: SMOOTH });
    height.value = withDelay(260, withTiming(1, { duration: 400, easing: SMOOTH }));
    lineAlpha.value = withDelay(460, withTiming(0, { duration: 180 }));
    parts.value = withDelay(500, withTiming(1, { duration: 130 }));
    chevron.value = withDelay(OPEN_MS, withTiming(1, { duration: 130 }));
    cursor.value = withDelay(OPEN_MS + 130, withTiming(1, { duration: 180, easing: Easing.linear }));
    blink.value = withDelay(OPEN_MS + 380, withRepeat(withSequence(withTiming(0, { duration: 1 }), withDelay(320, withTiming(1, { duration: 1 })), withDelay(320, withTiming(1, { duration: 1 }))), -1));
    const timer = setTimeout(() => setWaited(true), MAX_MS);
    return () => clearTimeout(timer);
  }, [blink, chevron, cursor, height, line, lineAlpha, parts]);

  useEffect(() => {
    if (phase !== "open" || !(ready || waited)) return;
    const timer = setTimeout(() => setPhase("close"), Math.max(0, MIN_MS - (Date.now() - started)));
    return () => clearTimeout(timer);
  }, [phase, ready, waited, started]);

  useEffect(() => {
    if (phase !== "close") return;
    cancelAnimation(blink);
    blink.value = 1;
    height.value = withTiming(SQUASHED, { duration: CLOSE_MS, easing: Easing.bezier(0.6, 0, 0.8, 0.4) });
    lineAlpha.value = withTiming(1, { duration: 160 });
    parts.value = withDelay(CLOSE_MS - 40, withTiming(0, { duration: 40 }));
    line.value = withDelay(CLOSE_MS, withTiming(0, { duration: 240, easing: Easing.in(Easing.cubic) }));
    overlay.value = withDelay(CLOSE_MS + 100, withTiming(0, { duration: 260 }, (finished) => { if (finished) runOnJS(setPhase)("done"); }));
  }, [phase, blink, height, line, lineAlpha, overlay, parts]);

  const overlayStyle = useAnimatedStyle(() => ({ opacity: overlay.value }));
  const lineStyle = useAnimatedStyle(() => ({ opacity: lineAlpha.value, transform: [{ scaleX: line.value }] }));
  const logoStyle = useAnimatedStyle(() => ({ opacity: parts.value > 0 || height.value > SQUASHED ? 1 : 0, transform: [{ scaleY: height.value }] }));
  const dotsStyle = useAnimatedStyle(() => ({ opacity: parts.value }));
  const chevronStyle = useAnimatedStyle(() => ({ opacity: chevron.value * parts.value }));
  const cursorStyle = useAnimatedStyle(() => ({ opacity: blink.value * parts.value, width: CURSOR_FROM + (CURSOR_TO - CURSOR_FROM) * cursor.value }));

  if (phase === "done") return null;
  return <Animated.View pointerEvents="none" style={[StyleSheet.absoluteFill, styles.overlay, overlayStyle]}>
    <Animated.View style={[styles.logo, logoStyle]}>
      <Image source={FRAME} style={styles.layer} />
      <Animated.Image source={DOTS} style={[styles.layer, dotsStyle]} />
      <Animated.Image source={CHEVRON} style={[styles.layer, chevronStyle]} />
      <Animated.View style={[styles.clip, cursorStyle]}>
        <Image source={CURSOR} style={styles.layer} />
      </Animated.View>
    </Animated.View>
    <View style={styles.lineSlot} pointerEvents="none">
      <Animated.View style={[styles.line, lineStyle]} />
    </View>
  </Animated.View>;
}

const styles = StyleSheet.create({
  overlay: { backgroundColor: BACKGROUND, alignItems: "center", justifyContent: "center" },
  logo: { width: WIDTH, height: HEIGHT },
  layer: { position: "absolute", left: 0, top: 0, width: WIDTH, height: HEIGHT },
  clip: { position: "absolute", left: 0, top: 0, height: HEIGHT, overflow: "hidden" },
  lineSlot: { position: "absolute", width: WIDTH * 1.4, alignItems: "center" },
  line: { width: "100%", height: 2.4, borderRadius: 2, backgroundColor: "#ffffff", shadowColor: "#bdb4f2", shadowOpacity: 0.9, shadowRadius: 8, shadowOffset: { width: 0, height: 0 } },
});
