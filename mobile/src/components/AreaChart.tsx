import { memo, useEffect, useId, useMemo, useState } from "react";
import { View } from "react-native";
import { Gesture, GestureDetector } from "react-native-gesture-handler";
import Animated, { Easing, runOnJS, useAnimatedProps, useReducedMotion, useSharedValue, withTiming } from "react-native-reanimated";
import Svg, { ClipPath, Circle, Defs, Line, LinearGradient, Path, Rect, Stop, Text as SvgText } from "react-native-svg";

import { indexAt, labelX, stackedLayout, type AreaSeries } from "../lib/chart-path";

const AnimatedRect = Animated.createAnimatedComponent(Rect);

/**
 * A stacked smooth area chart (the design's `mountChart`): the series pile up bottom to top,
 * the areas fade in and the lines draw in left to right when the data changes, and a finger
 * dragging across reads one bucket (`onRead(index)`, `null` when the finger lifts). Vertical
 * drags still scroll the page; only a mostly horizontal drag or a tap reads. No chart library:
 * react-native-svg paths, Reanimated for the reveal and Gesture Handler for the touch.
 */
export const AreaChart = memo(function AreaChart(props: {
  readonly series: readonly AreaSeries[];
  readonly labels: readonly string[];
  readonly height?: number;
  readonly grid: string;
  readonly labelColor: string;
  readonly cursorColor: string;
  readonly knockout: string;
  /** Draws only the first `drawn` points; the x axis keeps all the labels. */
  readonly drawn?: number;
  readonly onRead?: (index: number | null) => void;
  /** Changes when the data is a different one (a new range); the reveal runs again. */
  readonly revision: string;
}) {
  const height = props.height ?? 170;
  const uid = useId().replace(/[^a-zA-Z0-9]/g, "");
  const [width, setWidth] = useState(0);
  const [cursor, setCursor] = useState<number | null>(null);
  const reduced = useReducedMotion();
  const reveal = useSharedValue(reduced ? 1 : 0);
  const count = Math.max(2, props.labels.length);
  const layout = useMemo(() => width > 0 ? stackedLayout(props.series, width, height, 20, props.drawn) : null, [props.series, width, height, props.drawn]);

  useEffect(() => {
    if (reduced) { reveal.value = 1; return; }
    reveal.value = 0;
    reveal.value = withTiming(1, { duration: 1100, easing: Easing.bezier(0.4, 0, 0.2, 1) });
  }, [props.revision, reduced, reveal]);
  const clip = useAnimatedProps(() => ({ width: reveal.value * (width + 4) }));

  const widthValue = useSharedValue(0);
  widthValue.value = width;
  const lastIndex = useSharedValue(-1);
  const { onRead, drawn } = props;
  const lastDrawn = Math.max(0, Math.min(count, drawn ?? count) - 1);
  const read = (index: number | null) => { setCursor(index); onRead?.(index); };
  const gesture = useMemo(() => {
    const move = (x: number) => {
      "worklet";
      const index = Math.min(lastDrawn, indexAt(x, widthValue.value, count));
      if (index !== lastIndex.value) { lastIndex.value = index; runOnJS(read)(index); }
    };
    const end = () => {
      "worklet";
      lastIndex.value = -1;
      runOnJS(read)(null);
    };
    const pan = Gesture.Pan().activeOffsetX([-6, 6]).failOffsetY([-14, 14]).onUpdate((event) => move(event.x)).onFinalize(end);
    const tap = Gesture.Tap().maxDuration(10_000).onBegin((event) => move(event.x)).onFinalize(end);
    return Gesture.Race(pan, tap);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [count, lastDrawn, lastIndex, widthValue, onRead]);

  const step = Math.ceil(props.labels.length / 4.5);
  return <GestureDetector gesture={gesture}>
    <View style={{ height }} onLayout={(event) => setWidth(Math.round(event.nativeEvent.layout.width))} accessible accessibilityRole="image" accessibilityLabel="Gráfico">
      {layout ? <Svg width={width} height={height} style={{ overflow: "visible" }}>
        <Defs>
          {layout.layers.map((layer) => <LinearGradient key={layer.key} id={`g${uid}${layer.key}`} x1="0" y1="0" x2="0" y2="1">
            <Stop offset="0" stopColor={layer.color} stopOpacity={0.5} /><Stop offset="1" stopColor={layer.color} stopOpacity={0.04} />
          </LinearGradient>)}
          <ClipPath id={`c${uid}`}><AnimatedRect x={0} y={-4} height={height + 8} animatedProps={clip} /></ClipPath>
        </Defs>
        {[0.33, 0.66, 1].map((fraction) => <Line key={fraction} x1={0} x2={width} y1={layout.y((layout.max / 1.12) * fraction)} y2={layout.y((layout.max / 1.12) * fraction)} stroke={props.grid} strokeDasharray="2 5" />)}
        {layout.layers.map((layer) => <Path key={`a-${layer.key}`} d={layer.area} fill={`url(#g${uid}${layer.key})`} clipPath={`url(#c${uid})`} />)}
        {layout.layers.map((layer) => <Path key={`l-${layer.key}`} d={layer.line} fill="none" stroke={layer.color} strokeWidth={2.2} strokeLinecap="round" clipPath={`url(#c${uid})`} />)}
        {props.labels.map((label, index) => index % step === 0 ? <SvgText key={index} x={labelX(layout.x(index), width, label.length)} y={height - 3} fontSize={10.5} fill={props.labelColor} textAnchor="middle">{label}</SvgText> : null)}
        {cursor !== null ? <>
          <Line x1={layout.x(cursor)} x2={layout.x(cursor)} y1={4} y2={height - 20} stroke={props.cursorColor} strokeOpacity={0.35} />
          {layout.layers.map((layer) => <Circle key={layer.key} cx={layout.x(cursor)} cy={layer.top[Math.min(cursor, layer.top.length - 1)]![1]} r={4} fill={layer.color} stroke={props.knockout} strokeWidth={2} />)}
        </> : null}
      </Svg> : null}
    </View>
  </GestureDetector>;
});
