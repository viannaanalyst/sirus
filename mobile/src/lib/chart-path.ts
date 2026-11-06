/** Pure geometry for the phone's area charts (the design's `smooth` and `mountChart`). */
export type Point = readonly [number, number];

/**
 * A smooth path through the points that never leaves the range of its neighbours: monotone
 * cubic interpolation (Fritsch-Carlson tangents, as d3's `curveMonotoneX`), so a spike next to
 * zero rises and falls without dipping under the baseline. Works with x running either way.
 */
export function smoothPath(points: readonly Point[]): string {
  if (!points.length) return "";
  const n = points.length;
  let d = `M${round(points[0]![0])} ${round(points[0]![1])}`;
  if (n === 1) return d;
  const h = Array.from({ length: n - 1 }, (_, i) => points[i + 1]![0] - points[i]![0]);
  const s = h.map((width, i) => width === 0 ? 0 : (points[i + 1]![1] - points[i]![1]) / width);
  const sign = (value: number) => value < 0 ? -1 : value > 0 ? 1 : 0;
  const m: number[] = new Array(n).fill(0);
  for (let i = 1; i < n - 1; i += 1) {
    if (s[i - 1]! * s[i]! <= 0) continue;
    const p = (s[i - 1]! * h[i]! + s[i]! * h[i - 1]!) / (h[i - 1]! + h[i]!);
    m[i] = (sign(s[i - 1]!) + sign(s[i]!)) * Math.min(Math.abs(s[i - 1]!), Math.abs(s[i]!), 0.5 * Math.abs(p));
  }
  // Ends: the segment's own slope (no overshoot there either).
  m[0] = s[0]!;
  m[n - 1] = s[n - 2]!;
  for (let i = 0; i < n - 1; i += 1) {
    const [x1, y1] = points[i]!, [x2, y2] = points[i + 1]!;
    const third = h[i]! / 3;
    d += `C${round(x1 + third)} ${round(y1 + m[i]! * third)} ${round(x2 - third)} ${round(y2 - m[i + 1]! * third)} ${round(x2)} ${round(y2)}`;
  }
  return d;
}

const round = (value: number) => Math.round(value * 100) / 100;

export interface AreaSeries { readonly key: string; readonly color: string; readonly values: readonly number[] }
export interface StackedLayout {
  readonly width: number;
  readonly height: number;
  readonly max: number;
  readonly totals: readonly number[];
  /** For each series (bottom first): the closed area, its top line, and the top's points. */
  readonly layers: readonly { readonly key: string; readonly color: string; readonly area: string; readonly line: string; readonly top: readonly Point[] }[];
  readonly x: (index: number) => number;
  readonly y: (value: number) => number;
}

/**
 * Stacks the series on top of each other and lays them out in `width` x `height`, leaving
 * `bottom` for labels. A single point is drawn as a flat line so one bucket still shows.
 * `drawn` draws only the first points while the x axis keeps the full length (a day that
 * has not ended).
 */
export function stackedLayout(series: readonly AreaSeries[], width: number, height: number, bottom = 20, drawn?: number): StackedLayout {
  const count = Math.max(2, ...series.map((item) => item.values.length));
  const at = (item: AreaSeries, index: number) => item.values[Math.min(index, item.values.length - 1)] ?? 0;
  const totals = Array.from({ length: count }, (_, index) => series.reduce((sum, item) => sum + at(item, index), 0));
  const max = Math.max(...totals) * 1.12 || 1;
  const x = (index: number) => (index / (count - 1)) * width;
  const y = (value: number) => height - bottom - (value / max) * (height - bottom - 6);
  const last = Math.max(1, Math.min(count, drawn ?? count));
  let accumulated = Array.from({ length: count }, () => 0);
  const layers = series.map((item) => {
    const low = accumulated;
    accumulated = accumulated.map((value, index) => value + at(item, index));
    const top: Point[] = accumulated.slice(0, last).map((value, index) => [x(index), y(value)]);
    const down: Point[] = low.slice(0, last).map((value, index): Point => [x(index), y(value)]).reverse();
    const line = smoothPath(top);
    return { key: item.key, color: item.color, line, top, area: `${line}${smoothPath(down).replace(/^M/, "L")}Z` };
  });
  return { width, height, max, totals, layers, x, y };
}

/** The index nearest to a touch at `x` over a chart `width` wide with `count` points. */
export function indexAt(x: number, width: number, count: number): number {
  "worklet";
  if (count < 2 || width <= 0) return 0;
  return Math.max(0, Math.min(count - 1, Math.round((x / width) * (count - 1))));
}

/**
 * Where an axis label's centre goes: under its point, but kept far enough from both edges
 * that the whole label (about `chars` wide at `fontSize`) stays inside the chart.
 */
export function labelX(x: number, width: number, chars: number, fontSize = 10.5): number {
  const half = (chars * fontSize * 0.6) / 2;
  return Math.max(half, Math.min(width - half, x));
}
