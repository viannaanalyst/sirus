/** Composer highlight segments: standalone `/skill` invocations and `@file` mentions (ADR-037). */
export interface ComposerSegment { text: string; kind: "text" | "skill" | "file" }

// A slash word with no further slash (so paths like /usr/bin stay plain), or an @mention, quoted or bare.
const TOKEN = /(^|\s)(\/[A-Za-z][\w-]*(?=\s|$)|@"(?:\\.|[^"\\\n])*"?|@[^\s@"]+)/g;

export function composerSegments(value: string): ComposerSegment[] {
  const segments: ComposerSegment[] = [];
  let last = 0;
  for (const match of value.matchAll(TOKEN)) {
    const start = match.index + match[1].length;
    if (start > last) segments.push({ text: value.slice(last, start), kind: "text" });
    segments.push({ text: match[2], kind: match[2].startsWith("/") ? "skill" : "file" });
    last = start + match[2].length;
  }
  if (last < value.length) segments.push({ text: value.slice(last), kind: "text" });
  return segments;
}
