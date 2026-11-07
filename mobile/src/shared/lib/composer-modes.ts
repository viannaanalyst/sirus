import type { AgentProviderId, ExecutionOptions } from "../client/types";

/** The composer's modes, from its + menu (the Mac's add-menu): goal, planning, debug and team. */
export interface ComposerModes { goal: string; planning: boolean; debugging: boolean; team: boolean }
export const NO_MODES: ComposerModes = { goal: "", planning: false, debugging: false, team: false };

/** Extra fields of `send_prompt` the modes add (the Mac's `send_now`). */
export interface PromptExtras { goal?: string | null; debugging?: boolean; team?: boolean }

/** Whether the provider (and model) can plan before editing; Codex needs a chosen model. */
export function supportsPlanning(provider: AgentProviderId, model: string | null): boolean {
  return provider === "claude" || provider === "cursor" || provider === "opencode" || provider === "pi" || provider === "droid" || provider === "devin" || provider === "hermes" || provider === "antigravity" || (provider === "codex" && Boolean(model));
}

/** Whether the provider can run a team of agents. */
export function supportsTeam(provider: AgentProviderId): boolean {
  return provider === "claude" || provider === "codex" || provider === "opencode";
}

/** Planning never runs with debugging; turning it on drops debugging. */
export function setPlanning(modes: ComposerModes, on: boolean): ComposerModes {
  return { ...modes, planning: on, debugging: on ? false : modes.debugging };
}
/** Debugging replaces planning and team. */
export function setDebugging(modes: ComposerModes, on: boolean): ComposerModes {
  return { ...modes, debugging: on, planning: on ? false : modes.planning, team: on ? false : modes.team };
}
/** A team request plans read-only by itself, so it replaces planning and debugging. */
export function setTeam(modes: ComposerModes, on: boolean): ComposerModes {
  return { ...modes, team: on, planning: on ? false : modes.planning, debugging: on ? false : modes.debugging };
}
export function setGoal(modes: ComposerModes, goal: string): ComposerModes {
  return { ...modes, goal: goal.trim().slice(0, 400) };
}

/** Modes the provider cannot run are dropped before sending (a provider switch can leave them on). */
export function effectiveModes(modes: ComposerModes, provider: AgentProviderId, model: string | null): ComposerModes {
  return {
    ...modes,
    planning: modes.planning && supportsPlanning(provider, model),
    team: modes.team && supportsTeam(provider),
  };
}

/** The `execution` of a send: planning on, and Full access stepped down while planning (Cursor reviews automatically). */
export function executionWithModes(execution: ExecutionOptions, modes: ComposerModes, provider: AgentProviderId, model: string | null): ExecutionOptions {
  const planning = effectiveModes(modes, provider, model).planning;
  const approval = planning && execution.approval === "full" ? provider === "cursor" ? "auto" : "ask" : execution.approval;
  return { ...execution, planning, approval };
}

/** What `send_prompt` carries besides the prompt: the goal, debugging (never with planning) and team (one send). */
export function promptExtras(modes: ComposerModes, provider: AgentProviderId, model: string | null): PromptExtras {
  const effective = effectiveModes(modes, provider, model);
  return {
    goal: effective.goal || null,
    debugging: effective.debugging && !effective.planning,
    ...(effective.team ? { team: true } : {}),
  };
}

/** The modes after a send: a team request is spent (one-shot), the rest stay for the next message. */
export function afterSend(modes: ComposerModes): ComposerModes {
  return { ...modes, team: false };
}

/** The chips above the field, in the Mac's order and words. */
export function modeChips(modes: ComposerModes): { id: "planning" | "debugging" | "team" | "goal"; label: string; icon: string }[] {
  return [
    ...(modes.planning ? [{ id: "planning" as const, label: "Planejamento", icon: "lightbulb" }] : []),
    ...(modes.debugging ? [{ id: "debugging" as const, label: "Debug", icon: "ladybug" }] : []),
    ...(modes.team ? [{ id: "team" as const, label: "Equipe", icon: "person.2" }] : []),
    ...(modes.goal ? [{ id: "goal" as const, label: modes.goal, icon: "scope" }] : []),
  ];
}
