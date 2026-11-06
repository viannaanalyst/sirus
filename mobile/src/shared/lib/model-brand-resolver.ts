import type { AgentProviderId } from "../client/types";

export type ModelBrand = AgentProviderId
  | "claude"
  | "openai"
  | "codex"
  | "gemini"
  | "grok"
  | "cursor"
  | "opencode"
  | "deepseek"
  | "kimi"
  | "zai"
  | "qwen"
  | "minimax"
  | "mistral"
  | "meta"
  | "nvidia"
  | "hunyuan"
  | "mimo"
  | "longcat"
  | "cohere"
  | "unknown";

export function resolveModelBrand(modelId: string, provider: AgentProviderId): ModelBrand {
  const id = modelId.toLowerCase();
  // Match the upstream family before the routing provider (OpenRouter, OpenCode,
  // Pi, Droid, etc.). Transport prefixes never replace the model's own mark.
  if (id.includes("deepseek")) return "deepseek";
  if (id.includes("kimi") || id.includes("moonshot")) return "kimi";
  if (/(^|[/:_-])(glm|chatglm)([\d._-]|$)/.test(id) || id.includes("z-ai/") || id.includes("zai/")) return "zai";
  if (id.includes("qwen") || id.includes("qwq")) return "qwen";
  if (id.includes("minimax")) return "minimax";
  if (id.includes("mistral") || id.includes("codestral") || id.includes("devstral") || id.includes("ministral")) return "mistral";
  if (id.includes("llama") || id.includes("muse-spark")) return "meta";
  if (id.includes("nemotron") || id.includes("nvidia/")) return "nvidia";
  if (id.includes("hunyuan") || /(^|\/)hy\d/.test(id)) return "hunyuan";
  if (/(^|[/:_-])mimo([\d._-]|$)/.test(id)) return "mimo";
  if (id.includes("longcat")) return "longcat";
  if (id.includes("cohere") || /(^|\/)command-r([+-]|$)/.test(id)) return "cohere";
  if (id.includes("claude") || id.includes("anthropic") || /(^|[/-])(sonnet|opus|haiku|fable)([/-]|$)/.test(id)) {
    return "claude";
  }
  if (id.includes("gemini") || id.includes("google")) return "gemini";
  if (id.includes("grok")) return "grok";
  if (id.includes("composer")) return "cursor";
  if (id.includes("codex")) return "codex";
  if (id.includes("gpt") || id.includes("openai") || /(^|\/)(o[134]|luna|sol)([-.]|$)/.test(id)) {
    return "openai";
  }
  if (provider === "claude") return "claude";
  if (provider === "codex") return "codex";
  if (provider === "cursor") return "cursor";
  if (provider === "grok") return "grok";
  if (provider === "opencode") return "opencode";
  return provider;
}
