// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import assert from "node:assert/strict";
import { test } from "node:test";
import { compoundExtensions, materialFileIcon } from "../src/shared/lib/material-file-icon.ts";

test("compound extensions peel from the longest suffix and skip a leading dot", () => {
  assert.deepEqual(compoundExtensions("types.d.ts"), ["d.ts", "ts"]);
  assert.deepEqual(compoundExtensions(".env.local"), ["local"]);
  assert.deepEqual(compoundExtensions("makefile"), []);
});

test("Material icons resolve by name, then extension, then the plain file icon", async () => {
  const icons = await import("react-material-icon-theme");
  assert.equal(materialFileIcon(icons, "docker-compose.yml"), "docker");
  assert.equal(materialFileIcon(icons, "package.json"), "nodejs");
  assert.notEqual(materialFileIcon(icons, "config.toml"), "file");
  assert.notEqual(materialFileIcon(icons, "main.rs"), "file");
  assert.equal(materialFileIcon(icons, "notes.zzzunknown"), "file");
  assert.ok(icons.getIconSvg(materialFileIcon(icons, "Cargo.toml"))?.startsWith("<svg"));
});
