// "Compartilhar prévia" (ADR-210): the open page as one self-contained HTML document. Runs as the
// body of an async function (`callAsyncJavaScript`); `args` is the JSON text `{ "limit": bytes }`.
// Answers a JSON string: `{ "title", "html" }`, or `{ "error": "large" | "<message>" }`.
//
// The page's DOM is cloned (the live page is never touched). Every stylesheet is read from the
// CSSOM (or fetched when the browser hides its rules) and put in one <style>, with the url()s in
// it turned into data: URIs; images and canvases become data: URIs; scripts, frames, plugins,
// <base>, <link>, refresh redirects, event handlers and javascript: links are dropped, and the
// values of password and hidden fields are never copied. Pictures the page keeps out of reach
// (cross-origin without CORS, drawn in a shadow root, background images of a blocked fetch) are
// left out rather than failing the capture.
try {
  const opts = JSON.parse(args || "{}");
  const LIMIT = opts.limit || 15 * 1024 * 1024;
  const DEADLINE = Date.now() + 25000;
  let used = 0;

  const absolute = (url, base) => {
    try { return new URL(url, base || document.baseURI).href; } catch (e) { return null; }
  };
  const asData = (blob) => new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result);
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
  const fetchBlob = async (url, maxBytes) => {
    if (Date.now() > DEADLINE) return null;
    const full = absolute(url);
    if (!full || !/^(https?:|blob:)/i.test(full)) return null;
    const control = new AbortController();
    const timer = setTimeout(() => control.abort(), 8000);
    try {
      const response = await fetch(full, { signal: control.signal, credentials: "include" });
      if (!response.ok) return null;
      const blob = await response.blob();
      if (blob.size > maxBytes || used + blob.size * 1.4 > LIMIT) return null;
      return blob;
    } catch (e) {
      return null;
    } finally {
      clearTimeout(timer);
    }
  };
  const fetchData = async (url, maxBytes) => {
    const blob = await fetchBlob(url, maxBytes);
    if (!blob) return null;
    try {
      const data = await asData(blob);
      used += data.length;
      return data;
    } catch (e) {
      return null;
    }
  };
  const fetchText = async (url) => {
    const blob = await fetchBlob(url, 2 * 1024 * 1024);
    return blob ? await blob.text() : null;
  };
  const inBatches = async (items, work) => {
    for (let at = 0; at < items.length; at += 6) {
      await Promise.all(items.slice(at, at + 6).map(work));
    }
  };

  // url(...) in CSS text: data: URIs for what can be fetched, `none` for what cannot.
  const URLS = /url\(\s*(['"]?)([^'")]+)\1\s*\)/g;
  const keep = (value) => /^(data:|#|about:)/i.test(value);
  const inlineUrls = async (css, base) => {
    const wanted = new Map();
    for (const match of css.matchAll(URLS)) {
      if (!keep(match[2]) && wanted.size < 60) wanted.set(match[2], null);
    }
    await inBatches([...wanted.keys()], async (value) => {
      wanted.set(value, await fetchData(absolute(value, base), 3 * 1024 * 1024));
    });
    return css.replace(URLS, (all, quote, value) => {
      if (keep(value)) return all;
      const data = wanted.get(value);
      return data ? 'url("' + data + '")' : "none";
    });
  };
  const sheetCss = async (sheet, depth) => {
    let rules = null;
    try { rules = sheet.cssRules; } catch (e) { rules = null; }
    if (!rules) {
      if (sheet.href && depth < 4) {
        const text = await fetchText(sheet.href);
        if (text) return await inlineUrls(text, sheet.href);
      }
      return "";
    }
    let out = "";
    for (const rule of rules) {
      if (rule.type === 3 && rule.styleSheet && depth < 4) {
        out += await sheetCss(rule.styleSheet, depth + 1);
      } else {
        out += rule.cssText + "\n";
      }
    }
    return await inlineUrls(out, sheet.href || document.baseURI);
  };

  let css = "";
  const sheets = Array.from(document.styleSheets).concat(Array.from(document.adoptedStyleSheets || []));
  for (const sheet of sheets) {
    if (sheet.disabled) continue;
    let text = await sheetCss(sheet, 0);
    const media = sheet.media && sheet.media.mediaText;
    if (text && media && media !== "all") text = "@media " + media + "{" + text + "}";
    css += text;
    if (css.length > LIMIT / 2) break;
  }

  const clone = document.documentElement.cloneNode(true);
  const live = (selector) => Array.from(document.querySelectorAll(selector));
  const copy = (selector) => Array.from(clone.querySelectorAll(selector));

  // Pictures first, while the clone still lines up element by element with the page.
  const liveImages = live("img");
  const copyImages = copy("img");
  await inBatches(liveImages.map((_, at) => at), async (at) => {
    const image = liveImages[at];
    const target = copyImages[at];
    if (!target) return;
    const source = image.currentSrc || image.src;
    target.removeAttribute("srcset");
    target.removeAttribute("loading");
    if (!source) return;
    if (/^data:/i.test(source)) { target.setAttribute("src", source); return; }
    let data = await fetchData(source, 6 * 1024 * 1024);
    if (!data && image.complete && image.naturalWidth > 0) {
      try {
        const scale = Math.min(1, 2000 / Math.max(image.naturalWidth, image.naturalHeight));
        const canvas = document.createElement("canvas");
        canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
        canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
        canvas.getContext("2d").drawImage(image, 0, 0, canvas.width, canvas.height);
        data = canvas.toDataURL(canvas.width * canvas.height > 400000 ? "image/jpeg" : "image/png", 0.85);
        used += data.length;
      } catch (e) {
        data = null;
      }
    }
    if (data) target.setAttribute("src", data); else target.removeAttribute("src");
  });
  // SVG <image> elements.
  const svgImages = copy("image");
  await inBatches(svgImages, async (node) => {
    const link = node.getAttribute("href") || node.getAttribute("xlink:href");
    if (!link || /^data:/i.test(link)) return;
    const data = await fetchData(link, 3 * 1024 * 1024);
    node.removeAttribute("xlink:href");
    if (data) node.setAttribute("href", data); else node.removeAttribute("href");
  });
  // Canvases become pictures.
  const liveCanvases = live("canvas");
  const copyCanvases = copy("canvas");
  liveCanvases.forEach((canvas, at) => {
    try {
      const data = canvas.toDataURL("image/png");
      if (!copyCanvases[at] || used + data.length > LIMIT) return;
      used += data.length;
      const picture = document.createElement("img");
      picture.setAttribute("src", data);
      for (const name of ["width", "height", "class", "style", "id"]) {
        const value = copyCanvases[at].getAttribute(name);
        if (value !== null) picture.setAttribute(name, value);
      }
      copyCanvases[at].replaceWith(picture);
    } catch (e) { /* a tainted canvas stays out */ }
  });
  // Videos become the frame on screen (or their poster): a page that is mostly a video would
  // otherwise come out blank once <video> is dropped below.
  const liveVideos = live("video");
  const copyVideos = copy("video");
  await inBatches(liveVideos.map((_, at) => at), async (at) => {
    const video = liveVideos[at];
    const target = copyVideos[at];
    if (!target) return;
    let data = null;
    if (video.readyState >= 2 && video.videoWidth > 0) {
      try {
        const scale = Math.min(1, 1600 / Math.max(video.videoWidth, video.videoHeight));
        const canvas = document.createElement("canvas");
        canvas.width = Math.max(1, Math.round(video.videoWidth * scale));
        canvas.height = Math.max(1, Math.round(video.videoHeight * scale));
        canvas.getContext("2d").drawImage(video, 0, 0, canvas.width, canvas.height);
        data = canvas.toDataURL("image/jpeg", 0.85);
      } catch (e) { data = null; }
    }
    if (!data && video.poster) data = await fetchData(video.poster, 6 * 1024 * 1024);
    if (!data || used + data.length > LIMIT) return;
    used += data.length;
    const picture = document.createElement("img");
    picture.setAttribute("src", data);
    const box = video.getBoundingClientRect();
    for (const name of ["class", "style", "id", "width", "height"]) {
      const value = target.getAttribute(name);
      if (value !== null) picture.setAttribute(name, value);
    }
    if (!target.getAttribute("width") && box.width) picture.setAttribute("width", String(Math.round(box.width)));
    if (!picture.style.objectFit) picture.style.objectFit = getComputedStyle(video).objectFit || "contain";
    target.replaceWith(picture);
  });
  // What was typed or chosen stays visible; secrets never leave.
  const liveFields = live("input,textarea,select");
  const copyFields = copy("input,textarea,select");
  liveFields.forEach((field, at) => {
    const target = copyFields[at];
    if (!target) return;
    const tag = field.tagName.toLowerCase();
    if (tag === "textarea") { target.textContent = field.value; return; }
    if (tag === "select") {
      Array.from(target.options).forEach((option, index) => {
        if (field.options[index] && field.options[index].selected) option.setAttribute("selected", ""); else option.removeAttribute("selected");
      });
      return;
    }
    const type = (field.type || "").toLowerCase();
    if (type === "password" || type === "hidden" || type === "file") { target.removeAttribute("value"); return; }
    if (type === "checkbox" || type === "radio") {
      if (field.checked) target.setAttribute("checked", ""); else target.removeAttribute("checked");
      return;
    }
    target.setAttribute("value", field.value);
  });
  // Inline style="…url(…)".
  await inBatches(copy("[style*='url(']"), async (node) => {
    node.setAttribute("style", await inlineUrls(node.getAttribute("style") || "", document.baseURI));
  });

  // Nothing active.
  copy("script,noscript,iframe,frame,frameset,object,embed,applet,base,link,style,meta[http-equiv],meta[charset],audio,video,source,track").forEach((node) => node.remove());
  for (const node of [clone, ...copy("*")]) {
    for (const attribute of Array.from(node.attributes)) {
      const name = attribute.name.toLowerCase();
      if (name.startsWith("on") || name === "srcdoc" || name === "integrity" || name === "nonce" || name === "crossorigin" || name === "action" || name === "formaction") {
        node.removeAttribute(attribute.name);
      } else if ((name === "href" || name === "src" || name === "xlink:href") && /^\s*javascript:/i.test(attribute.value)) {
        node.removeAttribute(attribute.name);
      }
    }
  }
  // Links: whole addresses (the page's own relative ones would lead nowhere), none that run code.
  copy("a[href]").forEach((link) => {
    const value = link.getAttribute("href") || "";
    if (value.startsWith("#")) return;
    const full = absolute(value);
    if (full && /^https?:/i.test(full)) {
      link.setAttribute("href", full);
      link.setAttribute("rel", "noopener noreferrer nofollow");
    } else {
      link.removeAttribute("href");
    }
  });

  let head = clone.querySelector("head");
  if (!head) {
    head = document.createElement("head");
    clone.insertBefore(head, clone.firstChild);
  }
  const charset = document.createElement("meta");
  charset.setAttribute("charset", "utf-8");
  head.insertBefore(charset, head.firstChild);
  if (css) {
    const style = document.createElement("style");
    style.textContent = css.replace(/<\/style/gi, "<\\/style");
    head.appendChild(style);
  }

  const html = "<!doctype html>\n" + clone.outerHTML;
  if (new Blob([html]).size > LIMIT) return JSON.stringify({ error: "large" });
  return JSON.stringify({ title: document.title || location.hostname || "", html });
} catch (error) {
  return JSON.stringify({ error: String((error && error.message) || error) });
}
