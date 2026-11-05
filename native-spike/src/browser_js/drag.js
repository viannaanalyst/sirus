const input = JSON.parse(args);
const find = (spec) => spec.ref ? document.querySelector("[data-sirus-ref='" + spec.ref + "']") : (spec.selector ? document.querySelector(spec.selector) : null);
const humanBusy = () => Date.now() - (window.__sirusHumanAt || 0) < 1500;

if (humanBusy()) return JSON.stringify({ interrupted: true });
const isPoint = (spec) => typeof spec.x === "number";
const fromEl = isPoint(input.from) ? null : find(input.from);
const toEl = isPoint(input.to) ? null : find(input.to);
if (!isPoint(input.from) && !fromEl) return JSON.stringify({ ok: false, error: "drag source not found" });
if (!isPoint(input.to) && !toEl) return JSON.stringify({ ok: false, error: "drop target not found" });
if (fromEl) fromEl.scrollIntoView({ block: "center", inline: "center" });
const center = (el) => { const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };
const a = fromEl ? center(fromEl) : { x: input.from.x, y: input.from.y };
const b = toEl ? center(toEl) : { x: input.to.x, y: input.to.y };
const at = (p) => document.elementFromPoint(p.x, p.y) || document.body;
const source = fromEl || at(a);
const mouse = (type, p, el, buttons) => {
  const init = { bubbles: true, cancelable: true, composed: true, clientX: p.x, clientY: p.y, button: 0, buttons, view: window };
  el.dispatchEvent(new PointerEvent(type.replace("mouse", "pointer"), { ...init, pointerId: 1, pointerType: "mouse", isPrimary: true }));
  el.dispatchEvent(new MouseEvent(type, init));
};
const draggable = source.closest ? source.closest("[draggable='true']") : null;
const data = draggable ? new DataTransfer() : null;
const drag = (type, p, el) => el.dispatchEvent(new DragEvent(type, { bubbles: true, cancelable: true, composed: true, clientX: p.x, clientY: p.y, dataTransfer: data }));
mouse("mousedown", a, source, 1);
if (draggable) drag("dragstart", a, draggable);
let last = source;
const steps = 10;
for (let i = 1; i <= steps; i++) {
  const p = { x: a.x + (b.x - a.x) * i / steps, y: a.y + (b.y - a.y) * i / steps };
  const over = i === steps && toEl ? toEl : at(p);
  if (draggable) { if (over !== last) drag("dragenter", p, over); drag("dragover", p, over); }
  else mouse("mousemove", p, over, 1);
  last = over;
  await new Promise((resolve) => setTimeout(resolve, 16));
}
const target = toEl || at(b);
if (draggable) { drag("drop", b, target); drag("dragend", b, draggable); }
mouse("mouseup", b, target, 0);
return JSON.stringify({ ok: true, from: { x: Math.round(a.x), y: Math.round(a.y) }, to: { x: Math.round(b.x), y: Math.round(b.y) }, html5: Boolean(draggable) });
