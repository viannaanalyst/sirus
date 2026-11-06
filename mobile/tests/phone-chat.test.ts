// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";





import assert from "node:assert/strict";
import type { DiscoveredModel } from "../src/shared/client/types.ts";





import { composerApproval } from "../src/shared/lib/composer-approval.ts";





import { approvalOptions, chooseExecution, threadSettingsOptions, threadSettingsSummary } from "../src/shared/lib/phone-chat.ts";





import { modelExecutionControls } from "../src/shared/lib/execution-options.ts";






const model = (id: string, fields: Partial<DiscoveredModel> = {}): DiscoveredModel => ({ id, displayName: id, availability: "available", ...fields });

test("approval: remembered per session, Full access by default, never during planning", () => {
  assert.deepEqual(composerApproval("codex", null, undefined, false), { requested: "full", effective: "full" });
  assert.deepEqual(composerApproval("codex", { agent: "codex", execution: { approval: "ask" } }, undefined, false), { requested: "ask", effective: "ask" });
  assert.deepEqual(composerApproval("codex", { agent: "claude", execution: { approval: "ask" } }, undefined, false).requested, "full");
  assert.deepEqual(composerApproval("codex", null, { codex: "auto" }, false).requested, "auto");
  assert.equal(composerApproval("codex", null, undefined, true).effective, "ask");
  assert.equal(composerApproval("cursor", null, { cursor: "full" }, true).effective, "auto");
});

test("approval rows keep the desktop order and mark unsupported modes", () => {
  assert.deepEqual(approvalOptions(["ask", "full"]), [{ mode: "ask", supported: true }, { mode: "auto", supported: false }, { mode: "full", supported: true }]);
});

test("reasoning and speed are saved per model, Cursor presets switch entries", () => {
  const codex = [model("gpt-5", { effortLevels: ["low", "medium", "high"], fastMode: true })];
  assert.deepEqual(chooseExecution("codex", "gpt-5", codex, { fast: true }, { effort: "high" }), { model: "gpt-5", preference: { fast: true, effort: "high" }, save: true });
  assert.equal(chooseExecution("codex", null, codex, {}, { effort: "high" }).save, false);
  const cursor = [model("gpt-5-low", { displayName: "GPT-5 Low" }), model("gpt-5-high", { displayName: "GPT-5 High" }), model("gpt-5-high-fast", { displayName: "GPT-5 High Fast" })];
  assert.deepEqual(chooseExecution("cursor", "gpt-5-low", cursor, {}, { effort: "high" }), { model: "gpt-5-high", preference: {}, save: false });
  assert.equal(chooseExecution("cursor", "gpt-5-high", cursor, {}, { fast: true }).model, "gpt-5-high-fast");
});

test("the summary shows only the options the selected model has", () => {
  const models = [model("gpt-5", { effortLevels: ["low", "medium", "high"], fastMode: true }), model("gpt-plain")];
  assert.deepEqual(threadSettingsOptions(modelExecutionControls("codex", "gpt-5", models), ["ask", "auto", "full"]), { reasoning: true, tier: true, access: true });
  assert.deepEqual(threadSettingsOptions(modelExecutionControls("codex", "gpt-plain", models), ["ask"]), { reasoning: false, tier: false, access: true });
  // Fast is a Codex and Claude adapter option: another provider's flag does not show the row.
  assert.equal(threadSettingsOptions(modelExecutionControls("opencode", "gpt-5", models), []).tier, false);
  assert.deepEqual(threadSettingsOptions(modelExecutionControls("codex", null, models), []), { reasoning: false, tier: false, access: false });
});

test("the summary card names the pending model and notes default or agent switch", () => {
  const settings = { defaultModel: "claude::claude-opus-5-5" };
  const models = [model("claude-opus-5-5", { displayName: "Claude Opus 5.5" })];
  assert.deepEqual(threadSettingsSummary(settings, models, "claude", "claude-opus-5-5", "claude"), { name: "Opus 5.5", note: "default" });
  assert.deepEqual(threadSettingsSummary(settings, models, "claude", "claude-opus-5-5", "codex"), { name: "Opus 5.5", note: "switch" });
  // A new conversation has no agent to switch from; an id missing from the catalog still reads well.
  assert.deepEqual(threadSettingsSummary(settings, [], "claude", "claude-haiku-4-5-20251001", null), { name: "Haiku 4.5", note: null });
  assert.deepEqual(threadSettingsSummary(settings, models, "claude", null, "claude"), { name: null, note: null });
});