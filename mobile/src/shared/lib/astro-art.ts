import type { AstroIconId, AstroStyle } from "../client/types";

/**
 * Canvas art for Astro project icons (ADR-059, ADR-116): sixteen animated cosmic icons drawn
 * by one function each in a 64×64 space, rendered as Metal (silver, like the Sirus mark) or
 * Neon (the project colour with a glow).
 */

type Ctx = CanvasRenderingContext2D;

interface Palette { body: string; light: string; dark: string; line: string; accent: string; neon: boolean }

const TAU = Math.PI * 2;

const hexRgb = (hex: string) => { const n = parseInt(hex.slice(1), 16); return [n >> 16, (n >> 8) & 255, n & 255]; };

const rgba = (hex: string, a: number) => { const [r, g, b] = hexRgb(hex); return `rgba(${r},${g},${b},${a})`; };

const mix = (a: string, b: string, k: number) => { const A = hexRgb(a), B = hexRgb(b); return "#" + A.map((v, i) => Math.round(v + (B[i] - v) * k).toString(16).padStart(2, "0")).join(""); };

function palette(style: AstroStyle, color: string): Palette {
  if (style === "neon") return { body: mix(color, "#000000", .55), light: mix(color, "#ffffff", .55), dark: "#06060a", line: color, accent: color, neon: true };
  return { body: "#b9bcc4", light: "#f4f5f8", dark: "#5a5d66", line: "#e4e6ec", accent: color, neon: false };
}

function sphere(c: Ctx, x: number, y: number, r: number, P: Palette, tint?: string) {
  const g = c.createRadialGradient(x - r * .35, y - r * .4, r * .1, x, y, r);
  g.addColorStop(0, P.light); g.addColorStop(.55, tint ?? P.body); g.addColorStop(1, P.dark);
  c.fillStyle = g; c.beginPath(); c.arc(x, y, r, 0, TAU); c.fill();
  if (P.neon) { c.strokeStyle = P.line; c.lineWidth = 1.6; c.stroke(); }
}

function star4(c: Ctx, x: number, y: number, r: number, rot = 0) {
  c.beginPath();
  for (let i = 0; i < 8; i++) { const a = rot + i * Math.PI / 4, rr = i % 2 ? r * .28 : r; c.lineTo(x + Math.cos(a) * rr, y + Math.sin(a) * rr); }
  c.closePath(); c.fill();
}

const ink = (P: Palette) => (P.neon ? P.line : "#ffffff");

const DRAW: Record<AstroIconId, (c: Ctx, t: number, P: Palette) => void> = {
  orbita(c, t, P) {
    c.lineWidth = 3.2; c.strokeStyle = P.line;
    for (const k of [0, 1]) { c.save(); c.translate(32, 32); c.rotate((k ? -1 : 1) * .55 + Math.sin(t / 2600) * .08); c.beginPath(); c.ellipse(0, 0, 25, 11, 0, 0, TAU); c.stroke(); c.restore(); }
    sphere(c, 32, 32, 7.5, P, P.accent);
    for (const k of [0, 1]) { const a = t / 1100 + k * Math.PI; c.save(); c.translate(32, 32); c.rotate((k ? -1 : 1) * .55); sphere(c, Math.cos(a) * 25, Math.sin(a) * 11, 3.6, P); c.restore(); }
  },
  saturno(c, t, P) {
    const tilt = -.35 + Math.sin(t / 2400) * .06;
    c.save(); c.translate(32, 32); c.rotate(tilt); c.strokeStyle = P.line; c.lineWidth = 3; c.beginPath(); c.ellipse(0, 0, 28, 8, 0, Math.PI, TAU); c.stroke(); c.restore();
    sphere(c, 32, 32, 15, P, P.accent);
    c.save(); c.beginPath(); c.arc(32, 32, 15, 0, TAU); c.clip();
    c.fillStyle = rgba(ink(P), .18); for (let i = -2; i <= 2; i++) c.fillRect(10, 30 + i * 6 + Math.sin(t / 900 + i) * 1.2, 44, 2);
    c.restore();
    c.save(); c.translate(32, 32); c.rotate(tilt); c.strokeStyle = P.light; c.lineWidth = 3; c.beginPath(); c.ellipse(0, 0, 28, 8, 0, 0, Math.PI); c.stroke(); c.restore();
  },
  lua(c, t, P) {
    const phase = Math.sin(t / 2200) * 3;
    sphere(c, 32, 32, 22, P);
    c.fillStyle = rgba(P.neon ? P.line : "#000000", P.neon ? .25 : .12);
    for (const [x, y, r] of [[24, 26, 4], [38, 38, 5.5], [36, 22, 2.6], [22, 40, 3]]) { c.beginPath(); c.arc(x, y, r, 0, TAU); c.fill(); }
    // The phase is cut out of the moon, so nothing is painted outside it.
    c.save(); c.globalCompositeOperation = "destination-out"; c.shadowBlur = 0; c.fillStyle = "#000";
    c.beginPath(); c.arc(44 + phase, 26, 20, 0, TAU); c.fill(); c.restore();
    if (P.neon) { c.strokeStyle = rgba(P.line, .5); c.lineWidth = 1.2; c.beginPath(); c.arc(32, 32, 22, 2.1, 4.9); c.stroke(); }
  },
  sol(c, t, P) {
    c.save(); c.translate(32, 32); c.rotate(t / 4000); c.fillStyle = P.neon ? P.line : P.accent;
    for (let i = 0; i < 12; i++) { c.rotate(TAU / 12); const L = i % 2 ? 26 : 22 + Math.sin(t / 400 + i) * 2; c.beginPath(); c.moveTo(13, -2.2); c.lineTo(L, 0); c.lineTo(13, 2.2); c.fill(); }
    c.restore();
    sphere(c, 32, 32, 12, P, mix(P.accent, "#ffffff", .25));
  },
  galaxia(c, t, P) {
    c.save(); c.translate(32, 32); c.rotate(t / 3500); c.scale(1, .78);
    for (let arm = 0; arm < 2; arm++) for (let i = 0; i < 46; i++) {
      const th = i / 46 * 3.4, r = 3 + th * 7.4, a = th + arm * Math.PI;
      c.fillStyle = arm ? rgba(P.neon ? P.line : P.light, 1 - i / 52) : rgba(P.accent, 1 - i / 52);
      c.beginPath(); c.arc(Math.cos(a) * r, Math.sin(a) * r, 2.6 - i / 46 * 1.6, 0, TAU); c.fill();
    }
    c.restore();
    sphere(c, 32, 32, 5, P, P.light);
  },
  nebulosa(c, t, P) {
    for (const [x, y, r, col, ph] of [[26, 28, 18, P.accent, 0], [40, 36, 15, P.neon ? P.line : "#b48cff", 2], [30, 42, 12, "#5fb3f9", 4]] as const) {
      const g = c.createRadialGradient(x, y, 0, x, y, r + Math.sin(t / 1400 + ph) * 2);
      g.addColorStop(0, rgba(col, .85)); g.addColorStop(1, rgba(col, 0));
      c.fillStyle = g; c.beginPath(); c.arc(x, y, r + 3, 0, TAU); c.fill();
    }
    c.fillStyle = "#fff";
    for (const [x, y, p] of [[18, 18, 0], [46, 20, 1.5], [50, 46, 3], [16, 46, 4.5], [33, 30, 2.2]]) star4(c, x, y, 1 + Math.max(0, Math.sin(t / 500 + p)) * 2.4);
  },
  cometa(c, t, P) {
    const k = (t / 2600) % 1, x = 14 + k * 6, y = 50 - k * 6;
    const g = c.createLinearGradient(x + 6, y - 6, 58, 6);
    g.addColorStop(0, rgba(P.neon ? P.line : P.accent, .9)); g.addColorStop(1, rgba(P.accent, 0));
    c.fillStyle = g; c.beginPath(); c.moveTo(x + 2, y - 9); c.lineTo(60, 4); c.lineTo(x + 9, y - 2); c.closePath(); c.fill();
    c.strokeStyle = rgba(ink(P), .5); c.lineWidth = 1.2; c.beginPath(); c.moveTo(x + 4, y - 4); c.lineTo(56, 10); c.stroke();
    sphere(c, x + 4, y - 4, 8, P, P.light);
  },
  buraco(c, t, P) {
    const g = c.createLinearGradient(-30, 0, 30, 0); g.addColorStop(0, rgba(P.accent, .1)); g.addColorStop(.5, P.neon ? P.line : P.light); g.addColorStop(1, rgba(P.accent, .1));
    c.save(); c.translate(32, 32); c.rotate(-.35); c.strokeStyle = g; c.lineWidth = 4; c.beginPath(); c.ellipse(0, 0, 29, 7, 0, Math.PI, TAU); c.stroke(); c.restore();
    c.fillStyle = "#000"; c.beginPath(); c.arc(32, 32, 12, 0, TAU); c.fill();
    c.strokeStyle = P.neon ? P.line : P.light; c.lineWidth = 2.4; c.shadowColor = P.accent; c.shadowBlur = 10; c.beginPath(); c.arc(32, 32, 13, 0, TAU); c.stroke(); c.shadowBlur = 0;
    c.save(); c.translate(32, 32); c.rotate(-.35); c.strokeStyle = g; c.lineWidth = 4; c.beginPath(); c.ellipse(0, 0, 29, 7, 0, 0, Math.PI); c.stroke();
    const a = t / 700; c.fillStyle = "#fff"; c.beginPath(); c.arc(Math.cos(a) * 29, Math.sin(a) * 7, 1.8, 0, TAU); c.fill(); c.restore();
  },
  estrela(c, t, P) {
    const s = 1 + Math.sin(t / 700) * .08;
    const g = c.createRadialGradient(32, 32, 0, 32, 32, 26); g.addColorStop(0, rgba(P.accent, .55)); g.addColorStop(1, rgba(P.accent, 0));
    c.fillStyle = g; c.beginPath(); c.arc(32, 32, 26, 0, TAU); c.fill();
    c.fillStyle = P.neon ? P.line : P.light; star4(c, 32, 32, 26 * s, t / 5000);
    c.fillStyle = P.accent; star4(c, 32, 32, 12 * s, t / 5000 + Math.PI / 4);
  },
  pulsar(c, t, P) {
    c.save(); c.translate(32, 32); c.rotate(t / 900);
    for (const d of [1, -1]) { const g = c.createLinearGradient(0, 0, 0, d * 30); g.addColorStop(0, rgba(P.neon ? P.line : P.accent, .9)); g.addColorStop(1, rgba(P.accent, 0)); c.fillStyle = g; c.beginPath(); c.moveTo(-3, 0); c.lineTo(-8, d * 30); c.lineTo(8, d * 30); c.lineTo(3, 0); c.fill(); }
    c.restore();
    sphere(c, 32, 32, 9, P, P.light);
    c.strokeStyle = rgba(ink(P), .3 + .3 * Math.sin(t / 300)); c.lineWidth = 1.2; c.beginPath(); c.arc(32, 32, 14, 0, TAU); c.stroke();
  },
  constelacao(c, t, P) {
    const pts = [[12, 44], [22, 26], [34, 34], [44, 16], [54, 30], [46, 50]];
    c.strokeStyle = rgba(ink(P), .45); c.lineWidth = 1.4; c.beginPath(); pts.forEach(([x, y], i) => (i ? c.lineTo(x, y) : c.moveTo(x, y))); c.lineTo(34, 34); c.stroke();
    pts.forEach(([x, y], i) => { c.fillStyle = i % 2 ? P.accent : P.neon ? P.line : P.light; star4(c, x, y, (2.6 + Math.max(0, Math.sin(t / 600 + i * 1.3)) * 2.6) * 1.6); });
  },
  satelite(c, t, P) {
    sphere(c, 22, 42, 13, P, P.accent);
    const a = t / 1400, x = 32 + Math.cos(a) * 20, y = 30 + Math.sin(a) * 9;
    c.strokeStyle = rgba(ink(P), .25); c.lineWidth = 1; c.beginPath(); c.ellipse(32, 30, 20, 9, 0, 0, TAU); c.stroke();
    c.save(); c.translate(x, y); c.rotate(-.5);
    c.fillStyle = P.neon ? P.line : "#7d9bd6"; c.fillRect(-13, -3, 8, 6); c.fillRect(5, -3, 8, 6);
    c.fillStyle = P.light; c.fillRect(-5, -4.5, 10, 9); c.restore();
  },
  foguete(c, t, P) {
    c.save(); c.translate(32, 32 + Math.sin(t / 500) * 1.5); c.rotate(.6);
    const f = 8 + Math.sin(t / 90) * 3;
    const g = c.createLinearGradient(0, 14, 0, 22 + f); g.addColorStop(0, "#ffd36b"); g.addColorStop(1, rgba(P.accent, 0));
    c.fillStyle = g; c.beginPath(); c.moveTo(-5, 14); c.lineTo(0, 22 + f); c.lineTo(5, 14); c.fill();
    c.fillStyle = P.neon ? P.body : P.light; c.beginPath(); c.moveTo(0, -24); c.quadraticCurveTo(12, -8, 8, 14); c.lineTo(-8, 14); c.quadraticCurveTo(-12, -8, 0, -24); c.fill();
    if (P.neon) { c.strokeStyle = P.line; c.lineWidth = 1.6; c.stroke(); }
    c.fillStyle = P.accent; c.beginPath(); c.moveTo(-8, 6); c.lineTo(-14, 16); c.lineTo(-8, 14); c.fill(); c.beginPath(); c.moveTo(8, 6); c.lineTo(14, 16); c.lineTo(8, 14); c.fill();
    c.fillStyle = P.dark; c.beginPath(); c.arc(0, -6, 4.2, 0, TAU); c.fill(); c.fillStyle = rgba(P.accent, .9); c.beginPath(); c.arc(0, -6, 2.6, 0, TAU); c.fill();
    c.restore();
  },
  planeta(c, t, P) {
    sphere(c, 32, 32, 22, P, P.accent);
    c.save(); c.beginPath(); c.arc(32, 32, 22, 0, TAU); c.clip();
    const off = (t / 60) % 44; c.fillStyle = rgba(ink(P), .28);
    for (const dx of [-44, 0, 44]) for (const [x, y, w, h] of [[14, 20, 14, 8], [30, 34, 18, 9], [8, 40, 10, 6], [40, 18, 8, 5]]) { c.beginPath(); c.ellipse(x + dx + off, y, w / 2, h / 2, 0, 0, TAU); c.fill(); }
    c.restore();
    c.strokeStyle = rgba(P.accent, .6); c.lineWidth = 1.5; c.beginPath(); c.arc(32, 32, 26, -2.4, -.8); c.stroke();
  },
  asteroide(c, t, P) {
    c.save(); c.translate(32, 32); c.rotate(t / 2400);
    const R = [20, 17, 22, 15, 19, 21, 16, 18];
    c.beginPath(); R.forEach((r, i) => { const a = i / R.length * TAU; c.lineTo(Math.cos(a) * r, Math.sin(a) * r); }); c.closePath();
    const g = c.createRadialGradient(-6, -6, 2, 0, 0, 22); g.addColorStop(0, P.light); g.addColorStop(.6, P.body); g.addColorStop(1, P.dark);
    c.fillStyle = g; c.fill(); if (P.neon) { c.strokeStyle = P.line; c.lineWidth = 1.6; c.stroke(); }
    c.fillStyle = rgba(P.neon ? P.line : "#000000", .2);
    for (const [x, y, r] of [[-6, -4, 4], [7, 5, 3], [2, -10, 2.4]]) { c.beginPath(); c.arc(x, y, r, 0, TAU); c.fill(); }
    c.restore();
    c.fillStyle = P.accent; star4(c, 52, 14, 3 + Math.max(0, Math.sin(t / 400)) * 2);
  },
  eclipse(c, t, P) {
    const g = c.createRadialGradient(32, 32, 14, 32, 32, 30); g.addColorStop(0, rgba(ink(P), .9)); g.addColorStop(.35, rgba(P.accent, .55)); g.addColorStop(1, rgba(P.accent, 0));
    c.fillStyle = g; c.beginPath(); c.arc(32, 32, 30, 0, TAU); c.fill();
    c.save(); c.translate(32, 32); c.rotate(t / 3000); c.strokeStyle = rgba(ink(P), .5); c.lineWidth = 1;
    for (let i = 0; i < 16; i++) { c.rotate(TAU / 16); c.beginPath(); c.moveTo(17, 0); c.lineTo(20 + (i % 3) * 3 + Math.sin(t / 300 + i) * 1.5, 0); c.stroke(); }
    c.restore();
    c.fillStyle = "#050507"; c.beginPath(); c.arc(32, 32, 15.5, 0, TAU); c.fill();
    c.fillStyle = "#fff"; c.beginPath(); c.arc(43, 21, 2, 0, TAU); c.fill();
  },
};

/**
 * Draws one icon in the 64×64 space of whatever context it gets: the desktop's canvas, or the
 * phone's recorder that turns the same drawing into SVG.
 */
export function drawAstroIcon(c: Ctx, icon: AstroIconId, style: AstroStyle, color: string, t: number) {
  const draw = DRAW[icon];
  if (!draw) return;
  const P = palette(style, color);
  if (P.neon) { c.shadowColor = color; c.shadowBlur = 14; }
  draw(c, t, P);
  c.shadowBlur = 0;
}
