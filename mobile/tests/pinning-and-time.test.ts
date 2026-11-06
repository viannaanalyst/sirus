// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";
import assert from "node:assert/strict";
import { applyRowSettingsAction } from "../src/shared/lib/environments.ts";
import { parseGithubReference } from "../src/shared/lib/github-inbox.ts";
import { relativeTime } from "../src/shared/lib/session-board.ts";

test("pinning a row is idempotent and unpinning removes it from the pins", () => {
  const settings = { pinnedSessionIds: ["a"] };
  assert.equal(applyRowSettingsAction(settings, "pin", "a"), settings);
  assert.deepEqual(applyRowSettingsAction(settings, "pin", "b").pinnedSessionIds, ["b", "a"]);
  assert.deepEqual(applyRowSettingsAction(settings, "unpin", "a").pinnedSessionIds, []);
});

test("a GitHub link or short reference is parsed, anything else is refused", () => {
  assert.deepEqual(parseGithubReference("https://github.com/acme/app/pull/12"), { repository: "acme/app", number: 12, kind: "pullRequest" });
  assert.deepEqual(parseGithubReference("acme/app#12"), { repository: "acme/app", number: 12, kind: null });
  assert.equal(parseGithubReference("https://evil.example/acme/app/pull/1"), null);
});

test("relative times are compact", () => {
  const now = Date.parse("2026-10-09T12:00:00Z");
  assert.equal(relativeTime("2026-10-09T11:59:30Z", now), "now");
  assert.equal(relativeTime("2026-10-09T11:55:00Z", now), "5m");
  assert.equal(relativeTime("2026-10-09T10:00:00Z", now), "2h");
  assert.equal(relativeTime("2026-10-03T12:00:00Z", now), "6d");
  assert.equal(relativeTime("nope", now), "");
});
