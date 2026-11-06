// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import test from "node:test";
import assert from "node:assert/strict";
import { modelDisplayName } from "../src/shared/lib/model-registry.ts";

import type { ProviderModelList } from "../src/shared/client/types.ts";

import { AGENT_PROVIDER_IDS } from "../src/shared/client/types.ts";


test("model presentation omits provider/router namespaces for every provider without changing native IDs", () => {
  for (const provider of AGENT_PROVIDER_IDS) {
    const id = `${provider}::openrouter/deepseek/deepseek-v4.1-flash`;
    assert.equal(modelDisplayName(provider, id), "deepseek-v4.1-flash");
    assert.equal(id, `${provider}::openrouter/deepseek/deepseek-v4.1-flash`);
    assert.equal(modelDisplayName(provider, "DeepSeek V4.1 Flash"), "DeepSeek V4.1 Flash");
  }
  assert.equal(modelDisplayName("opencode", "opencode-go/deepseek-v4.1-flash"), "deepseek-v4.1-flash");
  assert.equal(modelDisplayName("cursor", "cursor/Claude Sonnet 5.5 Max", ["cursor/Claude Sonnet 5.5 Low", "cursor/Claude Sonnet 5.5 Max"]), "Claude Sonnet 5.5");
});

const cursorCatalog: ProviderModelList = { provider: "cursor", source: "cli", note: "", models: [
  { id: "claude-sonnet-5-thinking-xhigh", displayName: "Claude Sonnet 5 1M Extra High Thinking", availability: "available" },
  { id: "claude-sonnet-5-high", displayName: "Claude Sonnet 5 1M", availability: "available" },
  { id: "claude-sonnet-5-medium", displayName: "Claude Sonnet 5 1M Medium", availability: "available" },
  { id: "claude-4.6-sonnet-medium", displayName: "Claude Sonnet 4.6 1M", availability: "available" },
  { id: "gpt-5.4-mini-medium", displayName: "GPT-5.4 Mini", availability: "available" },
  { id: "gpt-5.6-sol-high", displayName: "GPT-5.6 Sol 1M High", availability: "unavailable" },
] };

test("presentation removes only Cursor preset suffixes and preserves model families and versions", () => {
  for (const name of ["GPT-5.4 Mini", "Gemini 3.8 Flash", "GPT-5.6 Sol", "Kimi K3 Max", "Claude Sonnet 4.6"]) assert.equal(modelDisplayName("cursor", name), name);
  assert.equal(modelDisplayName("cursor", "Grok 4.7\u200B Extra High Fast"), "Grok 4.7");
  assert.equal(modelDisplayName("cursor", "Claude Fable 5 1M Thinking (NO ZDR)"), "Claude Fable 5");
  assert.equal(modelDisplayName("cursor", "Auto (default)"), "Auto");
  assert.equal(modelDisplayName("cursor", "Claude Sonnet 5 1M Max Thinking"), "Claude Sonnet 5");
  assert.equal(modelDisplayName("cursor", "Claude Sonnet 5.5  Max", ["Claude Sonnet 5.5 Low", "Claude Sonnet 5.5 Max"]), "Claude Sonnet 5.5");
  assert.equal(modelDisplayName("cursor", "Gemini 3.6 Flash Minimal"), "Gemini 3.6 Flash");
  assert.equal(modelDisplayName("codex", "GPT Thinking"), "GPT Thinking");
});

test("native model ids read like the picker", async () => {
  const { readableModelId } = await import("../src/shared/lib/model-registry.ts");
  assert.equal(readableModelId("claude-haiku-4-5-20251001"), "Haiku 4.5");
  assert.equal(readableModelId("claude-opus-5"), "Opus 5");
  assert.equal(readableModelId("gpt-6-luna"), "gpt-6-luna");
  assert.equal(readableModelId("some-model-20260101"), "some-model");
});