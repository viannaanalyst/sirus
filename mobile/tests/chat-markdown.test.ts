// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";
import assert from "node:assert/strict";
import { fileReference } from "../src/shared/lib/chat-markdown.ts";

test("file references are relative workspace paths that look like files", () => {
  assert.deepEqual(fileReference("src/api/orders.js", "/w"), { path: "src/api/orders.js", line: null });
  assert.deepEqual(fileReference("src/ui/OrdersPage.js:12", "/w"), { path: "src/ui/OrdersPage.js", line: 12 });
  assert.deepEqual(fileReference("README.md", "/w"), { path: "README.md", line: null });
  assert.deepEqual(fileReference("/w/package.json", "/w"), { path: "package.json", line: null });
  assert.deepEqual(fileReference("src/", "/w"), { path: "src", line: null });
  for (const value of ["listOrders", "node --test", "/etc/passwd", "../secret.txt", "https://a.b/c.js", "v1.2", "a//b.js"]) assert.equal(fileReference(value, "/w"), null, value);
});
