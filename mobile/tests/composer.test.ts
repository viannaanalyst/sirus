// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";
import assert from "node:assert/strict";
import { completeComposerToken, composerTrigger } from "../src/shared/lib/composer-suggestions.ts";
import { afterSend, executionWithModes, modeChips, NO_MODES, promptExtras, setDebugging, setPlanning, setTeam, supportsPlanning } from "../src/shared/lib/composer-modes.ts";
import { filterSkills, insertSkillInvocation } from "../src/shared/lib/skills.ts";
import { splitAttachments } from "../src/shared/lib/sent-attachments.ts";
import { scrollTargetFor } from "../src/components/thread/work-anchors.ts";

test("a slash or at-sign token under the caret opens the matching list", () => {
  assert.deepEqual(composerTrigger("/re", 3), { kind: "skill", query: "re", start: 0, end: 3 });
  assert.deepEqual(composerTrigger("olha @src/ap", 12), { kind: "file", query: "src/ap", start: 5, end: 12 });
  assert.equal(composerTrigger("veja a/b", 8), null);
  assert.equal(composerTrigger("eu@gmail.com", 12), null);
  assert.equal(composerTrigger("/re", 1, 3), null);
  assert.equal(composerTrigger("/usr/bin", 8), null);
});

test("choosing a suggestion replaces the token and quotes paths with spaces", () => {
  const trigger = composerTrigger("use /re agora", 6)!;
  assert.deepEqual(completeComposerToken("use /re agora", trigger, "review"), { value: "use /review agora", caret: 11 });
  const file = composerTrigger("ver @rea", 8)!;
  assert.deepEqual(completeComposerToken("ver @rea", file, "docs/README.md"), { value: "ver @docs/README.md ", caret: 20 });
  assert.equal(completeComposerToken("@x", composerTrigger("@x", 2)!, "meu arquivo.txt").value, '@"meu arquivo.txt" ');
  // A folder keeps the token open to go deeper.
  assert.deepEqual(completeComposerToken("@sr", composerTrigger("@sr", 3)!, "src", true), { value: "@src/", caret: 5 });
});

test("skills filter by every word and skip disabled ones", () => {
  const skill = (name: string, description: string) => ({ name, description, sources: [] });
  const all = [skill("review", "Revisa o diff"), skill("deploy", "Publica"), skill("revert", "Desfaz")];
  assert.deepEqual(filterSkills(all, "rev").map((s) => s.name), ["review", "revert"]);
  assert.deepEqual(filterSkills(all, "rev diff").map((s) => s.name), ["review"]);
  assert.deepEqual(filterSkills(all, "rev", ["review"], true).map((s) => s.name), ["revert"]);
  assert.equal(insertSkillInvocation("faça isso", "review"), "/review faça isso");
  assert.equal(insertSkillInvocation("/review faça isso", "review"), "/review faça isso");
});

test("modes exclude each other as the Mac's add-menu does", () => {
  let modes = setPlanning(NO_MODES, true);
  modes = setDebugging(modes, true);
  assert.deepEqual([modes.planning, modes.debugging], [false, true]);
  modes = setTeam(modes, true);
  assert.deepEqual([modes.team, modes.debugging, modes.planning], [true, false, false]);
  assert.equal(afterSend(modes).team, false);
});

test("a send carries planning, the goal, debug and team only where the provider can", () => {
  assert.equal(supportsPlanning("codex", null), false);
  assert.equal(supportsPlanning("codex", "gpt-5"), true);
  const planning = setPlanning({ ...NO_MODES, goal: "terminar" }, true);
  assert.deepEqual(executionWithModes({ approval: "full", effort: "high" }, planning, "claude", null), { approval: "ask", effort: "high", planning: true });
  assert.equal(executionWithModes({ approval: "full" }, planning, "cursor", null).approval, "auto");
  assert.equal(executionWithModes({ approval: "full" }, planning, "grok", null).planning, false);
  assert.deepEqual(promptExtras(planning, "claude", null), { goal: "terminar", debugging: false });
  assert.deepEqual(promptExtras(setTeam(NO_MODES, true), "claude", null), { goal: null, debugging: false, team: true });
  assert.deepEqual(promptExtras(setTeam(NO_MODES, true), "grok", null), { goal: null, debugging: false });
  assert.deepEqual(modeChips({ goal: "g", planning: false, debugging: true, team: false }).map((chip) => chip.id), ["debugging", "goal"]);
});

test("the jump to the current work keeps it above the composer and the pill", () => {
  const base = { viewport: 800, topPad: 100, bottomReserve: 200, contentHeight: 3000 };
  // Fits: its bottom edge lands at the top of the reserved area.
  assert.equal(scrollTargetFor({ ...base, y: 1000, height: 100 }), 1000 + 100 - 600);
  // Taller than the room: its top lands under the header.
  assert.equal(scrollTargetFor({ ...base, y: 1000, height: 900 }), 900);
  // Never past the content or before the start.
  assert.equal(scrollTargetFor({ ...base, y: 2900, height: 100 }), 2200);
  assert.equal(scrollTargetFor({ ...base, y: 10, height: 20 }), 0);
});

test("a sent message shows pictures with a served thumbnail as images and the rest as chips", () => {
  const url = (id: string) => `https://mac/api/thumbnail/${id}?token=t`;
  const { images, files } = splitAttachments([
    { id: "a", name: "foto.jpg", kind: "file", hasThumbnail: true },
    { id: "b", name: "notas.pdf", kind: "file" },
    { id: "c", name: "velha.png", kind: "file", thumbnail: "data:image/jpeg;base64,xx" },
  ], url);
  assert.deepEqual(images.map((image) => image.small), ["https://mac/api/thumbnail/a?token=t&size=small", "data:image/jpeg;base64,xx"]);
  assert.equal(images[0].full, "https://mac/api/thumbnail/a?token=t");
  assert.deepEqual(files.map((file) => file.name), ["notas.pdf"]);
  // Without a token there is no address: the picture stays a chip.
  assert.equal(splitAttachments([{ id: "a", name: "foto.jpg", kind: "file", hasThumbnail: true }], () => undefined).files.length, 1);
});
