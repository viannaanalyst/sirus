// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";
import assert from "node:assert/strict";
import { activityElapsed, formatActivityDuration, canAnswer } from "../src/shared/lib/agent-activity.ts";
import type { TurnActivity, InputQuestion } from "../src/shared/client/types.ts";
const activity: TurnActivity = { provider: "codex", model: "model", startedAt: 1000, endedAt: null, waitingSince: null, pausedMs: 2000, status: "running", items: [], truncated: false };
test("display clock freezes on native waits and finality rather than session update dates", () => {
  assert.equal(activityElapsed(activity, 8000), 5);
  assert.equal(activityElapsed({ ...activity, waitingSince: 6000, status: "waiting" }, 999999), 3);
  assert.equal(activityElapsed({ ...activity, endedAt: 7000, status: "completed" }, 999999), 4);
  assert.equal(activityElapsed(activity, 0), 0);
});
test("compact duration retains hour/minute precision", () => {
  assert.equal(formatActivityDuration(0), "0s"); assert.equal(formatActivityDuration(68), "1m 8s"); assert.equal(formatActivityDuration(3661), "1h 1m");
});
const question: InputQuestion = { id: "q", header: "Scope", question: "Which?", isOther: false, isSecret: false, options: [{ label: "Offered", description: "Native option" }] };
test("native questions require offered or admitted custom answers and enforce UTF-8 byte bounds", () => {
  assert.equal(canAnswer(question, "Offered"), true); assert.equal(canAnswer(question, "Made up"), false);
  assert.equal(canAnswer({ ...question, isOther: true }, "Custom"), true);
  assert.equal(canAnswer({ ...question, options: null }, " "), false);
  assert.equal(canAnswer({ ...question, options: null }, "🚂".repeat(2049)), false);
  assert.equal(canAnswer({ ...question, isSecret: true }, "Offered"), false);
});
