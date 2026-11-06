import type { Project } from "@sirus/client/types";
import { memo, useEffect } from "react";
import Animated, { cancelAnimation, useAnimatedStyle, useReducedMotion, useSharedValue, withRepeat, withSequence, withTiming } from "react-native-reanimated";
import Svg, { Defs, G, LinearGradient, Path, Rect, Stop } from "react-native-svg";

import { robotGradient, robotLook, ROBOT_PATHS, type RobotEyes } from "../data/project-robot";

const INK = "#f4f5f8";

function Eyes(props: { readonly eyes: RobotEyes }) {
  return <>{[24.5, 39.5].map((x) => {
    if (props.eyes === "dots") return <Rect key={x} x={x - 4.2} y={27.8} width={8.4} height={8.4} rx={4.2} fill={INK} />;
    if (props.eyes === "sleepy") return <Rect key={x} x={x - 4.5} y={30.4} width={9} height={3.2} rx={1.6} fill={INK} />;
    if (props.eyes === "happy") return <Path key={x} d={`M${x - 5} 34Q${x} 27 ${x + 5} 34`} stroke={INK} strokeWidth={3.2} fill="none" strokeLinecap="round" />;
    return <Rect key={x} x={x - 3.8} y={26.2} width={7.6} height={11.5} rx={3.8} fill={INK} />;
  })}</>;
}

/**
 * A project's robot, as the Mac draws it in its sidebar (`project_avatar.rs`): body in the
 * project's colour, dark visor, eyes. While `working` it hops (two steps, half a second);
 * with Reduce Motion it stays still. Gradient ids are per colour and shape, so two robots of
 * one look share one definition.
 */
export const ProjectRobot = memo(function ProjectRobot(props: { readonly project: Pick<Project, "name" | "appearance"> | undefined; readonly size?: number; readonly working?: boolean; readonly name?: string }) {
  const size = props.size ?? 30;
  const look = robotLook(props.project?.name ?? props.name ?? "", props.project?.appearance);
  const { top, bottom } = robotGradient(look.color);
  const id = `r${look.color.slice(1)}${look.shape}`;
  const reduced = useReducedMotion();
  const hop = useSharedValue(0);
  const hopping = !!props.working && !reduced;
  useEffect(() => {
    if (!hopping) { cancelAnimation(hop); hop.value = 0; return; }
    hop.value = withRepeat(withSequence(withTiming(0, { duration: 250 }), withTiming(-size * 0.07, { duration: 0 }), withTiming(-size * 0.07, { duration: 250 }), withTiming(0, { duration: 0 })), -1);
    return () => cancelAnimation(hop);
  }, [hopping, hop, size]);
  const style = useAnimatedStyle(() => ({ transform: [{ translateY: hop.value }] }));
  return <Animated.View style={[{ width: size, height: size }, style]} accessibilityElementsHidden importantForAccessibility="no-hide-descendants">
    <Svg width={size} height={size} viewBox="0 0 64 64">
      <Defs>
        <LinearGradient id={id} x1="0" y1="0" x2="0" y2="1"><Stop offset="0" stopColor={top} /><Stop offset="1" stopColor={bottom} /></LinearGradient>
        <LinearGradient id={`${id}s`} x1="0" y1="0" x2="0" y2="1"><Stop offset="0" stopColor="#ffffff" stopOpacity={0.45} /><Stop offset="0.55" stopColor="#ffffff" stopOpacity={0} /></LinearGradient>
      </Defs>
      {look.shape === "tv" ? <Path d="M23 15 16 6M41 15l7-9" fill="none" stroke={bottom} strokeWidth={3.5} strokeLinecap="round" /> : null}
      <G><Path d={ROBOT_PATHS[look.shape]} fill={`url(#${id})`} /></G>
      <G opacity={0.55}><Path d={ROBOT_PATHS[look.shape]} fill={`url(#${id}s)`} /></G>
      <Rect x={13} y={22} width={38} height={20} rx={10} fill="#14151b" />
      <Eyes eyes={look.eyes} />
    </Svg>
  </Animated.View>;
});
