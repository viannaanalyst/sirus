// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";
import assert from "node:assert/strict";
import { mathMarkdown, splitReplyBlocks } from "../src/lib/reply-blocks.ts";
import { mermaidSvgForNative } from "../src/lib/mermaid-svg.ts";
import { imageExtension } from "../src/lib/image-extension.ts";

test("a reply with no diagram or formula stays one markdown block", () => {
  const text = "Olá\n\n```ts\nconst a = 1;\n```\n\nPreço: $5 e $10.";
  assert.deepEqual(splitReplyBlocks(text), [{ kind: "markdown", text }]);
});

test("a closed mermaid fence becomes a diagram between its paragraphs", () => {
  const blocks = splitReplyBlocks("Antes\n\n```mermaid\ngraph TD\n  A-->B\n```\n\nDepois");
  assert.deepEqual(blocks.map((block) => block.kind), ["markdown", "mermaid", "markdown"]);
  assert.equal(blocks[1].code, "graph TD\n  A-->B");
});

test("a fence still streaming stays a code block", () => {
  const text = "Veja:\n\n```mermaid\ngraph TD\n  A-->B";
  assert.deepEqual(splitReplyBlocks(text), [{ kind: "markdown", text }]);
});

test("math comes from a math fence, a $$ block or a one-line $$ formula", () => {
  const blocks = splitReplyBlocks("a\n\n```math\nx^2\n```\n\n$$\n\\frac{a}{b}\n$$\n\n$$E=mc^2$$\n\nfim");
  assert.deepEqual(blocks.map((block) => block.kind), ["markdown", "math", "math", "math", "markdown"]);
  assert.deepEqual(blocks.filter((block) => block.kind === "math").map((block) => block.code), ["x^2", "\\frac{a}{b}", "E=mc^2"]);
});

test("fences and $$ inside other code are not read as blocks", () => {
  const text = "````md\n```mermaid\ngraph TD\n```\n$$\nx\n$$\n````";
  assert.deepEqual(splitReplyBlocks(text), [{ kind: "markdown", text }]);
  const other = "```js\nconst s = `$$`;\n$$\nx\n$$\n```";
  assert.deepEqual(splitReplyBlocks(other), [{ kind: "markdown", text: other }]);
});

test("a diagram above the size limit stays a code block", () => {
  const text = `\`\`\`mermaid\n${"A-->B\n".repeat(5000)}\`\`\``;
  assert.equal(splitReplyBlocks(text)[0].kind, "markdown");
});

test("a formula is wrapped for the native engine without blank lines", () => {
  assert.equal(mathMarkdown("a\n\nb "), "$$\na\nb\n$$");
});

test("the diagram's CSS variables and styles become attributes react-native-svg understands", () => {
  const svg = '<svg viewBox="0 0 10 10" style="--pele-bg:#fff;font-family:Inter;font:14px Inter"><rect style="fill:var(--pele-fg);rx:4px" width="10"/></svg>';
  const out = mermaidSvgForNative(svg, { "--pele-fg": "#111" });
  assert.match(out, /<rect fill="#111" rx="4" width="10"\/>/);
  assert.match(out, /font-size="14"/);
  assert.doesNotMatch(out, /var\(/);
});

test("the extension Photos needs comes from the type, else the address", () => {
  assert.equal(imageExtension("data:image/png;base64,AAAA"), "png");
  assert.equal(imageExtension("https://x.test/a/b.JPEG?token=1"), "jpg");
  assert.equal(imageExtension("file:///tmp/x"), "jpg");
});
