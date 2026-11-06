// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";

import assert from "node:assert/strict";
import { canSnooze, isSnoozed, snoozeClock, snoozeCountdown, snoozeDeadline, snoozePresetTime, snoozeRemaining } from "../src/shared/lib/snooze.ts";

import { translate } from "../src/shared/i18n/index.ts";


// A fixed clock in the person's local time zone: Friday 9 October 2026, 14:37:25.
const now = new Date(2026, 9, 9, 14, 37, 25, 500);
const local = (month: number, day: number, hours: number, minutes = 0) => new Date(2026, month, day, hours, minutes, 0, 0);
const pt = (key: string, values?: Record<string, string | number>) => translate("pt-BR", key, values);
const en = (key: string, values?: Record<string, string | number>) => translate("en", key, values);

test("hour presets count elapsed time on a whole minute", () => {
  assert.equal(snoozePresetTime("hour", now).getTime(), local(9, 9, 15, 37).getTime());
  assert.equal(snoozePresetTime("threeHours", now).getTime(), local(9, 9, 17, 37).getTime());
  // Late in the evening, three hours cross midnight.
  assert.equal(snoozePresetTime("threeHours", new Date(2026, 9, 9, 22, 30)).getTime(), local(9, 10, 1, 30).getTime());
});

test("tomorrow and next Monday land at 9 in the morning, local time", () => {
  assert.equal(snoozePresetTime("tomorrow", now).getTime(), local(9, 10, 9).getTime());
  // Just after midnight, "tomorrow" is still the next calendar day.
  assert.equal(snoozePresetTime("tomorrow", new Date(2026, 9, 10, 0, 5)).getTime(), local(9, 11, 9).getTime());
  // End of month rolls over.
  assert.equal(snoozePresetTime("tomorrow", new Date(2026, 9, 31, 18, 0)).getTime(), local(10, 1, 9).getTime());
  assert.equal(snoozePresetTime("monday", now).getTime(), local(9, 12, 9).getTime());
  // On a Sunday, next Monday is tomorrow; on a Monday (even before 9), it is a week later.
  assert.equal(snoozePresetTime("monday", new Date(2026, 9, 11, 20, 0)).getTime(), local(9, 12, 9).getTime());
  assert.equal(snoozePresetTime("monday", new Date(2026, 9, 12, 7, 0)).getTime(), local(9, 19, 9).getTime());
});

test("deadlines and countdowns read naturally in both languages", () => {
  assert.equal(snoozeClock(local(9, 10, 9), "pt-BR"), "9h");
  assert.equal(snoozeDeadline(local(9, 10, 9), now, "pt-BR", pt), "amanhã às 9h");
  assert.equal(snoozeDeadline(local(9, 9, 17, 37), now, "pt-BR", pt), "hoje às 17:37");
  assert.match(snoozeDeadline(local(9, 12, 9), now, "pt-BR", pt), /^seg\.?, 12 de out\.? às 9h$/);
  assert.equal(snoozeDeadline(local(9, 10, 9), now, "en", en), "tomorrow at 9 AM");
  assert.match(snoozeDeadline(new Date(2027, 0, 4, 9), now, "en", en), /2027 at 9 AM$/);
  assert.equal(snoozeRemaining(new Date(now.getTime() + (2 * 60 + 10) * 60_000), now), "2 h 10 min");
  assert.equal(snoozeRemaining(new Date(now.getTime() + 45 * 60_000), now), "45 min");
  assert.equal(snoozeRemaining(new Date(now.getTime() + 3 * 3_600_000), now), "3 h");
  assert.equal(snoozeRemaining(new Date(now.getTime() + (3 * 24 + 4) * 3_600_000), now), "3 d 4 h");
  assert.equal(snoozeRemaining(new Date(now.getTime() + 20_000), now), "1 min");
  assert.equal(snoozeRemaining(new Date(now.getTime() - 1000), now), null);
  const later = new Date(now.getTime() + 130 * 60_000).toISOString();
  assert.equal(snoozeCountdown(later, now, pt), "volta em 2 h 10 min");
  assert.equal(snoozeCountdown(later, now, en), "back in 2 h 10 min");
  assert.equal(snoozeCountdown(now.toISOString(), now, pt), "volta em instantes");
});

test("only settled conversations can be snoozed, and a turn brings one back to the lists", () => {
  for (const status of ["starting", "running", "waiting"] as const) assert.equal(canSnooze({ status }), false);
  for (const status of ["idle", "completed", "failed", "stopped"] as const) assert.equal(canSnooze({ status }), true);
  assert.equal(isSnoozed({ status: "completed", snoozedUntil: "2026-10-10T12:00:00.000Z" }), true);
  assert.equal(isSnoozed({ status: "running", snoozedUntil: "2026-10-10T12:00:00.000Z" }), false);
  assert.equal(isSnoozed({ status: "completed", snoozedUntil: null }), false);
});