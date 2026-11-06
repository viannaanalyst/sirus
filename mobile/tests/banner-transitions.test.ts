// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";
import assert from "node:assert/strict";
import { advance, INITIAL_TRANSITIONS, pushBanners, stillRelevant, type Observed, type Snapshot } from "../src/data/banner-transitions.ts";

const base: Observed = { key: "c1:s1", computerId: "c1", sessionId: "s1", title: "Publicar", projectName: "newlex", projectId: "p1", sessionStatus: "idle", wait: null, request: undefined, preview: null, error: null };
const ask = (id: string): Observed["request"] => ({ requestId: id, generation: "g", turnId: "t", itemId: "i", kind: { type: "command", command: "npm publish", cwd: null, reason: null } });
const snap = (list: Observed[] | null, onScreen: Snapshot["onScreen"] = null, now = 1): Snapshot => ({ computers: new Map([["c1", list]]), onScreen, now });
const waiting = (id: string): Observed => ({ ...base, sessionStatus: "waiting", wait: "approval", request: ask(id) });

test("the first snapshot of a computer fires nothing, even with a waiting conversation", () => {
  const { fire, state } = advance(INITIAL_TRANSITIONS, snap([waiting("r1")]));
  assert.equal(fire.length, 0);
  assert.equal(advance(state, snap([waiting("r1")])).fire.length, 0);
});

test("a permission fires once, with the command, and not again after a reconnect", () => {
  let { state } = advance(INITIAL_TRANSITIONS, snap([base]));
  const first = advance(state, snap([waiting("r1")]));
  assert.equal(first.fire.length, 1);
  assert.equal(first.fire[0]!.kind, "approval");
  assert.equal(first.fire[0]!.message.code, "npm publish");
  state = advance(first.state, snap(null)).state;
  const back = advance(state, snap([waiting("r1")]));
  assert.equal(back.fire.length, 0);
  assert.equal(advance(back.state, snap([waiting("r1")])).fire.length, 0);
});

test("finishing and failing fire only after being active", () => {
  const running = advance(INITIAL_TRANSITIONS, snap([{ ...base, sessionStatus: "running" }])).state;
  const done = advance(running, snap([{ ...base, sessionStatus: "completed", preview: "Pronto." }]));
  assert.deepEqual(done.fire.map((item) => [item.kind, item.message.text]), [["done", "Pronto."]]);
  assert.equal(advance(done.state, snap([{ ...base, sessionStatus: "completed" }])).fire.length, 0);
  const failed = advance(running, snap([{ ...base, sessionStatus: "failed", error: "boom" }]));
  assert.deepEqual(failed.fire.map((item) => [item.kind, item.message.text]), [["failed", "boom"]]);
});

test("the conversation on screen never gets a banner, and a stopped one is silent", () => {
  const running = advance(INITIAL_TRANSITIONS, snap([{ ...base, sessionStatus: "running" }])).state;
  assert.equal(advance(running, snap([{ ...base, sessionStatus: "completed" }], { computerId: "c1", sessionId: "s1" })).fire.length, 0);
  assert.equal(advance(running, snap([{ ...base, sessionStatus: "stopped" }])).fire.length, 0);
});

test("a new conversation fires only when it asks, and a second ask in the same one fires again", () => {
  const seeded = advance(INITIAL_TRANSITIONS, snap([])).state;
  assert.equal(advance(seeded, snap([{ ...base, sessionStatus: "completed" }])).fire.length, 0);
  const asking = advance(seeded, snap([waiting("a")]));
  assert.equal(asking.fire.length, 1);
  assert.equal(advance(asking.state, snap([waiting("b")])).fire.length, 1);
});

test("an ask banner vanishes once answered elsewhere; a finished one stays relevant", () => {
  const seeded = advance(INITIAL_TRANSITIONS, snap([base])).state;
  const [shown] = advance(seeded, snap([waiting("r1")])).fire;
  assert.ok(stillRelevant(shown!, new Map([[base.key, waiting("r1")]])));
  assert.ok(!stillRelevant(shown!, new Map([[base.key, { ...base, sessionStatus: "running" as const }]])));
  assert.ok(!stillRelevant(shown!, new Map()));
  assert.ok(stillRelevant({ ...shown!, kind: "done" }, new Map()));
});

test("pushBanners keeps one banner per id", () => {
  const seeded = advance(INITIAL_TRANSITIONS, snap([base])).state;
  const [shown] = advance(seeded, snap([waiting("r1")])).fire;
  assert.equal(pushBanners([shown!], [shown!]).length, 1);
});
