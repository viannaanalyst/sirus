// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's).
import assert from "node:assert/strict";
import { test } from "node:test";

import { pairingFromLink } from "../src/data/pairing-link";

test("the app link gives the address, the code and the route's access", () => {
  const found = pairingFromLink("sirus://pair?address=https%3A%2F%2Fmac.example.com&code=123456&cfid=a.access&cfsecret=b");
  assert.equal(found?.address, "https://mac.example.com");
  assert.equal(found?.code, "123456");
  assert.deepEqual(found?.access, { clientId: "a.access", clientSecret: "b" });
});

test("a host's own link gives its origin and code", () => {
  const found = pairingFromLink("https://host.example.com/?pair=654321");
  assert.equal(found?.address, "https://host.example.com");
  assert.equal(found?.code, "654321");
  assert.equal(found?.access, undefined);
});

test("anything else is not a pairing", () => {
  assert.equal(pairingFromLink("https://example.com/"), null);
  assert.equal(pairingFromLink("sirus://pair?code=123456"), null);
  assert.equal(pairingFromLink("sirus://pair?address=https%3A%2F%2Fm.co&code=12"), null);
  assert.equal(pairingFromLink("hello"), null);
});
