const input = JSON.parse(args);
const ref = input.ref;
const selector = input.selector;
if (Date.now() - (window.__sirusHumanAt || 0) < 1500) return JSON.stringify({ interrupted: true });
const target = ref ? document.querySelector("[data-sirus-ref='" + ref + "']") : (selector ? document.querySelector(selector) : null);
if (!target) return JSON.stringify({ ok: false, error: "element not found" });
target.scrollIntoView({ block: "center" });
target.click();
return JSON.stringify({ ok: true });
