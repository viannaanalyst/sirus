// @ts-nocheck -- run with `node --import ../scripts/register-test-loader.mjs --test tests/*.test.ts`
import { test } from "node:test";
import assert from "node:assert/strict";
import { buildFeed, currentStep, describeRequest, hourlyActivity } from "../src/data/dashboard.ts";
import { pace, daysLeft, weeklyWindow, parseWindows } from "../src/data/usage-pace.ts";
import { bucketLabel, deltaPercent, formatCost, formatTokens, mergeSummaries } from "../src/data/usage-summary.ts";
import { labelX, smoothPath, stackedLayout } from "../src/lib/chart-path.ts";
import { hashName, robotLook } from "../src/data/project-robot.ts";

const turn = (items, extra = {}) => ({ provider: "claude", model: null, startedAt: 0, endedAt: null, waitingSince: null, pausedMs: 0, status: "running", items, truncated: false, ...extra });

test("the current step reads the newest running item in pt-BR", () => {
  const items = [{ id: "1", kind: "edit", label: "Edit", state: "completed", model: null, detail: "src/a.ts" }, { id: "2", kind: "edit", label: "Edit", state: "running", model: null, detail: "src/AreaLogada.tsx" }];
  assert.equal(currentStep(turn(items)), "Editando AreaLogada.tsx · 2 arquivos");
  assert.equal(currentStep(turn([], { endedAt: 5 })), null);
});

test("requests are described by what they ask", () => {
  const command = { requestId: "r", generation: "g", turnId: "t", itemId: "i", kind: { type: "command", command: "npm publish\nmore", cwd: null, reason: null } };
  assert.deepEqual(describeRequest(command), { text: "quer rodar", code: "npm publish", question: false });
});

test("hourly activity counts only today, per hour", () => {
  const now = new Date(2026, 10, 6, 15, 30).getTime();
  const counts = hourlyActivity([new Date(2026, 10, 6, 9, 5).getTime(), new Date(2026, 10, 6, 9, 50).getTime(), new Date(2026, 10, 5, 23, 0).getTime()], now);
  assert.equal(counts[9], 2);
  assert.equal(counts.reduce((a, b) => a + b, 0), 2);
});

test("the feed is newest first and skips idle old conversations", () => {
  const now = Date.now();
  const base = { projectName: "p", title: "t", turn: undefined, request: undefined, done: false };
  const feed = buildFeed([
    { ...base, key: "a", status: "failed", lastActivityAt: new Date(now - 60_000).toISOString() },
    { ...base, key: "b", status: null, done: true, lastActivityAt: new Date(now - 10 * 60_000).toISOString() },
    { ...base, key: "c", status: null, done: false, lastActivityAt: new Date(now).toISOString() },
  ], now);
  assert.deepEqual(feed.map((event) => event.key), ["a:fail", "b:done"]);
});

test("the weekly pace follows the Mac: allowance spread over the days left", () => {
  const now = new Date(2026, 10, 6, 12, 0).getTime();
  const resets = new Date(2026, 10, 9, 12, 0).getTime(); // 4 local days counting today
  assert.equal(daysLeft(now, resets), 4);
  const result = pace(38, { used: 30, resetsAt: resets, at: now }, resets, 10_080, now);
  assert.equal(result.start, 30);
  assert.equal(result.today, 8);
  assert.equal(result.allowance, 17.5);
  assert.equal(result.over, false);
  // a baseline from a week ago is no use after the reset: the week began at 0
  assert.equal(pace(5, { used: 92, resetsAt: resets - 7 * 86_400_000, at: 0 }, resets, 10_080, now).start, 0);
  const windows = parseWindows({ windows: [{ id: "seven_day", usedPercent: 62, resetsAt: Math.floor(resets / 1000), durationMinutes: 10_080 }, { id: "seven_day_opus", usedPercent: 10, resetsAt: Math.floor(resets / 1000), durationMinutes: 10_080 }] });
  assert.equal(weeklyWindow(windows).id, "seven_day");
  assert.equal(windows[0].resetsAt, resets);
});

test("usage summaries merge by instant and model, and money formats in pt-BR", () => {
  const one = { buckets: [{ at: 1_700_000_000, costs: { claude: 1 }, tokens: { claude: 10 } }], models: [{ model: "m", provider: "claude", cost: 1, tokens: 10 }], totalCost: 1, totalTokens: 10, previousCost: 2 };
  const merged = mergeSummaries([one, one]);
  assert.equal(merged.buckets.length, 1);
  assert.equal(merged.buckets[0].costs.claude, 2);
  assert.equal(merged.models[0].cost, 2);
  assert.equal(formatCost(148.2), "$148,20");
  assert.equal(formatCost(1704.4), "$1.704");
  assert.equal(formatTokens(1_250_000), "1,3 mi");
  assert.equal(deltaPercent(88, 100), -12);
  assert.equal(deltaPercent(1, 0), null);
  assert.equal(bucketLabel(new Date(2026, 10, 6, 9).getTime(), 1), "09h");
});

test("stacked areas pile up and keep a single point drawable", () => {
  const layout = stackedLayout([{ key: "a", color: "#fff", values: [1, 2, 3] }, { key: "b", color: "#000", values: [1, 1, 1] }], 300, 100);
  assert.deepEqual(layout.totals, [2, 3, 4]);
  assert.equal(layout.layers.length, 2);
  assert.ok(layout.layers[1].area.endsWith("Z"));
  assert.doesNotThrow(() => stackedLayout([{ key: "a", color: "#fff", values: [5] }], 300, 100));
});

test("a project's robot is stable for its name and honours a saved look", () => {
  assert.equal(hashName("moradah"), hashName("moradah"));
  const saved = robotLook("x", { color: "#112233", eyes: "happy", accessory: "ghost" });
  assert.deepEqual(saved, { shape: "ghost", color: "#112233", eyes: "happy" });
  assert.match(robotLook("moradah").color, /^#[0-9a-f]{6}$/);
});

test("the smooth path is monotone: a spike next to zero never dips under the baseline", () => {
  const baseline = 100;
  const points = [[0, baseline], [10, baseline], [20, 10], [30, baseline], [40, baseline]];
  const numbers = smoothPath(points).match(/-?\d+(\.\d+)?/g).map(Number);
  // M x y, then C x1 y1 x2 y2 x y per segment: every y stays within [10, baseline].
  const ys = numbers.filter((_, index) => index % 2 === 1);
  assert.ok(ys.every((y) => y <= baseline && y >= 10), ys.join(","));
  // Reversed (the area's way back) holds as well.
  const back = smoothPath([...points].reverse()).match(/-?\d+(\.\d+)?/g).map(Number).filter((_, index) => index % 2 === 1);
  assert.ok(back.every((y) => y <= baseline && y >= 10));
});

test("only the drawn points are laid out while the x axis keeps the full length", () => {
  const layout = stackedLayout([{ key: "a", color: "#000", values: [1, 2, 3, 0, 0, 0] }], 100, 100, 20, 3);
  assert.equal(layout.layers[0].top.length, 3);
  assert.equal(layout.x(5), 100);
});

test("axis labels stay inside the chart", () => {
  assert.equal(labelX(0, 300, 3) > 0, true);
  assert.equal(labelX(300, 300, 3) < 300, true);
  assert.equal(labelX(150, 300, 3), 150);
});

test("model ids read as names, unknown ones stay readable", async () => {
  const { modelDisplayName } = await import("../src/data/model-name.ts");
  assert.equal(modelDisplayName("claude-opus-5-5"), "Opus 5.5");
  assert.equal(modelDisplayName("claude-haiku-4-5-20251001"), "Haiku 4.5");
  assert.equal(modelDisplayName("deepseek-v4.1-flash"), "DeepSeek V4.1 Flash");
  assert.equal(modelDisplayName("gpt-5.4-mini"), "GPT-5.4 Mini");
  assert.equal(modelDisplayName("acme/weird_model-x2"), "Weird Model X2");
  assert.equal(modelDisplayName("o3"), "O3");
  assert.equal(modelDisplayName(""), "");
});
