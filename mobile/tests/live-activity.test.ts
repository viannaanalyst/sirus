// @ts-nocheck
import assert from "node:assert/strict";
import test from "node:test";

import { activityChoice } from "../src/lib/activity-choice.ts";

test("the lock screen follows the working conversation that spoke last", () => {
  const list = [
    { id: "a", status: "running", lastActivityAt: "2026-11-07T10:00:00Z" },
    { id: "b", status: "completed", lastActivityAt: "2026-11-07T12:00:00Z" },
    { id: "c", status: "waiting", lastActivityAt: "2026-11-07T11:00:00Z" },
  ];
  assert.equal(activityChoice(list).id, "c");
  assert.equal(activityChoice(list.slice(1, 2)), undefined);
});
