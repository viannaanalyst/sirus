import type { ActivityItem, ActivityState, SubagentFeedEntry } from "../client/types";

/** How a subagent reads on the team card and in its live view (ADR-146). */
export type MemberStatus = "running" | "completed" | "failed" | "stopped" | "unknown";

export interface TeamMember {
  /** The child row's id (`agent:<task or thread>`). */
  id: string;
  /** Its feed: the call that launched it (Claude), else the row itself (Codex). */
  key: string;
  label: string;
  model: string | null;
  status: MemberStatus;
  startedAt: number | null;
  endedAt: number | null;
  /** Tools it used: the steps it reported, kept or folded away. */
  tools: number;
  background: boolean;
  /** A Claude workflow: its members settled of all (ADR-115). */
  workflow?: { done: number; total: number };
}

export interface TeamPhase { index: number; members: TeamMember[] }

/** A subagent row of a turn: launched work, not a background shell or a "finished" marker. */
export function isSubagent(item: ActivityItem): boolean {
  return item.kind === "agent" && !item.shell && !item.finishedTask;
}

/** Where a child's own conversation is found (`subagent_feed`). */
export function feedKey(item: Pick<ActivityItem, "id" | "source">): string {
  return item.source || item.id;
}

/** A background child the turn stopped reads as stopped; an unreported one stays unknown. */
export function memberStatus(item: Pick<ActivityItem, "state" | "timedOut">): MemberStatus {
  const state: ActivityState = item.state;
  if (item.timedOut) return "stopped";
  return state;
}

export function teamMember(item: ActivityItem): TeamMember {
  const agents = item.workflow?.agents;
  return {
    id: item.id,
    key: feedKey(item),
    label: item.label.trim(),
    model: item.model,
    status: memberStatus(item),
    startedAt: item.startedAt ?? null,
    endedAt: item.endedAt ?? null,
    tools: agents ? agents.reduce((sum, agent) => sum + (agent.toolCalls ?? 0), 0) : (item.steps?.length ?? 0) + (item.hiddenSteps ?? 0),
    background: Boolean(item.background),
    ...(agents ? { workflow: { done: agents.filter((agent) => agent.state === "completed" || agent.state === "failed").length, total: agents.length } } : {}),
  };
}

/**
 * The turn's subagents in phases: subagents launched back to back (no other work between
 * them, at the same place in the reply) are one phase; anything else in between starts the
 * next. Finished markers and background shells neither join nor split a phase.
 */
export function teamPhases(items: readonly ActivityItem[]): TeamPhase[] {
  const phases: TeamPhase[] = [];
  let open: { phase: TeamPhase; offset: number | undefined } | null = null;
  for (const item of items) {
    if (item.finishedTask || (item.kind === "agent" && item.shell)) continue;
    if (!isSubagent(item)) { open = null; continue; }
    if (!open || (item.offset !== undefined && open.offset !== undefined && item.offset !== open.offset)) {
      const phase: TeamPhase = { index: phases.length + 1, members: [] };
      phases.push(phase);
      open = { phase, offset: item.offset };
    }
    open.phase.members.push(teamMember(item));
  }
  return phases;
}

/** Where the turn's first subagent began in the reply (UTF-16 offset), if it was placed. */
export function firstLaunchOffset(items: readonly ActivityItem[]): number | undefined {
  return items.find(isSubagent)?.offset;
}

/** A team card shows for a turn that started two subagents or more. */
export function teamSize(items: readonly ActivityItem[]): number {
  return items.reduce((count, item) => count + (isSubagent(item) ? 1 : 0), 0);
}

/** Seconds a member has worked: its native window, still open while it runs. */
export function memberElapsed(member: Pick<TeamMember, "startedAt" | "endedAt">, now: number): number | null {
  if (member.startedAt == null) return null;
  return Math.max(0, Math.floor(((member.endedAt ?? now) - member.startedAt) / 1000));
}

/** Settled members of all, and whether any still runs. */
export function teamProgress(phases: readonly TeamPhase[]): { done: number; total: number; running: number; failed: number } {
  const members = phases.flatMap((phase) => phase.members);
  return {
    done: members.filter((member) => member.status !== "running").length,
    total: members.length,
    running: members.filter((member) => member.status === "running").length,
    failed: members.filter((member) => member.status === "failed").length,
  };
}

const MAX_HEADLINE = 120;

function plainLine(line: string): string {
  return line.replace(/^#{1,6}\s+/, "").replace(/^[-*>]\s+/, "").replace(/[*_`]+/g, "").replace(/\[([^\]]*)\]\([^)]*\)/g, "$1").trim();
}

function clip(text: string): string {
  return text.length > MAX_HEADLINE ? `${text.slice(0, MAX_HEADLINE - 1).trimEnd()}…` : text;
}

/**
 * The card's headline: the title of a plan the agent wrote before delegating (its first
 * Markdown heading up to the first launch), else the first line of the person's prompt.
 */
export function teamHeadline(prompt: string, reply: string, firstOffset: number | undefined): string {
  const before = reply.slice(0, Math.max(0, firstOffset ?? reply.length));
  const heading = before.split("\n").find((line) => /^#{1,3}\s+\S/.test(line.trim()));
  if (heading) {
    const title = plainLine(heading.trim());
    if (title) return clip(title);
  }
  const line = prompt.split("\n").map(plainLine).find(Boolean);
  return line ? clip(line) : "";
}

/**
 * New feed entries merged by id: a known entry is replaced in place (an entry that did not
 * change keeps its identity, so its row does not render again); new ones go at the end.
 * At most `limit` are kept, the oldest dropped first.
 */
export function mergeFeedEntries(known: readonly SubagentFeedEntry[], incoming: readonly SubagentFeedEntry[], limit = 300): { entries: SubagentFeedEntry[]; dropped: number } {
  if (!incoming.length) return { entries: known as SubagentFeedEntry[], dropped: 0 };
  const index = new Map(known.map((entry, at) => [entry.id, at]));
  const entries = [...known];
  for (const entry of incoming) {
    const at = index.get(entry.id);
    if (at === undefined) { index.set(entry.id, entries.length); entries.push(entry); }
    else if (!sameEntry(entries[at], entry)) entries[at] = entry;
  }
  const dropped = Math.max(0, entries.length - limit);
  return { entries: dropped ? entries.slice(dropped) : entries, dropped };
}

function sameEntry(a: SubagentFeedEntry, b: SubagentFeedEntry): boolean {
  return a.kind === b.kind && a.text === b.text && a.label === b.label && a.detail === b.detail && a.output === b.output && a.state === b.state && a.tool === b.tool;
}
