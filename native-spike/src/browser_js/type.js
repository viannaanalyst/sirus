const input = JSON.parse(args);
const ref = input.ref;
const selector = input.selector;
const text = input.text;
const submit = input.submit;
if (Date.now() - (window.__sirusHumanAt || 0) < 1500) return JSON.stringify({ interrupted: true });
const target = ref ? document.querySelector("[data-sirus-ref='" + ref + "']") : (selector ? document.querySelector(selector) : null);
if (!target) return JSON.stringify({ ok: false, error: "element not found" });
target.focus();
const next = text ?? "";
if (target.isContentEditable) {
  target.textContent = next;
} else if (target instanceof HTMLSelectElement) {
  target.value = next;
} else {
  const proto = target instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")?.set;
  if (setter) setter.call(target, next); else target.value = next;
}
target.dispatchEvent(new Event("input", { bubbles: true }));
target.dispatchEvent(new Event("change", { bubbles: true }));
if (submit) {
  target.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
  if (target.form && typeof target.form.requestSubmit === "function") target.form.requestSubmit();
}
return JSON.stringify({ ok: true });
