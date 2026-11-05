const input = JSON.parse(args);
const key = input.key;
if (Date.now() - (window.__sirusHumanAt || 0) < 1500) return JSON.stringify({ interrupted: true });
const target = document.activeElement && document.activeElement !== document.documentElement ? document.activeElement : document.body;
const init = { key, code: key === " " ? "Space" : key, bubbles: true, cancelable: true };
const proceed = target.dispatchEvent(new KeyboardEvent("keydown", init));
if (proceed) {
  const field = target instanceof HTMLInputElement || target instanceof HTMLTextAreaElement;
  if (key === "Enter") {
    if (target instanceof HTMLTextAreaElement) {
      target.setRangeText("\n", target.selectionStart ?? target.value.length, target.selectionEnd ?? target.value.length, "end");
      target.dispatchEvent(new Event("input", { bubbles: true }));
    } else {
      const form = target.form || (target.closest ? target.closest("form") : null);
      if (form) { if (form.requestSubmit) form.requestSubmit(); else form.submit(); }
      else if (target instanceof HTMLElement && !field) target.click();
    }
  } else if (key === "Backspace" && field && target.value) {
    const end = target.selectionEnd ?? target.value.length;
    const start = target.selectionStart === end ? Math.max(0, end - 1) : (target.selectionStart ?? end - 1);
    target.setRangeText("", start, end, "end");
    target.dispatchEvent(new Event("input", { bubbles: true }));
  } else if (key === "Tab") {
    const items = [...document.querySelectorAll("a[href],button,input,select,textarea,[tabindex]:not([tabindex='-1']),[contenteditable='true']")].filter((item) => !item.disabled && item.offsetParent !== null);
    const next = items[(items.indexOf(target) + 1) % Math.max(1, items.length)];
    if (next) next.focus();
  } else if (!field) {
    const page = window.innerHeight * 0.85;
    const moves = { ArrowDown: 80, ArrowUp: -80, PageDown: page, PageUp: -page, " ": page, Home: -1e9, End: 1e9 };
    if (key in moves) window.scrollBy({ top: moves[key], behavior: "auto" });
  }
}
target.dispatchEvent(new KeyboardEvent("keyup", init));
return JSON.stringify({ ok: true });
