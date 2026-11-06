import type { ActivityItem } from "../client/types";
import { maskSecrets } from "./redact";

/** The app's own row for a secret the person provided privately (ADR-077); it names the label only. */
export function isSecretRow(item: Pick<ActivityItem, "id" | "kind" | "label">): boolean {
  return item.kind === "tool" && item.label === "Secret provided" && item.id.startsWith("secret:");
}

/**
 * A turn: reply text and tool work in the order they happened.
 * Rows carry the reply length when they began (`offset`); back-to-back rows form one
 * work group. Rows from turns recorded before offsets existed sit at the start.
 */
export type TimelinePart =
  | { kind: "text"; start: number; end: number }
  | { kind: "steer"; text: string }
  | { kind: "work"; items: ActivityItem[] }
  /** Where a background subagent finished (ADR-101); the text after it is that subagent's answer. */
  | { kind: "marker"; item: ActivityItem };

export function timelineParts(content: string, items: readonly ActivityItem[], steers: readonly { offset: number; text: string }[] = []): TimelinePart[] {
  type Event = { at: number; order: number; item?: ActivityItem; steer?: string };
  const clamp = (at: number | undefined) => Math.max(0, Math.min(at ?? 0, content.length));
  const events: Event[] = [
    // A background shell's own row is its `Bash` call; only the strip and its marker show the task.
    ...items.map((item, order) => ({ at: clamp(item.offset), order, item })).filter(({ item }) => !item.shell || item.finishedTask),
    ...steers.map((steer, order) => ({ at: clamp(steer.offset), order: items.length + order, steer: steer.text })),
  ].sort((a, b) => a.at - b.at || a.order - b.order);
  const parts: TimelinePart[] = [];
  let cursor = 0;
  const text = (end: number) => {
    // Whitespace between two pieces of work does not split their group.
    if (end > cursor && content.slice(cursor, end).trim()) parts.push({ kind: "text", start: cursor, end });
    cursor = Math.max(cursor, end);
  };
  for (const event of events) {
    text(event.at);
    if (event.item?.finishedTask) {
      parts.push({ kind: "marker", item: event.item });
    } else if (event.item) {
      const last = parts.at(-1);
      if (last?.kind === "work") last.items.push(event.item);
      else parts.push({ kind: "work", items: [event.item] });
    } else {
      parts.push({ kind: "steer", text: event.steer! });
      // The reply resumes after the instruction without the blank lines between its items.
      while (content[cursor] === "\n") cursor += 1;
    }
  }
  text(content.length);
  return parts;
}

/** The first background-completion marker, or the end when there is none. */
function firstMarker(parts: readonly TimelinePart[]): number {
  const index = parts.findIndex((part) => part.kind === "marker");
  return index < 0 ? parts.length : index;
}

/**
 * The parts a finished turn folds away: everything before its final answer (the text after
 * the last work). With background-completion markers (ADR-101), only what came before the
 * turn's own answer folds; the markers and each subagent's reply after them stay visible.
 */
export function foldBoundary(parts: readonly TimelinePart[]): number {
  const end = firstMarker(parts);
  let index = end;
  while (index > 0 && parts[index - 1].kind === "text") index -= 1;
  if (index === end && end < parts.length) {
    // The answer came before the work that launched the subagents: it still stays in view.
    for (let last = end - 1; last >= 0; last -= 1) if (parts[last].kind === "text") return last;
  }
  return index;
}

export type StepCategory = "skill" | "edit" | "command" | "read" | "search" | "web" | "tool" | "agent";

export function stepCategory(item: Pick<ActivityItem, "kind" | "label" | "detail">): StepCategory {
  if (item.kind === "read") {
    if (item.label === "Web search") return "web";
    const detail = item.detail ?? "";
    // Globs and regex are searches; an absolute or relative path is a read even with spaces in it
    // ("Application Support"); otherwise a bare file name reads and a phrase searches.
    if (!detail || /[*?[\]{}|^$\\]/.test(detail)) return "search";
    if (/^(?:\/|~\/|\.{1,2}\/)/.test(detail)) return "read";
    return !/\s/.test(detail) && /[./]/.test(detail) ? "read" : "search";
  }
  return item.kind === "skill" ? "skill" : item.kind === "edit" ? "edit" : item.kind === "command" ? "command" : item.kind === "agent" ? "agent" : "tool";
}

/** A detail as shown: workspace paths become relative, long ones keep their end. */
export function stepDetail(detail: string | undefined, cwd?: string): string {
  // Rows recorded before native masking are masked here too.
  let value = maskSecrets((detail ?? "").trim());
  if (cwd && value.startsWith(`${cwd}/`)) value = value.slice(cwd.length + 1);
  return value.length > 72 ? `…${value.slice(-71)}` : value;
}

type Translate = (key: string, params?: Record<string, string | number>) => string;

/** One step in a sentence: "Leu src/app.ts", or "Lendo src/app.ts…" while it runs. */
export function stepSentence(item: ActivityItem, t: Translate, cwd?: string): string {
  const category = stepCategory(item);
  const detail = stepDetail(item.detail, cwd);
  const live = item.state === "running";
  if (item.finishedTask) return markerSentence(item, t);
  if (category === "skill") return t("timeline.done.skill");
  if (category === "agent") return item.label || t("Subagent");
  if (isSecretRow(item)) return t("timeline.secretProvided", { label: item.detail ?? "" });
  if (!detail) return t(`timeline.${live ? "plainLive" : "plain"}.${category}`);
  return t(`timeline.${live ? "live" : "done"}.${category}`, { detail });
}

/** A background-completion marker (ADR-101): "Subagente Auditoria concluído", or failed or stopped. */
export function markerSentence(item: Pick<ActivityItem, "label" | "state" | "shell">, t: Translate): string {
  const outcome = item.state === "completed" || item.state === "failed" || item.state === "stopped" ? item.state : "ended";
  // The native side names an unlabeled task "Subagent" or "Command"; the sentence then reads without a name.
  const name = item.label.trim() === (item.shell ? "Command" : "Subagent") ? "" : item.label.trim();
  return t(`timeline.marker.${item.shell ? "shell." : ""}${outcome}`, { name }).replace(/\s+/g, " ").trim();
}
