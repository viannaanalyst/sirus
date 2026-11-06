// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import test from "node:test";
import assert from "node:assert/strict";
import type { ActivityItem } from "../src/shared/client/types.ts";

import { foldBoundary, timelineParts } from "../src/shared/lib/turn-timeline.ts";

import { translate } from "../src/shared/i18n/index.ts";


const row = (id: string, kind: ActivityItem["kind"], detail: string, offset?: number, state: ActivityItem["state"] = "completed", label = ""): ActivityItem => ({ id, kind, label, state, model: null, detail, offset });
const pt = (key: string, params?: Record<string, string | number>) => translate("pt-BR", key, params);

test("text and work interleave in the order they happened; back-to-back work is one group", () => {
  const content = "Vou ler o README.\n\nAgora rodo os testes.\n\nPronto.";
  const parts = timelineParts(content, [row("s", "skill", "graphify", 0), row("a", "read", "README.md", 18), row("b", "read", "docs/a.md", 19), row("c", "command", "npm test", 41)]);
  assert.deepEqual(parts.map((part) => part.kind === "work" ? `work:${part.items.map((item) => item.id).join("")}` : part.kind === "text" ? `text:${content.slice(part.start, part.end).trim()}` : "steer"), [
    "work:s", "text:Vou ler o README.", "work:ab", "text:Agora rodo os testes.", "work:c", "text:Pronto.",
  ]);
  assert.equal(foldBoundary(parts), 5, "a finished turn folds everything before its final answer");
  // Rows recorded before offsets existed sit at the start.
  assert.deepEqual(timelineParts("Olá", [row("x", "edit", "a.ts")]).map((part) => part.kind), ["work", "text"]);
  assert.equal(foldBoundary(timelineParts("", [row("x", "edit", "a.ts")])), 1);
});