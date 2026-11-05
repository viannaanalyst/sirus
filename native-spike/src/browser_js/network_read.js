const input = JSON.parse(args);
const log = window.__sirusNetwork || [];
const entries = log.slice(-Math.max(1, Math.min(200, input.limit || 50)));
if (input.clear) log.length = 0;
return JSON.stringify({ ok: true, url: location.href, entries });
