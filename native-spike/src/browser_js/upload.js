const input = JSON.parse(args);
const find = (spec) => spec.ref ? document.querySelector("[data-sirus-ref='" + spec.ref + "']") : (spec.selector ? document.querySelector(spec.selector) : null);
const humanBusy = () => Date.now() - (window.__sirusHumanAt || 0) < 1500;

if (humanBusy()) return JSON.stringify({ interrupted: true });
const target = find(input);
if (!target) return JSON.stringify({ ok: false, error: "element not found" });
const transfer = new DataTransfer();
for (const file of input.files) {
  const binary = atob(file.data);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  transfer.items.add(new File([bytes], file.name, { type: file.type }));
}
if (target instanceof HTMLInputElement && target.type === "file") {
  if (!target.multiple && input.files.length > 1) return JSON.stringify({ ok: false, error: "this file input takes one file" });
  if (target.disabled) return JSON.stringify({ ok: false, error: "the file input is disabled" });
  target.files = transfer.files;
  target.dispatchEvent(new Event("input", { bubbles: true }));
  target.dispatchEvent(new Event("change", { bubbles: true }));
  return JSON.stringify({ ok: true, files: target.files.length, via: "input" });
}
// Not a file input: drop the files on it, as a drag-and-drop upload area expects.
target.scrollIntoView({ block: "center" });
const rect = target.getBoundingClientRect();
const init = { bubbles: true, cancelable: true, composed: true, clientX: rect.left + rect.width / 2, clientY: rect.top + rect.height / 2, dataTransfer: transfer };
for (const type of ["dragenter", "dragover", "drop"]) target.dispatchEvent(new DragEvent(type, init));
return JSON.stringify({ ok: true, files: input.files.length, via: "drop" });
