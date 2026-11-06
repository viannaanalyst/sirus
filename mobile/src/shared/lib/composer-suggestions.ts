export interface ComposerTrigger { kind: "skill" | "file"; query: string; start: number; end: number }

/**
 * The `/skill` or `@file` token under the caret, or null. Only standalone tokens at a collapsed
 * caret count (URLs, emails and selections stay plain text); quoted `@"a b"` paths allow spaces.
 * Ported from the desktop composer (`composer-suggestions.ts`).
 */
export function composerTrigger(value: string, start: number, end = start): ComposerTrigger | null {
  if (start !== end || start < 0 || start > value.length) return null;
  const match = /(?:^|\s)(\/[\w-]*|@(?:"(?:\\.|[^"\\\n])*|[^\s@"\n]*))$/.exec(value.slice(0, start));
  if (!match) return null;
  const token = match[1]!;
  const offset = start - token.length;
  const quoted = token.startsWith('@"');
  const remaining = value.slice(start);
  const suffix = quoted ? /^(?:\\.|[^"\\\n])*"?/.exec(remaining)?.[0] ?? "" : /^[^\s]*/.exec(remaining)?.[0] ?? "";
  return { kind: token[0] === "/" ? "skill" : "file", query: token.slice(quoted ? 2 : 1).replace(/\\"/g, '"'), start: offset, end: start + suffix.length };
}

/** Replaces the token with the chosen skill or path; a directory keeps the token open to go deeper. */
export function completeComposerToken(value: string, trigger: ComposerTrigger, selected: string, directory = false): { value: string; caret: number } {
  const path = directory ? `${selected}/` : selected;
  const quote = trigger.kind === "file" && /[\s"@]/.test(path);
  const token = trigger.kind === "skill" ? `/${selected}` : quote ? `@"${path.replace(/"/g, '\\"')}${directory ? "" : '"'}` : `@${path}`;
  const after = value.slice(trigger.end);
  const spacer = !directory && !/^\s/.test(after) ? " " : "";
  const next = value.slice(0, trigger.start) + token + spacer + after;
  return { value: next, caret: trigger.start + token.length + spacer.length };
}
