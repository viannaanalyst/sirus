const input = JSON.parse(args);
const deadline = Date.now() + Math.min(8000, Math.max(100, input.timeoutMs || 5000));
const found = () => input.selector ? Boolean(document.querySelector(input.selector)) : (document.body ? document.body.innerText : "").includes(input.text);
try { found(); } catch { return JSON.stringify({ ok: false, error: "invalid selector" }); }
while (Date.now() < deadline) {
  if (found()) return JSON.stringify({ ok: true });
  await new Promise((resolve) => setTimeout(resolve, 150));
}
return JSON.stringify({ ok: false, error: "timed out waiting" });
