import { useMemo } from "react";
import { View } from "react-native";
import Animated, { makeMutable, type SharedValue, useAnimatedStyle, useFrameCallback, useReducedMotion, useSharedValue } from "react-native-reanimated";
import { SvgXml } from "react-native-svg";

import { avatarLook, avatarSvg } from "../lib/project-avatar";

const BARS = 14;
/** The voice's level, 0 to 1, as `ComposerSend` hears it. */
export const dictationLevel = makeMutable(0);

function ListenBar(props: { readonly index: number; readonly side: 1 | -1; readonly color: string; readonly clock: SharedValue<number>; readonly level: SharedValue<number> }) {
  const { index, side, clock, level } = props;
  const style = useAnimatedStyle(() => {
    const wave = 0.5 + 0.5 * Math.sin(clock.value * 8 + index * 0.9 * side);
    return { height: 2 + wave * level.value * 20 * (1 - index / (BARS + 1)) };
  });
  return <Animated.View style={[style, { width: 2.5, borderRadius: 2, backgroundColor: props.color, opacity: 0.75 * (1 - index / BARS) }]} />;
}

/**
 * While the microphone records, the project's robot stands over the composer and hops with the
 * voice, with bars on both sides, as on the Mac (`composer_listener`). Reduce Motion keeps it
 * still and the bars steady.
 */
export function ListeningRobot(props: { readonly name: string; readonly appearance?: { readonly color?: string; readonly eyes?: string; readonly accessory?: string } }) {
  const look = useMemo(() => avatarLook(props.name, props.appearance), [props.name, props.appearance]);
  const xml = useMemo(() => avatarSvg(look), [look]);
  const reduced = useReducedMotion();
  const clock = useSharedValue(0);
  const shown = useSharedValue(0);
  useFrameCallback((frame) => {
    if (!reduced) clock.value = frame.timeSinceFirstFrame / 1000;
    // It rises fast and falls slowly, as a voice meter does.
    const target = dictationLevel.value;
    shown.value += (target - shown.value) * (target > shown.value ? 0.45 : 0.14);
  });
  const hop = useAnimatedStyle(() => ({
    transform: [{ translateY: reduced ? 0 : -shown.value * 8 * Math.abs(Math.sin(clock.value * 6.8)) }],
  }));
  const bars = (side: 1 | -1) => Array.from({ length: BARS }, (_, index) => <ListenBar key={index} index={index} side={side} color={look.color} clock={clock} level={shown} />);
  return <View pointerEvents="none" style={{ position: "absolute", top: -38, left: 0, right: 0, height: 34, flexDirection: "row", alignItems: "flex-end", justifyContent: "center", gap: 3.5, paddingBottom: 4 }}>
    {bars(-1).reverse()}
    <Animated.View style={[hop, { marginHorizontal: 8 }]}><SvgXml xml={xml} width={26} height={26} /></Animated.View>
    {bars(1)}
  </View>;
}
