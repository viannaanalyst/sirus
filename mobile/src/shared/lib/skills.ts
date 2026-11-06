import type { AgentSkill } from "../client/types";

/** Skills whose name, description or source contains every word of the query (enabled ones only when asked). */
export function filterSkills(skills: readonly AgentSkill[], query: string, disabled: readonly string[] = [], enabledOnly = false): AgentSkill[] {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  return skills.filter((skill) => (!enabledOnly || !disabled.includes(skill.name)) && words.every((word) =>
    `${skill.name} ${skill.description} ${skill.sources.map((source) => `${source.origin} ${source.scope} ${source.path}`).join(" ")}`.toLowerCase().includes(word)));
}

/** Adds `/name` at the start of the draft, once (the Mac's `use_skill`). */
export function insertSkillInvocation(draft: string, name: string): string {
  const leading = draft.trimStart().split(/\s+/).filter(Boolean);
  const boundary = leading.findIndex((token) => !token.startsWith("/"));
  if (leading.slice(0, boundary < 0 ? leading.length : boundary).some((token) => token.toLowerCase() === `/${name}`)) return draft;
  return `/${name} ${draft}`;
}
