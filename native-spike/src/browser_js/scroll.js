const input = JSON.parse(args);
const dy = input.dy;
if (Date.now() - (window.__sirusHumanAt || 0) < 1500) return JSON.stringify({ interrupted: true });
window.scrollBy({ top: dy ?? 0, left: 0, behavior: "auto" });
return JSON.stringify({ ok: true, y: window.scrollY });
