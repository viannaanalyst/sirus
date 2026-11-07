import { useEffect, useState } from "react";
import { Pressable, View } from "react-native";
import Animated, { Easing, useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";

import { inkOn, useMacAccent } from "../data/mac-accent";
import { AppText } from "./AppText";

/** The Mac's segment thumb: 220 ms, ease-out (`segment_pill.rs`). */
const SLIDE = { duration: 220, easing: Easing.out(Easing.exp) };
const TRACK_PADDING = 3;

export interface SegmentedControlProps<Value extends string | number> {
  readonly options: readonly { readonly value: Value; readonly label: string }[];
  readonly selected: Value;
  readonly onSelect: (value: Value) => void;
  /** Compact is 32 points tall; the default is 36. */
  readonly size?: "default" | "compact";
  readonly className?: string;
}

/**
 * The Mac's segmented control: equal segments on a quiet track, and one thumb in the Mac's accent
 * that slides to the chosen segment, whose label takes the ink that reads on it.
 */
export function SegmentedControl<Value extends string | number>(props: SegmentedControlProps<Value>) {
  const accent = useMacAccent();
  const count = Math.max(props.options.length, 1);
  const chosen = Math.max(props.options.findIndex((option) => option.value === props.selected), 0);
  const [width, setWidth] = useState(0);
  const cell = useSharedValue(0);
  const place = useSharedValue(chosen);
  useEffect(() => {
    place.value = withTiming(chosen, SLIDE);
  }, [chosen, place]);
  const thumb = useAnimatedStyle(() => ({
    width: cell.value,
    transform: [{ translateX: place.value * cell.value }],
  }));
  const ink = inkOn(accent);
  return <View className={props.className}>
    <View
      className="flex-row rounded-[10px] bg-card"
      style={{ height: props.size === "compact" ? 32 : 36, padding: TRACK_PADDING }}
      accessibilityRole="tablist"
      onLayout={(event) => {
        const inner = event.nativeEvent.layout.width - TRACK_PADDING * 2;
        setWidth(inner);
        cell.value = inner / count;
      }}>
      {width > 0 ? <Animated.View
        pointerEvents="none"
        style={[thumb, { position: "absolute", top: TRACK_PADDING, bottom: TRACK_PADDING, left: TRACK_PADDING, borderRadius: 7, backgroundColor: accent }]} /> : null}
      {props.options.map((option, index) => <Pressable
        key={String(option.value)}
        onPress={() => props.onSelect(option.value)}
        accessibilityRole="tab"
        accessibilityState={{ selected: index === chosen }}
        className="flex-1 items-center justify-center active:opacity-70">
        <AppText numberOfLines={1} className={index === chosen ? "text-[13px] font-app-medium" : "text-[13px] text-foreground-muted"} style={index === chosen ? { color: ink } : undefined}>{option.label}</AppText>
      </Pressable>)}
    </View>
  </View>;
}
