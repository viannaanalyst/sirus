/**
 * A project's little head on the phone, drawn as the Mac draws it (`project_avatar.rs`): the same
 * name gives the same default, a saved look wins over it, and the colours and paths match.
 */

const EYES = ["bars", "dots", "sleepy", "happy"] as const;
const SHAPES = ["round", "square", "cloud", "ghost", "hex", "capsule", "tv", "bean"] as const;
const SWATCHES = ["#8b7cf6", "#5b8def", "#38bdf8", "#2dd4bf", "#4ade80", "#a3c94a", "#facc15", "#fb923c", "#f87171", "#f472b6", "#c084fc", "#94a3b8"];

export type AvatarEyes = (typeof EYES)[number];
type AvatarShape = (typeof SHAPES)[number];

export interface AvatarLook { readonly color: string; readonly eyes: AvatarEyes; readonly shape: AvatarShape }

function hashName(name: string): number {
  let hash = 2166136261;
  for (const character of name) {
    hash ^= character.charCodeAt(0);
    hash = Math.imul(hash, 16777619) >>> 0;
  }
  return hash;
}

/** The saved choices over the name's defaults; unknown values fall back to them. */
export function avatarLook(name: string, appearance?: { readonly color?: string; readonly eyes?: string; readonly accessory?: string }): AvatarLook {
  const hash = hashName(name);
  const color = appearance?.color && /^#[0-9a-f]{6}$/i.test(appearance.color) ? appearance.color : SWATCHES[hash % SWATCHES.length]!;
  const saved = appearance?.eyes;
  const eyes = EYES.find((item) => item === saved) ?? (saved === "dot" || saved === "round" ? "dots" : "bars");
  const shape = SHAPES.find((item) => item === appearance?.accessory) ?? SHAPES[(hash >>> 5) % SHAPES.length]!;
  return { color, eyes, shape };
}

const SHAPE_PATHS: Record<AvatarShape, string> = {
  round: '<circle cx="32" cy="32" r="26"/>',
  square: '<rect x="7" y="7" width="50" height="50" rx="16"/>',
  cloud: '<path d="M20 50a13 13 0 0 1-2.6-25.7A16 16 0 0 1 47 21.5 14.5 14.5 0 0 1 45 50z"/>',
  ghost: '<path d="M10 30a22 22 0 0 1 44 0v22c0 3-3 4-5 2l-4-3-4 3a4 4 0 0 1-5 0l-4-3-4 3a4 4 0 0 1-5 0l-4-3-4 3c-2 2-5 1-5-2z"/>',
  hex: '<path d="M28 6.5a8 8 0 0 1 8 0l16 9.3a8 8 0 0 1 4 7V41a8 8 0 0 1-4 7l-16 9.3a8 8 0 0 1-8 0L12 48a8 8 0 0 1-4-7V22.8a8 8 0 0 1 4-7z"/>',
  capsule: '<rect x="13" y="5" width="38" height="54" rx="19"/>',
  tv: '<rect x="6" y="15" width="52" height="42" rx="13"/>',
  bean: '<path d="M32 6c15 0 26 10 26 25 0 8-3 12-6 15-4 4-10 12-22 12C16 58 6 48 6 33 6 17 17 6 32 6z"/>',
};

function hexToOklch(hex: string): [number, number, number] {
  const value = parseInt(hex.slice(1), 16);
  const linear = (channel: number) => {
    const x = channel / 255;
    return x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4;
  };
  const [r, g, b] = [linear((value >> 16) & 255), linear((value >> 8) & 255), linear(value & 255)];
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const labL = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const labA = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const labB = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  return [labL, Math.hypot(labA, labB), ((Math.atan2(labB, labA) * 180) / Math.PI + 360) % 360];
}

function oklchToHex(l: number, c: number, h: number): string {
  const a = c * Math.cos((h * Math.PI) / 180);
  const b = c * Math.sin((h * Math.PI) / 180);
  const l3 = (l + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m3 = (l - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s3 = (l - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const byte = (channel: number) => {
    const x = Math.min(Math.max(channel, 0), 1);
    return Math.round((x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055) * 255).toString(16).padStart(2, "0");
  };
  return `#${byte(4.0767416621 * l3 - 3.3077115913 * m3 + 0.2309699292 * s3)}${byte(-1.2684380046 * l3 + 2.6097574011 * m3 - 0.3413193965 * s3)}${byte(-0.0041960863 * l3 - 0.7034186147 * m3 + 1.707614701 * s3)}`;
}

function eyesSvg(eyes: AvatarEyes): string {
  return [24.5, 39.5].map((x) => {
    if (eyes === "sleepy") return `<rect x="${x - 4.5}" y="30.4" width="9" height="3.2" rx="1.6" fill="#f4f5f8"/>`;
    if (eyes === "happy") return `<path d="M${x - 5} 34Q${x} 27 ${x + 5} 34" fill="none" stroke="#f4f5f8" stroke-width="3.2" stroke-linecap="round"/>`;
    if (eyes === "dots") return `<circle cx="${x}" cy="32" r="4.2" fill="#f4f5f8"/>`;
    return `<rect x="${x - 3.8}" y="26.25" width="7.6" height="11.5" rx="3.8" fill="#f4f5f8"/>`;
  }).join("");
}

/** The whole head as SVG in a 64-unit square: body, gloss, visor and eyes. */
export function avatarSvg(look: AvatarLook): string {
  const [l, c, h] = hexToOklch(look.color);
  const top = oklchToHex(Math.min(l + 0.09, 0.95), c, h);
  const bottom = oklchToHex(Math.max(l - 0.11, 0.3), c, h);
  const path = SHAPE_PATHS[look.shape];
  const antennas = look.shape === "tv" ? `<path d="M23 15 16 6M41 15l7-9" fill="none" stroke="${bottom}" stroke-width="3.5" stroke-linecap="round"/>` : "";
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64"><defs><linearGradient id="b" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="${top}"/><stop offset="1" stop-color="${bottom}"/></linearGradient><linearGradient id="s" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#ffffff" stop-opacity="0.45"/><stop offset="0.55" stop-color="#ffffff" stop-opacity="0"/></linearGradient></defs>${antennas}<g fill="url(#b)">${path}</g><g fill="url(#s)" opacity="0.55">${path}</g><rect x="13" y="22" width="38" height="20" rx="10" fill="#14151b"/>${eyesSvg(look.eyes)}</svg>`;
}

/** The speech recogniser's volume (-2 to 10, under 0 is silence) as the Mac's 0 to 1 level. */
export function voiceLevel(volume: number): number {
  return Math.min(Math.max(volume, 0) / 10, 1);
}
