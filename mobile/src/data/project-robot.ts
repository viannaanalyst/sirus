import type { ProjectAppearance } from "@sirus/client/types";

/**
 * The Mac's project robot (`native-spike/src/project_avatar.rs`): one of eight bodies in the
 * project's colour with a dark visor and two eyes. The default look comes from a hash of the
 * project's name; a saved `appearance` wins. Pure, so the phone draws the same robot.
 */
export type RobotShape = "round" | "square" | "cloud" | "ghost" | "hex" | "capsule" | "tv" | "bean";
export type RobotEyes = "bars" | "dots" | "sleepy" | "happy";

export const ROBOT_SHAPES: readonly RobotShape[] = ["round", "square", "cloud", "ghost", "hex", "capsule", "tv", "bean"];
const EYES: readonly RobotEyes[] = ["bars", "dots", "sleepy", "happy"];
const SWATCHES = [0x8b7cf6, 0x5b8def, 0x38bdf8, 0x2dd4bf, 0x4ade80, 0xa3c94a, 0xfacc15, 0xfb923c, 0xf87171, 0xf472b6, 0xc084fc, 0x94a3b8];

/** SVG markup of each body, in a 64-unit square. */
export const ROBOT_PATHS: Record<RobotShape, string> = {
  square: "M23 7h18a16 16 0 0 1 16 16v18a16 16 0 0 1-16 16H23A16 16 0 0 1 7 41V23A16 16 0 0 1 23 7z",
  cloud: "M20 50a13 13 0 0 1-2.6-25.7A16 16 0 0 1 47 21.5 14.5 14.5 0 0 1 45 50z",
  ghost: "M10 30a22 22 0 0 1 44 0v22c0 3-3 4-5 2l-4-3-4 3a4 4 0 0 1-5 0l-4-3-4 3a4 4 0 0 1-5 0l-4-3-4 3c-2 2-5 1-5-2z",
  hex: "M28 6.5a8 8 0 0 1 8 0l16 9.3a8 8 0 0 1 4 7V41a8 8 0 0 1-4 7l-16 9.3a8 8 0 0 1-8 0L12 48a8 8 0 0 1-4-7V22.8a8 8 0 0 1 4-7z",
  capsule: "M32 5a19 19 0 0 1 19 19v16a19 19 0 0 1-38 0V24A19 19 0 0 1 32 5z",
  tv: "M19 15h26a13 13 0 0 1 13 13v16a13 13 0 0 1-13 13H19A13 13 0 0 1 6 44V28a13 13 0 0 1 13-13z",
  bean: "M32 6c15 0 26 10 26 25 0 8-3 12-6 15-4 4-10 12-22 12C16 58 6 48 6 33 6 17 17 6 32 6z",
  round: "M32 6a26 26 0 1 1 0 52 26 26 0 0 1 0-52z",
};

export interface RobotLook { readonly shape: RobotShape; readonly color: string; readonly eyes: RobotEyes }

/** FNV-1a over the first UTF-16 unit of each code point, as the Mac's `hash_name`. */
export function hashName(name: string): number {
  let hash = 2166136261;
  for (const ch of name) {
    hash ^= ch.charCodeAt(0);
    hash = Math.imul(hash, 16777619) >>> 0;
  }
  return hash >>> 0;
}

const HEX = /^#[0-9a-f]{6}$/i;

/** The saved choices over the name's defaults; unknown values fall back to them. */
export function robotLook(name: string, appearance?: Partial<ProjectAppearance> | null): RobotLook {
  const hash = hashName(name);
  const color = appearance?.color && HEX.test(appearance.color) ? appearance.color.toLowerCase() : `#${SWATCHES[hash % SWATCHES.length]!.toString(16).padStart(6, "0")}`;
  const saved = appearance?.eyes as string | undefined;
  const eyes = EYES.find((item) => item === saved) ?? (saved === "dot" || saved === "round" ? "dots" : "bars");
  const accessory = appearance?.accessory as string | undefined;
  const shape = ROBOT_SHAPES.find((item) => item === accessory) ?? ROBOT_SHAPES[(hash >>> 5) % ROBOT_SHAPES.length]!;
  return { shape, color, eyes };
}

// ---- colour: the body gradient is the colour lifted and sunk in OKLCH, as the Mac does.

function hexToOklch(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  const linear = (v: number) => { const x = v / 255; return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4; };
  const r = linear((n >> 16) & 255), g = linear((n >> 8) & 255), b = linear(n & 255);
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const labL = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const labA = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const labB = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return [labL, Math.hypot(labA, labB), ((Math.atan2(labB, labA) * 180) / Math.PI + 360) % 360];
}

function oklchToHex(l: number, c: number, h: number): string {
  const a = c * Math.cos((h * Math.PI) / 180), b = c * Math.sin((h * Math.PI) / 180);
  const l_ = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m_ = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s_ = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const lin = [4.0767416621 * l_ - 3.3077115913 * m_ + 0.2309699292 * s_, -1.2684380046 * l_ + 2.6097574011 * m_ - 0.3413193965 * s_, -0.0041960863 * l_ - 0.7034186147 * m_ + 1.707614701 * s_];
  const byte = (v: number) => {
    const x = Math.min(1, Math.max(0, v));
    return Math.round((x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055) * 255).toString(16).padStart(2, "0");
  };
  return `#${byte(lin[0]!)}${byte(lin[1]!)}${byte(lin[2]!)}`;
}

/** The body's gradient: the colour lighter at the top, darker at the bottom. */
export function robotGradient(color: string): { readonly top: string; readonly bottom: string } {
  const [l, c, h] = hexToOklch(color);
  return { top: oklchToHex(Math.min(0.95, l + 0.09), c, h), bottom: oklchToHex(Math.max(0.3, l - 0.11), c, h) };
}
