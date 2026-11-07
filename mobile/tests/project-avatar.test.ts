// @ts-nocheck
import assert from "node:assert/strict";
import test from "node:test";

import { avatarLook, avatarSvg, voiceLevel } from "../src/lib/project-avatar.ts";

test("a saved look wins and unknown values fall back to the name", () => {
  assert.deepEqual(avatarLook("x", { color: "#112233", eyes: "dot", accessory: "tv" }), { color: "#112233", eyes: "dots", shape: "tv" });
  assert.deepEqual(avatarLook("sirus"), avatarLook("sirus", { color: "nope", accessory: "crown" }));
});

test("a head is complete SVG with the antennas only on the tv", () => {
  const tv = avatarSvg({ color: "#8b7cf6", eyes: "bars", shape: "tv" });
  assert.match(tv, /^<svg .*<\/svg>$/);
  assert.match(tv, /M23 15 16 6/);
  assert.doesNotMatch(avatarSvg({ color: "#8b7cf6", eyes: "happy", shape: "round" }), /M23 15/);
});

test("the voice maps to 0 to 1 and silence is zero", () => {
  assert.equal(voiceLevel(-2), 0);
  assert.equal(voiceLevel(5), 0.5);
  assert.equal(voiceLevel(12), 1);
});
