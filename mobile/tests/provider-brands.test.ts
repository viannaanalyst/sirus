// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import assert from "node:assert/strict";
import test from "node:test";
import { resolveModelBrand } from "../src/shared/lib/model-brand-resolver.ts";


test("routed models retain their upstream brand across coding CLIs", () => {
  const models = [
    ["opencode-go/deepseek-v4.1-flash", "deepseek"],
    ["moonshotai/kimi-k3", "kimi"], ["opencode-go/glm-5.3", "zai"],
    ["z-ai/glm-5.2", "zai"], ["opencode-go/qwen3.8-max", "qwen"],
    ["opencode-go/minimax-m3", "minimax"], ["mistral/devstral", "mistral"],
    ["opencode-go/muse-spark-1.3-contributor", "meta"],
    ["opencode/nemotron-3-ultra-free", "nvidia"], ["opencode-go/hy4-preview", "hunyuan"],
    ["opencode-go/mimo-v2.6-pro", "mimo"], ["opencode-go/longcat-2.5", "longcat"],
    ["openrouter/anthropic/claude-opus-5", "claude"], ["opencode/gpt-6-sol", "openai"],
    ["opencode/gpt-5.3-codex", "codex"], ["google/gemini-3.8-flash", "gemini"],
    ["opencode-go/grok-4.7", "grok"], ["cohere/command-r-plus", "cohere"],
  ] as const;
  for (const provider of ["opencode", "pi", "droid", "devin"] as const) {
    for (const [id, brand] of models) assert.equal(resolveModelBrand(id, provider), brand, `${provider}:${id}`);
  }
  assert.equal(resolveModelBrand("opencode/space-bunny-free", "opencode"), "opencode");
  assert.equal(resolveModelBrand("opencode/absolute-custom-model", "opencode"), "opencode");
});