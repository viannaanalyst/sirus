const nodes = Array.from(document.querySelectorAll("a,button,input,textarea,select,[role='button'],[role='link'],[contenteditable='true']"));
const elements = [];
let count = 0;
for (const el of nodes) {
  if (elements.length >= 200) break;
  const rect = el.getBoundingClientRect();
  if (rect.width < 1 || rect.height < 1) continue;
  const ref = "e" + (++count);
  el.setAttribute("data-sirus-ref", ref);
  elements.push({
    ref,
    tag: el.tagName.toLowerCase(),
    role: el.getAttribute("role") || "",
    name: (el.getAttribute("aria-label") || el.getAttribute("placeholder") || el.innerText || el.value || "").trim().slice(0, 120),
    value: (el.value ?? "").toString().slice(0, 80),
    disabled: !!el.disabled,
  });
}
return JSON.stringify({ url: location.href, title: document.title, elements });
