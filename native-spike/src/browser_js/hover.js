const input = JSON.parse(args);
const find = (spec) => spec.ref ? document.querySelector("[data-sirus-ref='" + spec.ref + "']") : (spec.selector ? document.querySelector(spec.selector) : null);
const humanBusy = () => Date.now() - (window.__sirusHumanAt || 0) < 1500;

if (humanBusy()) return JSON.stringify({ interrupted: true });
const target = find(input);
if (!target) return JSON.stringify({ ok: false, error: "element not found" });
target.scrollIntoView({ block: "center", inline: "center" });
const rect = target.getBoundingClientRect();
const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2;
const init = { bubbles: true, cancelable: true, composed: true, clientX: x, clientY: y, view: window };
const pointer = { ...init, pointerId: 1, pointerType: "mouse", isPrimary: true };
target.dispatchEvent(new PointerEvent("pointerover", pointer));
target.dispatchEvent(new PointerEvent("pointerenter", { ...pointer, bubbles: false }));
target.dispatchEvent(new MouseEvent("mouseover", init));
target.dispatchEvent(new MouseEvent("mouseenter", { ...init, bubbles: false }));
target.dispatchEvent(new PointerEvent("pointermove", pointer));
target.dispatchEvent(new MouseEvent("mousemove", init));
return JSON.stringify({ ok: true, x: Math.round(x), y: Math.round(y) });
