import { useEffect } from "react";
import Animated, { Easing, useAnimatedStyle, useSharedValue, withRepeat, withTiming } from "react-native-reanimated";

import { AppText, type AppTextProps } from "../AppText";

const AnimatedText = Animated.createAnimatedComponent(AppText);

/** Text that breathes while something runs: the live row of the tree, the "Pensando" line. */
export function ShimmerText(props: AppTextProps) {
  const phase = useSharedValue(0);
  useEffect(() => {
    phase.value = withRepeat(withTiming(1, { duration: 1100, easing: Easing.inOut(Easing.quad) }), -1, true);
  }, [phase]);
  const style = useAnimatedStyle(() => ({ opacity: 0.45 + 0.55 * phase.value }));
  return <AnimatedText {...props} style={[props.style, style]} />;
}
