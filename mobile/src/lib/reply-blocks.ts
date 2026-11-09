/**
 * A reply's markdown cut into the pieces the phone draws differently: Mermaid fences (a diagram),
 * math (a ```math fence or a `$$` block, drawn by the native LaTeX engine) and everything else,
 * which stays markdown. The same two cases the desktop draws in `reply_render.rs`.
 *
 * Only a block that is closed is cut out, so a fence still streaming stays a code block until it
 * ends. Fences inside other fences, and `$$` inside code, are left alone.
 */
export type ReplyBlock =
  | { readonly kind: "markdown"; readonly text: string }
  | { readonly kind: "mermaid"; readonly code: string }
  | { readonly kind: "math"; readonly code: string };

/** Diagram source above this stays a code block (the desktop's `MAX_SOURCE`). */
export const MAX_DIAGRAM_SOURCE = 20_000;

const FENCE = /^( {0,3})(`{3,}|~{3,})[ \t]*([^\s`]*)[^\n]*$/;

export function splitReplyBlocks(markdown: string): ReplyBlock[] {
  if (!/mermaid|math|latex|\$\$/i.test(markdown)) return [{ kind: "markdown", text: markdown }];
  const lines = markdown.split("\n");
  const blocks: ReplyBlock[] = [];
  let pending: string[] = [];
  const flush = () => {
    const text = pending.join("\n");
    if (text.trim()) blocks.push({ kind: "markdown", text });
    pending = [];
  };
  let at = 0;
  while (at < lines.length) {
    const line = lines[at]!;
    const fence = FENCE.exec(line);
    if (fence) {
      const marker = fence[2]!;
      const closing = new RegExp(`^ {0,3}${marker[0] === "`" ? "`" : "~"}{${marker.length},}[ \\t]*$`);
      let end = at + 1;
      while (end < lines.length && !closing.test(lines[end]!)) end++;
      const closed = end < lines.length;
      const language = fence[3]!.toLowerCase();
      const code = lines.slice(at + 1, end).join("\n");
      if (closed && language === "mermaid" && code.length <= MAX_DIAGRAM_SOURCE && code.trim()) {
        flush();
        blocks.push({ kind: "mermaid", code });
      } else if (closed && (language === "math" || language === "latex") && code.trim()) {
        flush();
        blocks.push({ kind: "math", code });
      } else {
        // Any other fence, open or not, is copied whole so nothing inside it is read as a block.
        pending.push(...lines.slice(at, closed ? end + 1 : end));
      }
      at = closed ? end + 1 : end;
      continue;
    }
    const dollars = /^ {0,3}\$\$(.*)$/.exec(line);
    if (dollars) {
      const rest = dollars[1]!;
      const same = /^(.+?)\$\$[ \t]*$/.exec(rest);
      if (same) {
        flush();
        blocks.push({ kind: "math", code: same[1]!.trim() });
        at++;
        continue;
      }
      if (!rest.includes("$$") && !rest.trim()) {
        let end = at + 1;
        while (end < lines.length && !/^\s*\$\$[ \t]*$/.test(lines[end]!)) end++;
        if (end < lines.length) {
          const code = lines.slice(at + 1, end).join("\n");
          if (code.trim()) {
            flush();
            blocks.push({ kind: "math", code });
            at = end + 1;
            continue;
          }
        }
      }
    }
    pending.push(line);
    at++;
  }
  flush();
  return blocks.length ? blocks : [{ kind: "markdown", text: markdown }];
}

/** The markdown the native engine draws as one display formula. A blank line would end it early. */
export function mathMarkdown(code: string): string {
  return `$$\n${code.replace(/\n\s*\n/g, "\n").trim()}\n$$`;
}
