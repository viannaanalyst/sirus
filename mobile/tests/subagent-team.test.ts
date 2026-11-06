// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import assert from "node:assert/strict";
import { test } from "node:test";

import type { ActivityItem, SubagentFeedEntry } from "../src/shared/client/types.ts";

import { feedKey, firstLaunchOffset, memberElapsed, memberStatus, mergeFeedEntries, teamHeadline, teamPhases, teamProgress, teamSize } from "../src/shared/lib/subagent-team.ts";


const child = (id: string, offset: number, extra: Partial<ActivityItem> = {}): ActivityItem => ({ id: `agent:${id}`, kind: "agent", label: `Audit ${id}`, state: "running", model: null, offset, startedAt: 1000, endedAt: null, source: `toolu_${id}`, ...extra });
const step = (id: string, offset: number): ActivityItem => ({ id, kind: "read", label: "Read / search", state: "completed", model: null, offset });

test("subagents launched back to back are one phase; other work or a later place starts the next", () => {
  const items = [step("r0", 0), child("a", 10), child("b", 10), step("r1", 10), child("c", 10), child("d", 42, { state: "completed", endedAt: 4000 })];
  const phases = teamPhases(items);
  assert.deepEqual(phases.map((phase) => phase.members.map((member) => member.id)), [["agent:a", "agent:b"], ["agent:c"], ["agent:d"]]);
  assert.deepEqual(phases.map((phase) => phase.index), [1, 2, 3]);
  assert.equal(teamSize(items), 4);
  assert.equal(firstLaunchOffset(items), 10);
});

test("finished markers and background shells neither join nor split a phase", () => {
  const marker: ActivityItem = { id: "done:a", kind: "tool", label: "Audit a", state: "completed", model: null, finishedTask: true, offset: 10 };
  const shell: ActivityItem = { ...child("sh", 10), shell: true, background: true };
  const phases = teamPhases([child("a", 10), marker, shell, child("b", 10)]);
  assert.equal(phases.length, 1);
  assert.equal(phases[0].members.length, 2);
  assert.equal(teamSize([child("a", 1), shell, marker]), 1, "a shell or a marker is not a subagent");
});

test("members carry their feed key, tools used, status and elapsed time", () => {
  const [phase] = teamPhases([
    child("a", 0, { steps: [{ id: "s1", kind: "read", label: "Read / search", state: "completed" }], hiddenSteps: 4 }),
    { ...child("b", 0), source: undefined, state: "stopped", timedOut: true, background: true, endedAt: 61_000 },
    { ...child("w", 0), workflow: { phases: [], agents: [{ index: 1, label: "x", state: "completed", toolCalls: 3 }, { index: 2, label: "y", state: "running", toolCalls: 2 }] } },
  ]);
  const [a, b, w] = phase.members;
  assert.equal(a.key, "toolu_a", "Claude: the call that launched it");
  assert.equal(b.key, "agent:b", "Codex: the child row itself");
  assert.equal(a.tools, 5, "kept and folded steps");
  assert.equal(w.tools, 5);
  assert.deepEqual(w.workflow, { done: 1, total: 2 });
  assert.equal(b.status, "stopped");
  assert.equal(b.background, true);
  assert.equal(memberElapsed(a, 31_500), 30, "a running member ticks to now");
  assert.equal(memberElapsed(b, 999_999), 60, "a settled member keeps its window");
  assert.equal(memberElapsed({ startedAt: null, endedAt: null }, 5000), null);
  assert.deepEqual(teamProgress([phase]), { done: 1, total: 3, running: 2, failed: 0 });
  assert.equal(memberStatus({ state: "unknown" }), "unknown");
  assert.equal(feedKey({ id: "agent:x", source: "" }), "agent:x");
});

test("the headline is a plan title written before the launch, else the prompt's first line", () => {
  assert.equal(teamHeadline("Review the **parser**\nand more", "", undefined), "Review the parser");
  assert.equal(teamHeadline("Do it", "Plan:\n## Split the audit\n- one", 30), "Split the audit");
  assert.equal(teamHeadline("Do it", "Starting.\n\n# Later heading", 9), "Do it", "a heading after the launch is not the plan");
  assert.equal(teamHeadline("\n\n", "", undefined), "");
  const long = teamHeadline("x".repeat(300), "", undefined);
  assert.equal(long.length, 120);
  assert.ok(long.endsWith("…"));
});

test("feed entries merge by id, keep unchanged identities and stay bounded", () => {
  const a: SubagentFeedEntry = { id: "a", kind: "text", text: "Hello" };
  const b: SubagentFeedEntry = { id: "t1", kind: "tool", label: "Bash", detail: "npm test", state: "running" };
  const first = mergeFeedEntries([], [a, b]);
  assert.deepEqual(first.entries.map((entry) => entry.id), ["a", "t1"]);
  const settled = { ...b, state: "completed" as const, output: "ok" };
  const second = mergeFeedEntries(first.entries, [{ ...a }, settled]);
  assert.equal(second.entries[0], first.entries[0], "an equal entry keeps its identity");
  assert.equal(second.entries[1], settled, "a changed entry is replaced in place");
  assert.equal(mergeFeedEntries(second.entries, []).entries, second.entries);
  const many = Array.from({ length: 10 }, (_, index) => ({ id: `e${index}`, kind: "text" as const, text: String(index) }));
  const bounded = mergeFeedEntries(second.entries, many, 5);
  assert.equal(bounded.entries.length, 5);
  assert.equal(bounded.dropped, 7);
  assert.equal(bounded.entries[0].id, "e5");
});