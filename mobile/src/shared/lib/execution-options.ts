import type { AgentProviderId, DiscoveredModel, ExecutionOptions } from "../client/types";
import { modelDisplayName } from "./model-registry";

export const EFFORT_ORDER = ["none", "minimal", "low", "medium", "high", "xhigh", "max", "ultra"] as const;

export function cursorPreset(model: DiscoveredModel) {
  // Only actual catalog IDs are selected. The suffix identifies an offered CLI preset.
  const match = /-(none|minimal|low|medium|high|xhigh|max)(?:-thinking)?(?:-fast)?$/.exec(model.id);
  return { effort: match?.[1] ?? null, fast: /-fast$/.test(model.id) };
}

export function modelExecutionControls(provider: AgentProviderId, modelId: string | null, models: DiscoveredModel[], preference: ExecutionOptions = {}) {
  const current = models.find((model) => model.id === modelId);
  if (provider === "cursor" && current && !current.parameterized) {
    const names = models.map((model) => model.displayName);
    const name = modelDisplayName(provider, current.displayName, names);
    const variants = models.filter((model) => model.availability === "available" && modelDisplayName(provider, model.displayName, names) === name);
    const preset = cursorPreset(current);
    const levels: string[] = EFFORT_ORDER.filter((effort) => variants.some((model) => cursorPreset(model).effort === effort));
    const selectableLevels = levels.filter((effort) => variants.some((model) => cursorPreset(model).effort === effort && cursorPreset(model).fast === preset.fast));
    const fastAvailable = variants.some((model) => cursorPreset(model).effort === preset.effort && cursorPreset(model).fast !== preset.fast);
    return { levels, selectableLevels, effort: preset.effort, fast: preset.fast, fastAvailable, variants, parameterized: false };
  }
  const adapterOptions = provider !== "cursor" || Boolean(current?.parameterized);
  const fastAdapter = provider === "codex" || provider === "claude" || (provider === "cursor" && current?.parameterized);
  const levels: string[] = adapterOptions ? EFFORT_ORDER.filter((level) => current?.effortLevels?.includes(level)) : [];
  return { levels, selectableLevels: levels, effort: preference.effort && levels.includes(preference.effort) ? preference.effort : current?.defaultEffort && levels.includes(current.defaultEffort) ? current.defaultEffort : levels[Math.floor(levels.length / 2)] ?? null,
    fast: Boolean(fastAdapter && (preference.fast ?? current?.defaultFast) && current?.fastMode), fastAvailable: Boolean(fastAdapter && current?.fastMode), variants: [], parameterized: Boolean(current?.parameterized) };
}

export function offeredCursorVariant(variants: DiscoveredModel[], currentId: string | null, effort: string | null, fast: boolean) {
  const thinking = currentId?.includes("thinking") ?? false;
  return variants.filter((model) => { const preset = cursorPreset(model); return preset.effort === effort && preset.fast === fast; })
    .sort((a, b) => Number(b.id.includes("thinking") === thinking) - Number(a.id.includes("thinking") === thinking) || a.id.localeCompare(b.id))[0]?.id ?? null;
}
