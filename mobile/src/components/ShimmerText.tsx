import { useEffect } from "react";
import Animated, { cancelAnimation, interpolateColor, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withTiming, Easing } from "react-native-reanimated";

import { useUniwindTheme } from "../lib/useUniwindTheme";
import { AppText } from "./AppText";

const AnimatedText = Animated.createAnimatedComponent(AppText);

/**
 * A line that breathes between muted and bright while something runs (the design's shimmer;
 * React Native cannot clip a gradient to text, so the colour itself sweeps). Still with Reduce Motion.
 */
export function ShimmerText(props: { readonly text: string; readonly size?: number }) {
  const theme = useUniwindTheme();
  const reduced = useReducedMotion();
  const phase = useSharedValue(0);
  useEffect(() => {
    if (reduced) { phase.value = 0; return; }
    phase.value = withRepeat(withTiming(1, { duration: 1100, easing: Easing.inOut(Easing.quad) }), -1, true);
    return () => cancelAnimation(phase);
  }, [phase, reduced]);
  const from = theme["--color-foreground-muted"], to = theme["--color-foreground"];
  const style = useAnimatedStyle(() => ({ color: interpolateColor(phase.value, [0, 1], [from, to]) }));
  return <AnimatedText numberOfLines={1} className="mt-[3px]" style={[{ fontSize: props.size ?? 12.5 }, style]}>{props.text}</AnimatedText>;
}
