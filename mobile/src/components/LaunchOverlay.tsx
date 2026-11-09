import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AccessibilityInfo, StyleSheet, View, useWindowDimensions } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import * as SplashScreen from "expo-splash-screen";
import Animated, {
  Easing,
  runOnJS,
  useAnimatedStyle,
  useSharedValue,
  withTiming,
  type SharedValue,
} from "react-native-reanimated";

import { useSirus } from "../data/store";
import { DOT_GRID, LAUNCH_DOTS } from "./launchDots";

const BUST = require("../../assets/splash.png");

/** The same dark and picture size as the system splash (app.config.ts), so the hand-over shows no seam. */
const BACKGROUND = "#0f0f12";
const BUST_SIZE = 309;
const CELL = BUST_SIZE / DOT_GRID;
const DOT_COLOR = "#f2efe9";
/** The dots reach the list rows at this time (ms); the overlay then waits for the data and fades. */
const SETTLE_MS = 1060;
const FADE_MS = 240;
/** Shortest time on screen (1.3 s in all) and the longest the overlay can ever stay. */
const MIN_MS = 1300;
const MAX_MS = 5000;
/** Where the home list's rows sit, used as the dots' destinations. */
const ROW_TOP = 68;
const ROW_STEP = 72;
const ROW_COUNT = 5;

function clamp(v: number, a: number, b: number) {
  "worklet";
  return Math.min(b, Math.max(a, v));
}
function seg(t: number, a: number, b: number) {
  "worklet";
  return clamp((t - a) / (b - a), 0, 1);
}
function outCubic(p: number) {
  "worklet";
  return 1 - Math.pow(1 - p, 3);
}
function inOutCubic(p: number) {
  "worklet";
  return p < 0.5 ? 4 * p * p * p : 1 - Math.pow(-2 * p + 2, 3) / 2;
}

type Dot = { readonly sx: number; readonly sy: number; readonly r: number; readonly tx: number; readonly ty: number; readonly nx: number; readonly ny: number; readonly bend: number; readonly h1: number; readonly h2: number };

/** Each bust dot gets a destination (in the picture's own coordinates) on a row of the list, in reading order, and a sideways bend for its path. */
function buildDots(width: number, height: number, top: number): Dot[] {
  const originX = (width - BUST_SIZE) / 2;
  const originY = (height - BUST_SIZE) / 2;
  const targets: { x: number; y: number }[] = [];
  for (let row = 0; row < ROW_COUNT; row++) {
    const rowTop = top + ROW_TOP + row * ROW_STEP;
    for (let y = rowTop + 10; y <= rowTop + 54; y += 7) for (let x = 30; x <= width - 30; x += 7) targets.push({ x: x - originX, y: y - originY });
  }
  const count = LAUNCH_DOTS.length;
  return LAUNCH_DOTS.map(([fx, fy, radius, h1, h2, h3], index) => {
    const sx = fx * BUST_SIZE;
    const sy = fy * BUST_SIZE;
    const target = targets[Math.floor((index * targets.length) / count)] ?? { x: sx, y: sy };
    const length = Math.hypot(target.x - sx, target.y - sy) || 1;
    return { sx, sy, r: Math.max(0.5, radius * CELL), tx: target.x, ty: target.y, nx: -(target.y - sy) / length, ny: (target.x - sx) / length, bend: (h3 - 0.5) * 90, h1, h2 };
  });
}

const DotView = memo(function DotView({ dot, t }: { readonly dot: Dot; readonly t: SharedValue<number> }) {
  const style = useAnimatedStyle(() => {
    const time = t.value;
    const appear = outCubic(seg(time, dot.h1 * 120, dot.h1 * 120 + 200));
    const start = 320 + dot.h2 * 140;
    const p = inOutCubic(seg(time, start, start + 600));
    const off = dot.bend * Math.sin(Math.PI * p);
    const radius = (dot.r * (1 - p) + 1.5 * p) * appear;
    return {
      transform: [
        { translateX: (dot.tx - dot.sx) * p + dot.nx * off },
        { translateY: (dot.ty - dot.sy) * p + dot.ny * off },
        { scale: radius / dot.r },
      ],
    };
  });
  return <Animated.View style={[styles.dot, { left: dot.sx - dot.r, top: dot.sy - dot.r, width: dot.r * 2, height: dot.r * 2, borderRadius: dot.r }, style]} />;
});

/**
 * The app's opening: the halftone bust of the system splash dissolves into dots that fly out and
 * settle on the home list's rows, then the overlay fades and shows the real screen. It holds the
 * dots on the rows until the computers answer (five seconds at most). Reduce Motion gets a quick
 * crossfade instead. Anything that fails hides the overlay.
 */
export function LaunchOverlay() {
  const ready = useSirus((state) => state.ready && (state.computers.length === 0 || state.computers.some((computer) => !!state.data[computer.id])));
  const { width, height } = useWindowDimensions();
  const insets = useSafeAreaInsets();
  const [done, setDone] = useState(false);
  const [reduced, setReduced] = useState<boolean | null>(null);
  const [settled, setSettled] = useState(false);
  const [waited, setWaited] = useState(false);
  const started = useRef(0);
  const begun = useRef(false);
  const t = useSharedValue(0);
  const overlay = useSharedValue(1);
  const dots = useMemo(() => buildDots(width, height, insets.top), [width, height, insets.top]);

  const finish = useCallback(() => setDone(true), []);

  const begin = useCallback(() => {
    if (begun.current) return;
    begun.current = true;
    started.current = Date.now();
    void SplashScreen.hideAsync().catch(() => undefined);
    void AccessibilityInfo.isReduceMotionEnabled().then(setReduced, () => setReduced(false));
  }, []);

  // The system splash goes once the picture is drawn, so the first frame matches it; a timer covers a load that never reports.
  useEffect(() => {
    const timers = [setTimeout(begin, 400), setTimeout(() => setWaited(true), MAX_MS), setTimeout(finish, MAX_MS + 2000)];
    return () => timers.forEach(clearTimeout);
  }, [begin, finish]);

  useEffect(() => {
    if (reduced === null) return;
    try {
      if (reduced) {
        setSettled(true);
        return;
      }
      t.value = withTiming(SETTLE_MS, { duration: SETTLE_MS, easing: Easing.linear }, (finished) => { if (finished) runOnJS(setSettled)(true); });
    } catch {
      finish();
    }
  }, [reduced, t, finish]);

  useEffect(() => {
    if (!settled || !(ready || waited)) return;
    const wait = reduced ? 0 : Math.max(0, MIN_MS - FADE_MS - (Date.now() - started.current));
    const timer = setTimeout(() => {
      try {
        overlay.value = withTiming(0, { duration: reduced ? 200 : FADE_MS, easing: Easing.inOut(Easing.cubic) }, (finished) => { if (finished) runOnJS(finish)(); });
      } catch {
        finish();
      }
    }, wait);
    return () => clearTimeout(timer);
  }, [settled, ready, waited, reduced, overlay, finish]);

  const overlayStyle = useAnimatedStyle(() => ({ opacity: overlay.value }));
  const bustStyle = useAnimatedStyle(() => {
    const dissolve = inOutCubic(seg(t.value, 0, 320));
    return { opacity: 1 - dissolve, transform: [{ scale: 1 - 0.05 * dissolve }] };
  });

  if (done) return null;
  return <Animated.View pointerEvents="none" style={[StyleSheet.absoluteFill, styles.overlay, overlayStyle]}>
    <View style={[styles.box, { left: (width - BUST_SIZE) / 2, top: (height - BUST_SIZE) / 2 }]}>
      <Animated.Image source={BUST} onLoadEnd={begin} onError={finish} style={[styles.bust, bustStyle]} />
      {reduced ? null : dots.map((dot, index) => <DotView key={index} dot={dot} t={t} />)}
    </View>
  </Animated.View>;
}

const styles = StyleSheet.create({
  overlay: { backgroundColor: BACKGROUND },
  box: { position: "absolute", width: BUST_SIZE, height: BUST_SIZE },
  bust: { width: BUST_SIZE, height: BUST_SIZE },
  dot: { position: "absolute", backgroundColor: DOT_COLOR },
});
