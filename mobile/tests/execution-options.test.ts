// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import test from "node:test";
import assert from "node:assert/strict";
import { modelExecutionControls, offeredCursorVariant } from "../src/shared/lib/execution-options.ts";

import type { DiscoveredModel } from "../src/shared/client/types.ts";

const models: DiscoveredModel[] = [
  { id: "claude-opus-5-medium", displayName: "Claude Opus 5 1M Medium", availability: "available" },
  { id: "claude-opus-5-high", displayName: "Claude Opus 5 1M", availability: "available" },
  { id: "claude-opus-5-high-fast", displayName: "Claude Opus 5 1M Fast", availability: "available" },
  { id: "claude-opus-5-thinking-high", displayName: "Claude Opus 5 1M Thinking", availability: "available" },
  { id: "claude-opus-5-thinking-high-fast", displayName: "Claude Opus 5 1M Thinking Fast", availability: "available" },
  { id: "claude-opus-5-max", displayName: "Claude Opus 5 1M Max", availability: "unavailable" },
  { id: "claude-sonnet-5-high-fast", displayName: "Claude Sonnet 5 1M Fast", availability: "available" },
];
test("approval controls offer only modes supported by the native provider adapter", async () => {
  const { providerById } = await import("../src/shared/lib/provider-registry.ts");
  for (const provider of ["codex", "claude", "opencode"] as const) {
    assert.deepEqual(Reflect.get(providerById(provider), "approvalModes"), ["ask", "auto", "full"]);
  }
  assert.deepEqual(Reflect.get(providerById("cursor"), "approvalModes"), ["ask", "auto", "full"]);
  assert.deepEqual(Reflect.get(providerById("grok"), "approvalModes"), []);
});
test("Cursor effort and Fast select only offered presets within the same model family", () => {
  const controls = modelExecutionControls("cursor", "claude-opus-5-thinking-high", models);
  assert.deepEqual(controls.levels, ["medium", "high"]);
  assert.equal(controls.fastAvailable, true);
  const fast = modelExecutionControls("cursor", "claude-opus-5-thinking-high-fast", models);
  assert.deepEqual(fast.levels, controls.levels);
  assert.equal(fast.effort, controls.effort);
  assert.deepEqual(fast.selectableLevels, ["high"]);
  assert.equal(offeredCursorVariant(controls.variants, "claude-opus-5-thinking-high", "high", true), "claude-opus-5-thinking-high-fast");
  assert.equal(offeredCursorVariant(controls.variants, "claude-opus-5-high", "medium", true), null);
  assert.equal(offeredCursorVariant(controls.variants, "claude-opus-5-high", "max", false), null);
  assert.equal(modelExecutionControls("cursor", "claude-sonnet-5-high-fast", models).fastAvailable, false);
});

test("Cursor parameter catalog offers Composer Fast independently of automatic reasoning", () => {
  const catalog: DiscoveredModel[]=[{id:"composer-2.5",displayName:"Composer 2.5",availability:"available",parameterized:true,effortLevels:[],fastMode:true,defaultFast:true}];
  const controls=modelExecutionControls("cursor","composer-2.5",catalog);
  assert.equal(controls.fastAvailable,true);assert.equal(controls.fast,true);assert.equal(controls.effort,null);assert.deepEqual(controls.levels,[]);assert.equal(controls.parameterized,true);
  assert.equal(modelExecutionControls("cursor","composer-2.5",catalog,{fast:false}).fast,false);
});
test("OpenCode and Grok use their discovered effort levels without inventing priority service", () => {
  const catalog:DiscoveredModel[]=[{id:"m",displayName:"M",availability:"available",effortLevels:["low","high"],defaultEffort:"high",fastMode:true}];
  for(const provider of ["opencode","grok"] as const){ const c=modelExecutionControls(provider,"m",catalog,{effort:"low",fast:true});assert.equal(c.effort,"low");assert.equal(c.fastAvailable,false);assert.equal(c.fast,false); }
});