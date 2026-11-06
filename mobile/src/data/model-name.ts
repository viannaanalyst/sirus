/** Brands whose capitalisation is not a plain "Title case". */
const BRAND: Record<string, string> = {
  deepseek: "DeepSeek", gpt: "GPT", glm: "GLM", gemini: "Gemini", grok: "Grok", qwen: "Qwen", kimi: "Kimi", llama: "Llama", mistral: "Mistral",
  codex: "Codex", opencode: "OpenCode", openai: "OpenAI", mini: "Mini", nano: "Nano", flash: "Flash", pro: "Pro", max: "Max", lite: "Lite",
};

/**
 * A model id as people read it: "claude-opus-5-5" → "Opus 5.5", "claude-haiku-4-5-20251001" →
 * "Haiku 4.5", "deepseek-v4.1-flash" → "DeepSeek V4.1 Flash", "gpt-5.4-mini" → "GPT-5.4 Mini".
 * An id it does not know comes out as words in Title case, never empty or mangled.
 */
export function modelDisplayName(model: string | null | undefined): string {
  const raw = (model ?? "").trim();
  if (!raw) return "";
  const id = raw.replace(/^.*\//, "").replace(/-(?:\d{8}|latest)$/i, "").replace(/^claude-/i, "");
  const tokens = id.split(/[-_\s]+/).filter(Boolean);
  // "5", "5" → "5.5": the version a dash split in two.
  const merged: string[] = [];
  for (const token of tokens) {
    const last = merged[merged.length - 1];
    if (last !== undefined && /^v?\d+(?:\.\d+)*$/i.test(last) && /^\d{1,2}$/.test(token) && /^\d{1,2}$/.test(last.replace(/^v/i, "").split(".").pop() ?? "")) merged[merged.length - 1] = `${last}.${token}`;
    else merged.push(token);
  }
  const words = merged.map((token) => {
    const lower = token.toLowerCase();
    if (BRAND[lower]) return BRAND[lower];
    if (/^v\d/i.test(token)) return `V${token.slice(1)}`;
    if (/^[\d.]+$/.test(token) || token === token.toUpperCase()) return token;
    return token.charAt(0).toUpperCase() + token.slice(1);
  });
  // GPT-5.5 keeps its dash; everything else reads as spaced words.
  return words[0] === "GPT" && words[1] && /^\d/.test(words[1]) ? [`GPT-${words[1]}`, ...words.slice(2)].join(" ") : words.join(" ");
}
