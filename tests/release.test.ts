import { test } from "node:test";
import assert from "node:assert/strict";
import {
  PLATFORM,
  bumpCargoLock,
  bumpCargoVersion,
  bumpJsonVersion,
  compareVersions,
  feedProblems,
  latestTag,
  notesFromCommits,
  parseArgs,
  parseVersion,
  publicKeyId,
  updateFeed,
  verifyUpdateSignature,
} from "../scripts/release-lib.ts";

const v = (raw: string) => {
  const parsed = parseVersion(raw);
  assert.ok(parsed, raw);
  return parsed;
};

test("versions compare as semantic versions", () => {
  assert.ok(compareVersions(v("0.1.1"), v("0.1.0")) > 0);
  assert.ok(compareVersions(v("0.2.0"), v("0.1.9")) > 0);
  assert.ok(compareVersions(v("0.10.0"), v("0.9.0")) > 0);
  assert.equal(compareVersions(v("v1.0.0"), v("1.0.0")), 0);
  assert.ok(compareVersions(v("1.0.0-beta.1"), v("1.0.0")) < 0);
  assert.ok(compareVersions(v("1.0.0-beta.2"), v("1.0.0-beta.10")) < 0);
  assert.ok(compareVersions(v("1.0.0-alpha"), v("1.0.0-beta")) < 0);
  assert.equal(parseVersion("1.0"), null);
  assert.equal(parseVersion("01.0.0"), null);
  assert.equal(latestTag(["v0.1.0", "v0.10.0", "v0.9.3", "nightly", ""]), "v0.10.0");
  assert.equal(latestTag([]), null);
});

test("arguments name one version and known options", () => {
  assert.deepEqual(parseArgs(["v0.1.1", "--dry-run", "--skip-checks", "--notes", "Fixes"]), { version: "0.1.1", dryRun: true, skipChecks: true, notes: "Fixes", key: null });
  assert.equal(parseArgs(["0.2.0", "--notes=Hi"]).notes, "Hi");
  assert.throws(() => parseArgs([]));
  assert.throws(() => parseArgs(["0.1"]));
  assert.throws(() => parseArgs(["0.1.1", "--publish"]));
  assert.throws(() => parseArgs(["0.1.1", "--notes"]));
});

test("notes list commit subjects without merges or earlier releases", () => {
  assert.equal(notesFromCommits(["Faster sidebar", "Merge branch 'x'", "Release v0.1.0", "", "Fix toast"]), "- Faster sidebar\n- Fix toast");
  assert.equal(notesFromCommits([]), "Maintenance release.");
});

test("latest.json has the shape the updater reads", () => {
  const feed = updateFeed({ version: "0.1.1", notes: " - Fix\n", date: new Date("2026-10-18T12:00:00.123Z"), signature: "c2ln\n" });
  assert.deepEqual(feed, {
    version: "0.1.1",
    notes: "- Fix",
    pub_date: "2026-10-18T12:00:00Z",
    platforms: { [PLATFORM]: { signature: "c2ln", url: "https://github.com/viannaanalyst/sirus/releases/download/v0.1.1/SirusCode-arm64.app.tar.gz" } },
  });
  assert.deepEqual(feedProblems(feed), []);
  assert.ok(feedProblems({ ...feed, version: "v0.1.1", platforms: {} }).length === 2);
  assert.ok(updateFeed({ version: "1.0.0", notes: "x".repeat(9000), date: new Date(), signature: "s" }).notes.length <= 4000);
});

test("version files are bumped in place", () => {
  assert.equal(bumpJsonVersion('{\n  "name": "a",\n  "version": "0.1.0",\n  "dependencies": { "b": { "version": "9" } }\n}', "0.1.1"), '{\n  "name": "a",\n  "version": "0.1.1",\n  "dependencies": { "b": { "version": "9" } }\n}');
  const cargo = '[package]\nname = "sirus-code"\nversion = "0.1.0"\n\n[dependencies]\nserde = { version = "1" }\n';
  assert.equal(bumpCargoVersion(cargo, "0.1.1"), cargo.replace('"0.1.0"', '"0.1.1"'));
  const lock = '[[package]]\nname = "serde"\nversion = "1.0.0"\n\n[[package]]\nname = "sirus-code"\nversion = "0.1.0"\n';
  assert.equal(bumpCargoLock(lock, "sirus-code", "0.1.1"), lock.replace('"0.1.0"', '"0.1.1"'));
  assert.throws(() => bumpCargoLock(lock, "other", "0.1.1"));
});

// Signed with a throwaway key made by `tauri signer` for this test only.
const PUBKEY = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IEY1OUJCMEEyODcxNTQyRjMKUldUelFoV0hvckNiOVFiczRUOW9iNnBhRVdOb3NORnlQQTU1eExvNVhreDF0Q2ZUOTVaeHNrQ3cK";
const SIGNATURE = "dW50cnVzdGVkIGNvbW1lbnQ6IHNpZ25hdHVyZSBmcm9tIHRhdXJpIHNlY3JldCBrZXkKUlVUelFoV0hvckNiOVlXZ3MzUWtzMWVCbFA0VzFHSkNaT2RpVlozWng2YTFEZWFIOVNHeEJaVDBRbkx5V3MzaFFyVHYxemcvVnhpRTFiLzc3S0J3cDdZMWF3eU5DTmwxS2dVPQp0cnVzdGVkIGNvbW1lbnQ6IHRpbWVzdGFtcDoxNzkyMzM2OTA4CWZpbGU6cGF5bG9hZC5iaW4KdXVSQWxOaXpuSCtxWWxhMEVjdi9ZdTZwNENQL0psY0hESXNMU08vWDE3VjZtMm1yc3BFTmU1elo0M24zSnFSYTZ5L1FZb2FlRkV3cEtUZnRTRzZtQnc9PQo=";
const PAYLOAD = Buffer.from("hello sirus update\n");

test("update signatures are checked against the trusted public key", () => {
  assert.equal(publicKeyId(PUBKEY), "f3421587a2b09bf5");
  assert.deepEqual(verifyUpdateSignature(PAYLOAD, SIGNATURE, PUBKEY), { ok: true });
  assert.equal(verifyUpdateSignature(Buffer.from("hello sirus updatE\n"), SIGNATURE, PUBKEY).ok, false);
  const appKey = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDZGNDdGMUM5M0JBOEVDMjgKUldRbzdLZzd5ZkZIYjRHWVNZcHA4NjdXK2lWUXdpTUlKRnNGWmhhWWV0MStrTTdRY1Q4VGZmdWEK";
  const other = verifyUpdateSignature(PAYLOAD, SIGNATURE, appKey);
  assert.equal(other.ok, false);
  assert.match(other.reason ?? "", /signed with key/);
  assert.equal(verifyUpdateSignature(PAYLOAD, "bm90IGEgc2lnbmF0dXJl", PUBKEY).ok, false);
});
