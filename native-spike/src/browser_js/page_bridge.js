(() => {
  window.__sirusHumanAt = window.__sirusHumanAt || 0;
  // Trusted events only: the agent's own synthetic keys (a typed Enter) must not read as the person.
  const mark = (event) => { if (event.isTrusted) window.__sirusHumanAt = Date.now(); };
  document.addEventListener("pointerdown", mark, true);
  document.addEventListener("keydown", mark, true);
  document.addEventListener("wheel", mark, true);
  window.__sirusLogs = window.__sirusLogs || [];
  const push = (level, args) => {
    try {
      window.__sirusLogs.push({ level, text: Array.from(args).map(String).join(" ").slice(0, 500), at: Date.now() });
      if (window.__sirusLogs.length > 200) window.__sirusLogs.shift();
    } catch {}
  };
  for (const level of ["log", "warn", "error", "info"]) {
    const original = console[level];
    console[level] = (...args) => { push(level, args); original.apply(console, args); };
  }
  window.addEventListener("error", (event) => push("error", [event.message]));
  window.addEventListener("unhandledrejection", (event) => push("error", [String(event.reason)]));
  // Key presses and clicks for recordings (ADR-112); never what is typed in a password field.
  window.__sirusInput = window.__sirusInput || [];
  const note = (entry) => { window.__sirusInput.push(entry); if (window.__sirusInput.length > 60) window.__sirusInput.shift(); };
  const named = { " ": "Space", ArrowLeft: "←", ArrowRight: "→", ArrowUp: "↑", ArrowDown: "↓", Enter: "↩", Backspace: "⌫", Delete: "⌦", Escape: "Esc", Tab: "⇥" };
  document.addEventListener("keydown", (event) => {
    if (["Meta", "Control", "Alt", "Shift"].includes(event.key)) return;
    const target = event.target;
    if (target && target.type === "password") { note({ kind: "key", key: "•", at: Date.now() }); return; }
    const key = named[event.key] || event.key;
    const mods = (event.metaKey ? "⌘" : "") + (event.ctrlKey ? "⌃" : "") + (event.altKey ? "⌥" : "") + (event.shiftKey && event.key.length > 1 ? "⇧" : "");
    // A letter with ⌘ or ⌃ reads as a shortcut, upper case (⌘K).
    const shortcut = (event.metaKey || event.ctrlKey) && event.key.length === 1;
    note({ kind: "key", key: (mods + (shortcut ? key.toUpperCase() : key)).slice(0, 24), at: Date.now() });
  }, true);
  document.addEventListener("pointerdown", (event) => note({ kind: "click", x: event.clientX, y: event.clientY, at: Date.now() }), true);
})()
