const input = JSON.parse(args);
const find = (spec) => spec.ref ? document.querySelector("[data-sirus-ref='" + spec.ref + "']") : (spec.selector ? document.querySelector(spec.selector) : null);
const humanBusy = () => Date.now() - (window.__sirusHumanAt || 0) < 1500;

if (humanBusy()) return JSON.stringify({ interrupted: true });
const target = find(input);
if (!target) return JSON.stringify({ ok: false, error: "element not found" });
const wanted = input.values.map(String);
const text = (node) => (node.textContent || "").trim().replace(/\s+/g, " ");
if (target instanceof HTMLSelectElement) {
  const options = Array.from(target.options);
  const chosen = options.filter((option) => wanted.some((value) => value === option.value || value === (option.label || "").trim() || value === text(option)));
  const known = options.slice(0, 50).map((option) => ({ value: option.value, label: (option.label || text(option)).slice(0, 80) }));
  if (!chosen.length) return JSON.stringify({ ok: false, error: "no option matches; options: " + JSON.stringify(known) });
  if (!target.multiple && wanted.length > 1) return JSON.stringify({ ok: false, error: "this select takes one value" });
  if (target.disabled) return JSON.stringify({ ok: false, error: "the select is disabled" });
  target.focus();
  if (target.multiple) { for (const option of options) option.selected = chosen.includes(option); }
  else target.value = chosen[0].value;
  target.dispatchEvent(new Event("input", { bubbles: true }));
  target.dispatchEvent(new Event("change", { bubbles: true }));
  return JSON.stringify({ ok: true, selected: chosen.map((option) => option.value) });
}
const owned = (node) => { const id = node.getAttribute("aria-controls") || node.getAttribute("aria-owns"); return id ? document.getElementById(id) : null; };
let listbox = target.getAttribute("role") === "listbox" ? target : owned(target);
if (!listbox && (target.getAttribute("role") === "combobox" || target.getAttribute("aria-haspopup"))) {
  target.click();
  await new Promise((resolve) => setTimeout(resolve, 200));
  listbox = owned(target) || document.querySelector("[role='listbox']");
}
if (!listbox) return JSON.stringify({ ok: false, error: "the element is not a <select>, a listbox or a combobox with one" });
const options = Array.from(listbox.querySelectorAll("[role='option']"));
const chosen = options.filter((option) => wanted.some((value) => value === text(option) || value === (option.getAttribute("data-value") || "") || value === (option.getAttribute("aria-label") || "")));
if (!chosen.length) return JSON.stringify({ ok: false, error: "no option matches; options: " + JSON.stringify(options.slice(0, 50).map(text)) });
for (const option of chosen) {
  option.scrollIntoView({ block: "nearest" });
  const rect = option.getBoundingClientRect();
  const init = { bubbles: true, cancelable: true, composed: true, clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2, view: window };
  option.dispatchEvent(new PointerEvent("pointerdown", { ...init, pointerId: 1, pointerType: "mouse", isPrimary: true }));
  option.dispatchEvent(new MouseEvent("mousedown", init));
  option.dispatchEvent(new PointerEvent("pointerup", { ...init, pointerId: 1, pointerType: "mouse", isPrimary: true }));
  option.dispatchEvent(new MouseEvent("mouseup", init));
  option.click();
}
return JSON.stringify({ ok: true, selected: chosen.map((option) => text(option).slice(0, 80)) });
