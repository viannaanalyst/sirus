(() => {
  try {
    const spec = JSON.parse(args);
    const cfg = spec.sirusPointer;
    if (!cfg || Date.now() - (window.__sirusHumanAt || 0) < 1500) return;
    const find = (s) => !s ? null : s.ref ? document.querySelector("[data-sirus-ref='" + s.ref + "']") : (s.selector ? document.querySelector(s.selector) : null);
    const center = (el) => { const r = el.getBoundingClientRect(); return { x: r.left + r.width / 2, y: r.top + r.height / 2 }; };
    const isPoint = (s) => s && typeof s.x === "number" && typeof s.y === "number";
    const kind = cfg.kind;
    let start = null;
    let target = null;
    if (kind === "browser_click" || kind === "browser_type") {
      const el = find(spec);
      if (!el) return;
      // The tool scrolls the same way next, so the point is where it acts.
      el.scrollIntoView({ block: "center" });
      const r = el.getBoundingClientRect();
      target = kind === "browser_type" ? { x: r.left + Math.min(r.width / 2, 16), y: r.top + r.height / 2 } : center(el);
    } else if (kind === "browser_drag") {
      const a = isPoint(spec.from) ? null : find(spec.from);
      if (a) a.scrollIntoView({ block: "center", inline: "center" });
      start = a ? center(a) : isPoint(spec.from) ? { x: spec.from.x, y: spec.from.y } : null;
      const b = isPoint(spec.to) ? null : find(spec.to);
      target = b ? center(b) : isPoint(spec.to) ? { x: spec.to.x, y: spec.to.y } : null;
      if (!start || !target) return;
    }

    const ns = "http://www.w3.org/2000/svg";
    const s = cfg.scale > 0 ? cfg.scale : 1;
    let P = window.__sirusAgentPointer;
    if (!P || !P.host) {
      const host = document.createElement("div");
      const root = host.attachShadow({ mode: "closed" });
      const wrap = document.createElement("div");
      Object.assign(wrap.style, { position: "fixed", inset: "0", pointerEvents: "none", overflow: "hidden" });
      root.appendChild(wrap);
      const dots = [];
      for (let i = 0; i < 12; i++) {
        const d = document.createElement("div");
        Object.assign(d.style, { position: "absolute", left: "0", top: "0", borderRadius: "50%", opacity: "0" });
        wrap.appendChild(d);
        dots.push(d);
      }
      const ring = document.createElement("div");
      Object.assign(ring.style, { position: "absolute", left: "0", top: "0", borderRadius: "50%", borderStyle: "solid", opacity: "0", boxSizing: "border-box" });
      wrap.appendChild(ring);
      const arrow = document.createElementNS(ns, "svg");
      arrow.setAttribute("viewBox", "-2 -2 18 25");
      arrow.style.position = "absolute";
      arrow.style.overflow = "visible";
      const shape = document.createElementNS(ns, "path");
      shape.setAttribute("d", "M0 0 L0 17 L4.6 13.2 L7.6 20 L10.4 18.8 L7.5 12.2 L13 12.2 Z");
      shape.setAttribute("stroke", "#fff");
      shape.setAttribute("stroke-width", "1.4");
      shape.setAttribute("stroke-linejoin", "round");
      arrow.appendChild(shape);
      const hint = document.createElementNS(ns, "path");
      hint.setAttribute("fill", "none");
      hint.setAttribute("stroke-width", "1.8");
      hint.setAttribute("stroke-linecap", "round");
      hint.setAttribute("stroke-linejoin", "round");
      arrow.appendChild(hint);
      const pill = document.createElement("div");
      Object.assign(pill.style, { position: "absolute", left: "0", top: "0", display: "flex", alignItems: "center", whiteSpace: "nowrap", borderRadius: "999px", borderStyle: "solid", background: "rgba(20,20,22,0.9)", color: "#fff", font: "500 11px -apple-system, system-ui, sans-serif", lineHeight: "14px", boxSizing: "border-box" });
      const mark = document.createElementNS(ns, "svg");
      const label = document.createElement("span");
      pill.appendChild(mark);
      pill.appendChild(label);
      wrap.appendChild(arrow);
      wrap.appendChild(pill);
      P = window.__sirusAgentPointer = { host, wrap, dots, ring, arrow, shape, hint, pill, mark, label, last: -1e9, raf: 0 };
    }
    const host = P.host;
    Object.assign(host.style, { position: "fixed", inset: "0", pointerEvents: "none", zIndex: "2147483647", display: "block", contain: "strict" });
    host.setAttribute("aria-hidden", "true");
    if (!host.isConnected) document.documentElement.appendChild(host);

    // Look for this provider.
    const color = cfg.color;
    P.shape.setAttribute("fill", color);
    P.hint.setAttribute("stroke", color);
    P.ring.style.borderColor = color;
    P.ring.style.borderWidth = 2 * s + "px";
    P.ring.style.background = "color-mix(in srgb, " + color + " 12%, transparent)";
    P.arrow.style.width = 18 * s + "px";
    P.arrow.style.height = 25 * s + "px";
    P.arrow.style.filter = "drop-shadow(0 " + s + "px " + 1.5 * s + "px rgba(0,0,0,0.3))";
    Object.assign(P.pill.style, { gap: 4 * s + "px", padding: 2 * s + "px " + 7 * s + "px " + 2 * s + "px " + 5 * s + "px", borderWidth: s + "px", borderColor: color, fontSize: 11 * s + "px", lineHeight: 14 * s + "px" });
    for (const d of P.dots) { d.style.width = d.style.height = 5 * s + "px"; d.style.background = color; }
    if (P.provider !== cfg.provider) {
      P.provider = cfg.provider;
      while (P.mark.firstChild) P.mark.removeChild(P.mark.firstChild);
      if (cfg.mark) {
        P.mark.setAttribute("viewBox", cfg.mark.viewBox);
        for (const d of cfg.mark.paths) {
          const path = document.createElementNS(ns, "path");
          path.setAttribute("d", d);
          path.setAttribute("fill", color);
          if (cfg.mark.evenOdd) path.setAttribute("fill-rule", "evenodd");
          P.mark.appendChild(path);
        }
      } else {
        P.mark.setAttribute("viewBox", "0 0 10 10");
        const dot = document.createElementNS(ns, "circle");
        dot.setAttribute("cx", "5"); dot.setAttribute("cy", "5"); dot.setAttribute("r", "3.5"); dot.setAttribute("fill", color);
        P.mark.appendChild(dot);
      }
      P.label.textContent = cfg.label;
    }
    P.mark.style.width = P.mark.style.height = 11 * s + "px";
    P.mark.style.flex = "none";

    // The motion, as agent_pointer.rs's `Pointer`.
    const bezier = (t, x1, y1, x2, y2) => {
      const at = (a, b, u) => 3 * a * u * (1 - u) * (1 - u) + 3 * b * u * u * (1 - u) + u * u * u;
      let lo = 0, hi = 1;
      for (let i = 0; i < 12; i++) { const mid = (lo + hi) / 2; if (at(x1, x2, mid) < t) lo = mid; else hi = mid; }
      return at(y1, y2, (lo + hi) / 2);
    };
    const ease = (t) => t <= 0 ? 0 : t >= 1 ? 1 : bezier(t, cfg.ease[0], cfg.ease[1], cfg.ease[2], cfg.ease[3]);
    const clamp = (v) => Math.min(1, Math.max(0, v));
    const now = performance.now();
    const reduced = !!cfg.reduced;
    const visible = now - P.last < cfg.hideMs + cfg.fadeMs;
    const position = (t) => {
      if (reduced) return P.to;
      const k = ease((t - P.movedAt) / cfg.glideMs);
      return { x: P.from.x + (P.to.x - P.from.x) * k, y: P.from.y + (P.to.y - P.from.y) * k };
    };
    const fallback = { x: window.innerWidth / 2, y: window.innerHeight / 2 };
    if (!visible) {
      const at = start || target || P.to || fallback;
      P.from = at; P.to = target || at; P.shownAt = now; P.movedAt = now;
    } else {
      const here = start || position(now);
      const to = target || P.to;
      if (Math.hypot(to.x - here.x, to.y - here.y) > 0.5) P.movedAt = now;
      P.from = here; P.to = to;
    }
    P.kind = kind;
    P.down = (spec.dy ?? 0) >= 0;
    P.last = now;
    const moved = P.from.x !== P.to.x || P.from.y !== P.to.y;
    P.rippleAt = kind === "browser_click" || kind === "browser_drag" ? now + (moved && !reduced ? cfg.glideMs : 0) : null;

    const draw = () => {
      const t = performance.now();
      const appear = clamp((t - P.shownAt) / cfg.fadeMs);
      const hide = 1 - clamp((t - P.last - cfg.hideMs) / cfg.fadeMs);
      const fadeIn = reduced ? clamp((t - P.movedAt) / cfg.fadeMs) : 1;
      const opacity = Math.min(appear, hide, fadeIn);
      const p = position(t);
      P.arrow.style.transform = "translate(" + (p.x - 2 * s) + "px," + (p.y - 2 * s) + "px)";
      P.arrow.style.opacity = opacity;
      P.pill.style.transform = "translate(" + (p.x + 15 * s) + "px," + (p.y + 17 * s) + "px)";
      P.pill.style.opacity = opacity;
      // The scroll hint: two chevrons under the arrow, in the arrow's own units.
      const scroll = P.kind === "browser_scroll";
      const tip = P.down ? 4 : -4;
      const step = P.down ? 5 : -5;
      P.hint.setAttribute("d", scroll ? ["M1 " + (30 - tip / 2) + " L6 " + (30 + tip / 2) + " L11 " + (30 - tip / 2), "M1 " + (30 + step - tip / 2) + " L6 " + (30 + step + tip / 2) + " L11 " + (30 + step - tip / 2)].join(" ") : "");
      // The trail.
      const landed = t - (P.movedAt + cfg.glideMs);
      const trailFade = 1 - clamp(landed / cfg.trailFadeMs);
      P.dots.forEach((d, i) => {
        const k = i + 1;
        const at = t - k * cfg.trailGapMs;
        if (reduced || !moved || k > cfg.trailDots || at < P.movedAt || trailFade <= 0) { d.style.opacity = "0"; return; }
        const q = position(at);
        d.style.transform = "translate(" + (q.x - 2.5 * s) + "px," + (q.y - 2.5 * s) + "px)";
        d.style.opacity = 0.55 * (1 - k / (cfg.trailDots + 1)) * trailFade * opacity;
      });
      // The click's ring.
      const e = P.rippleAt == null ? -1 : t - P.rippleAt;
      if (e >= 0 && e < cfg.rippleMs) {
        const u = e / cfg.rippleMs;
        const size = (reduced ? (cfg.rippleFrom + cfg.rippleTo) / 2 : cfg.rippleFrom + (cfg.rippleTo - cfg.rippleFrom) * (1 - Math.pow(1 - u, 3))) * s;
        Object.assign(P.ring.style, { width: size + "px", height: size + "px", transform: "translate(" + (P.to.x - size / 2) + "px," + (P.to.y - size / 2) + "px)", opacity: String((1 - u) * hide) });
      } else {
        P.ring.style.opacity = "0";
      }
      if (t - P.last >= cfg.hideMs + cfg.fadeMs) {
        P.host.style.display = "none";
        P.raf = 0;
        return;
      }
      P.raf = requestAnimationFrame(draw);
    };
    if (!P.raf) P.raf = requestAnimationFrame(draw);
  } catch (_) {
    // The pointer is only a picture; the tool runs whatever happens here.
  }
})();
