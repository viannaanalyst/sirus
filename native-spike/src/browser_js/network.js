(() => {
  if (window.__sirusNetwork) return;
  const log = window.__sirusNetwork = [];
  const push = (entry) => { log.push(entry); if (log.length > 300) log.shift(); };
  const now = () => performance.now();
  const absolute = (value) => { try { return new URL(String(value), location.href).href.slice(0, 2048); } catch { return String(value).slice(0, 2048); } };
  const originalFetch = window.fetch;
  if (typeof originalFetch === "function") {
    window.fetch = function (resource, init) {
      const started = now();
      const request = typeof Request !== "undefined" && resource instanceof Request ? resource : null;
      const entry = { kind: "fetch", method: String((init && init.method) || (request && request.method) || "GET").toUpperCase(), url: absolute(request ? request.url : resource), status: 0, at: Date.now() };
      return originalFetch.apply(this, arguments).then((response) => {
        entry.status = response.status; entry.durationMs = now() - started; push(entry); return response;
      }, (error) => {
        entry.error = String(error).slice(0, 200); entry.durationMs = now() - started; push(entry); throw error;
      });
    };
  }
  const open = XMLHttpRequest.prototype.open;
  const send = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.open = function (method, url) {
    this.__sirusEntry = { kind: "xhr", method: String(method || "GET").toUpperCase(), url: absolute(url), status: 0 };
    return open.apply(this, arguments);
  };
  XMLHttpRequest.prototype.send = function () {
    const entry = this.__sirusEntry;
    if (entry) {
      const started = now();
      entry.at = Date.now();
      this.addEventListener("loadend", () => { entry.status = this.status; entry.durationMs = now() - started; if (!this.status) entry.error = "failed"; push(entry); }, { once: true });
    }
    return send.apply(this, arguments);
  };
  try {
    new PerformanceObserver((list) => {
      for (const item of list.getEntries()) {
        if (item.initiatorType === "fetch" || item.initiatorType === "xmlhttprequest") continue;
        push({ kind: item.initiatorType || "resource", method: "GET", url: String(item.name).slice(0, 2048), status: item.responseStatus || null, at: Math.round(performance.timeOrigin + item.startTime), durationMs: item.duration, size: item.transferSize || undefined });
      }
    }).observe({ type: "resource", buffered: true });
  } catch {}
})()
