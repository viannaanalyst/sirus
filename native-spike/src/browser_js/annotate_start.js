(() => {
  const KEY = "__sirusAnnotations";
  const current = window[KEY];
  if (current && current.active) return "already";
  const entries = [];
  const nodes = [];
  const targets = [];
  const box = document.createElement("div");
  box.style.cssText = "position:fixed;pointer-events:none;z-index:2147483646;border:2px solid #3b82f6;border-radius:3px;box-shadow:0 0 0 1px rgba(59,130,246,.35);display:none";
  const hud = document.createElement("div");
  hud.style.cssText = "position:fixed;pointer-events:none;z-index:2147483647;display:none;background:rgba(10,10,10,.94);color:#f5f5f5;font:11px/17px ui-monospace,SFMono-Regular,Menlo,monospace;border-radius:8px;padding:7px 10px;white-space:pre;box-shadow:0 8px 24px rgba(0,0,0,.4)";
  const describe = (el) => {
    const rect = el.getBoundingClientRect();
    const style = getComputedStyle(el);
    const tag = el.tagName.toLowerCase();
    const name = el.getAttribute("aria-label") || el.getAttribute("placeholder") || el.getAttribute("title") || (el.innerText || "").trim().replace(/\s+/g, " ").slice(0, 60);
    return (name ? tag + " \"" + name + "\"" : tag)
      + "\n" + Math.round(rect.width) + "x" + Math.round(rect.height)
      + "\ncolor " + style.color
      + "\nfont " + style.fontSize + " " + style.fontFamily
      + "\npadding " + style.padding;
  };
  const selectorFor = (el) => {
    const parts = [];
    let node = el;
    while (node && node.nodeType === 1 && parts.length < 6) {
      let part = node.tagName.toLowerCase();
      if (node.id) { parts.unshift(part + "#" + node.id); break; }
      const parent = node.parentElement;
      if (parent) {
        const siblings = Array.from(parent.children).filter((child) => child.tagName === node.tagName);
        if (siblings.length > 1) part += ":nth-of-type(" + (siblings.indexOf(node) + 1) + ")";
      }
      parts.unshift(part);
      node = node.parentElement;
    }
    return parts.join(" > ").slice(0, 400);
  };
  const labelFor = (el) => {
    const tag = el.tagName.toLowerCase();
    const rect = el.getBoundingClientRect();
    const name = (el.getAttribute("aria-label") || el.getAttribute("placeholder") || el.getAttribute("title") || (el.innerText || "").trim().replace(/\s+/g, " ")).slice(0, 80);
    return name ? tag + " \"" + name + "\"" : tag + " " + Math.round(rect.width) + "x" + Math.round(rect.height);
  };
  const move = (event) => {
    const target = document.elementFromPoint(event.clientX, event.clientY);
    if (!target || target === box || target === hud) { box.style.display = "none"; hud.style.display = "none"; return; }
    const rect = target.getBoundingClientRect();
    box.style.display = "block";
    box.style.left = rect.left + "px"; box.style.top = rect.top + "px";
    box.style.width = rect.width + "px"; box.style.height = rect.height + "px";
    hud.textContent = describe(target);
    hud.style.display = "block";
    hud.style.left = Math.max(8, Math.min(event.clientX + 14, window.innerWidth - 260)) + "px";
    hud.style.top = Math.max(8, Math.min(event.clientY + 16, window.innerHeight - 130)) + "px";
  };
  const click = (event) => {
    const target = event.target;
    if (!target || target === box || target === hud) return;
    event.preventDefault();
    event.stopPropagation();
    const rect = target.getBoundingClientRect();
    const marker = document.createElement("div");
    marker.style.cssText = "position:fixed;pointer-events:none;z-index:2147483647;background:#3b82f6;color:#fff;border-radius:999px;font:600 10px/16px -apple-system,sans-serif;min-width:16px;height:16px;text-align:center;padding:0 4px;box-shadow:0 1px 4px rgba(0,0,0,.4)";
    marker.textContent = String(entries.length + 1);
    marker.style.left = Math.max(0, rect.left - 6) + "px";
    marker.style.top = Math.max(0, rect.top - 6) + "px";
    document.body.appendChild(marker);
    nodes.push(marker);
    targets.push(target);
    entries.push({ selector: selectorFor(target), label: labelFor(target) });
  };
  // Hides the hover outline, puts each marker back on its element and returns where they are.
  const measure = () => {
    box.style.display = "none";
    hud.style.display = "none";
    return {
      url: location.href, width: window.innerWidth, height: window.innerHeight,
      entries: entries.map((entry, index) => {
        const target = targets[index];
        const rect = target && target.isConnected ? target.getBoundingClientRect() : null;
        if (rect && nodes[index]) {
          nodes[index].style.left = Math.max(0, rect.left - 6) + "px";
          nodes[index].style.top = Math.max(0, rect.top - 6) + "px";
        }
        return { selector: entry.selector, label: entry.label, x: rect ? rect.left : 0, y: rect ? rect.top : 0, width: rect ? rect.width : 0, height: rect ? rect.height : 0 };
      }),
    };
  };
  const cancel = () => {
    document.removeEventListener("mousemove", move, true);
    document.removeEventListener("click", click, true);
    document.removeEventListener("keydown", key, true);
    box.remove();
    hud.remove();
    nodes.forEach((node) => node.remove());
    window[KEY] = null;
  };
  const key = (event) => { if (event.key === "Escape") cancel(); };
  document.addEventListener("mousemove", move, true);
  document.addEventListener("click", click, true);
  document.addEventListener("keydown", key, true);
  document.body.appendChild(box);
  document.body.appendChild(hud);
  window[KEY] = { active: true, entries, cancel, measure };
  return "started";
})()
