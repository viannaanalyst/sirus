import type { ActivityItem, ActivityState } from "@sirus/client/types";
import { translate } from "@sirus/i18n";
import { stepCategory, stepDetail, stepSentence, type StepCategory } from "@sirus/lib/turn-timeline";
import { formatActivityDuration } from "@sirus/lib/agent-activity";

export const t = (key: string, params?: Record<string, string | number>) => translate("pt-BR", key, params);

/** A tree row as it reads: an icon, the Mac's verb and what it acted on (`step_lines.rs`). */
export interface StepLine { readonly icon: string; readonly verb: string; readonly target: string | null; readonly category: StepCategory }

const CATEGORY_ICON: Record<StepCategory, string> = {
  skill: "sparkles", edit: "pencil", command: "terminal", read: "doc.text", search: "magnifyingglass", web: "globe", tool: "wrench.and.screwdriver", agent: "person.2",
};

/** Browser and computer tools: icon and verb (the Mac's `known_tool`). */
const KNOWN_TOOL: Record<string, readonly [string, string]> = {
  computer_launch: ["macwindow", "Abriu"], computer_apps: ["macwindow", "Listou os apps"], computer_focus: ["macwindow", "Trouxe para a frente"],
  computer_observe: ["eye", "Observou"], computer_click: ["cursorarrow.click", "Clicou"], browser_click: ["cursorarrow.click", "Clicou"],
  computer_type: ["keyboard", "Digitou"], browser_type: ["keyboard", "Digitou"], computer_key: ["keyboard", "Pressionou uma tecla"], browser_press: ["keyboard", "Pressionou uma tecla"],
  computer_scroll: ["cursorarrow.click", "Rolou"], browser_scroll: ["cursorarrow.click", "Rolou"], computer_screenshot: ["eye", "Captura da tela"], browser_screenshot: ["eye", "Captura da tela"],
  browser_open: ["globe", "Abriu"], browser_navigate: ["globe", "Abriu"], browser_back: ["globe", "Voltou"], browser_forward: ["globe", "Avançou"],
  browser_reload: ["globe", "Recarregou"], browser_close: ["globe", "Fechou o navegador"], browser_snapshot: ["eye", "Leu a página"], browser_wait_for: ["globe", "Esperou a página"],
};

export function fileName(path: string): string {
  return path.split("/").filter(Boolean).at(-1) ?? path;
}

/** The host may send the MCP tool's name and what it acted on beside the shared fields. */
function extras(item: object): { tool?: string; target?: string } {
  return item as { tool?: string; target?: string };
}

export function stepLine(item: Pick<ActivityItem, "kind" | "label" | "detail">, cwd?: string): StepLine {
  const category = stepCategory(item);
  const detail = (item.detail ?? "").trim();
  const { tool, target } = extras(item);
  const known = KNOWN_TOOL[tool ?? detail];
  if (known) return { icon: known[0], verb: known[1], target: target?.trim() || null, category };
  // Claude loading a tool's schema is housekeeping, and its query an internal name.
  if (detail.startsWith("select:")) return { icon: "wrench.and.screwdriver", verb: "Carregou ferramentas", target: null, category };
  const shown = stepDetail(detail, cwd);
  switch (item.label) {
    case "Read / search": return category === "read"
      ? { icon: CATEGORY_ICON.read, verb: "Leu", target: fileName(detail), category }
      : { icon: CATEGORY_ICON.search, verb: "Buscou", target: shown || null, category };
    case "File changes": return { icon: CATEGORY_ICON.edit, verb: "Editou", target: detail ? fileName(detail) : null, category };
    case "Command": return { icon: CATEGORY_ICON.command, verb: "Executou", target: shown || null, category };
    case "Web search": return { icon: CATEGORY_ICON.web, verb: "Buscou", target: shown || null, category };
    case "Skill": return { icon: CATEGORY_ICON.skill, verb: "Usou a skill", target: shown || null, category };
    case "Delegate task": return { icon: CATEGORY_ICON.agent, verb: "Delegou", target: shown || null, category };
    case "Tool call": case "": return { icon: CATEGORY_ICON.tool, verb: "Usou", target: shown || null, category };
    // Anything else keeps the shared sentence: the label already says it.
    default: return { icon: CATEGORY_ICON[category], verb: "", target: stepSentence(item as ActivityItem, t, cwd), category };
  }
}

/** A step's seconds when both ends are known, ticking while it runs. */
export function stepSeconds(item: Pick<ActivityItem, "startedAt" | "endedAt" | "state">, now: number): number | null {
  if (item.startedAt == null) return null;
  const end = item.endedAt ?? (item.state === "running" ? now : null);
  return end == null ? null : Math.max(0, Math.floor((end - item.startedAt) / 1000));
}

/** "4 s" / "1m 2s", the tree's right-hand time; hidden under a second. */
export function shortElapsed(seconds: number | null): string | null {
  if (seconds == null || seconds < 1) return null;
  return seconds < 60 ? `${seconds} s` : formatActivityDuration(seconds);
}

/** A model id as the Mac names it ("claude-opus-4-5" → "Opus 4.5", "gpt-5.5" → "GPT-5.5"). */
export function modelLabel(model: string | null | undefined, provider: string): string {
  const raw = model?.trim();
  if (!raw) return ({ claude: "Claude", codex: "Codex", opencode: "OpenCode" } as Record<string, string>)[provider] ?? "Agente";
  const trimmed = raw.replace(/^claude-/, "");
  if (trimmed.startsWith("gpt")) return `GPT${trimmed.slice(3)}`;
  const parts = trimmed.split("-");
  const name = parts[0] ?? trimmed;
  const version = parts.slice(1).filter((part) => part.length <= 2 && /^\d+$/.test(part));
  const title = name.charAt(0).toUpperCase() + name.slice(1);
  return version.length ? `${title} ${version.join(".")}` : title;
}

export const STATE_COLOR: Record<ActivityState, string> = { running: "#7fb4ff", completed: "#5fce8c", failed: "#ee8a96", stopped: "#a4a4ae", unknown: "#a4a4ae" };
