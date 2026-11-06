// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";
import assert from "node:assert/strict";
import { splitLinks } from "../src/shared/lib/link-text.ts";

test("user text links http(s) addresses and leaves sentence punctuation out", () => {
  assert.deepEqual(splitLinks("veja https://yab.one, ok"), [
    { kind: "text", text: "veja " }, { kind: "link", text: "https://yab.one", url: "https://yab.one" }, { kind: "text", text: ", ok" },
  ]);
  assert.deepEqual(splitLinks("(https://example.com/a)").map((segment) => segment.text), ["(", "https://example.com/a", ")"]);
  assert.equal(splitLinks("https://en.wikipedia.org/wiki/Foo_(bar)")[0].text, "https://en.wikipedia.org/wiki/Foo_(bar)");
  assert.deepEqual(splitLinks("sem link"), [{ kind: "text", text: "sem link" }]);
  assert.equal(splitLinks("file:///etc/passwd").some((segment) => segment.kind === "link"), false);
});

test("bare web addresses link as https, file names and emails do not", () => {
  const link = (text: string) => splitLinks(text).filter((segment) => segment.kind === "link");
  assert.deepEqual(link("E isso aqui ?\n\nportaldatransparencia.gov.br/api-de-dados"), [
    { kind: "link", text: "portaldatransparencia.gov.br/api-de-dados", url: "https://portaldatransparencia.gov.br/api-de-dados" },
  ]);
  assert.equal(link("veja google.com.").at(0)?.url, "https://google.com");
  assert.equal(link("www.cnpj.ws").at(0)?.url, "https://www.cnpj.ws");
  for (const text of ["edite package.json", "abra src/index.ts", "leia README.md", "mande para eu@gmail.com", "versão 1.2.3", "app-store.ts", "localhost:3000"]) {
    assert.equal(link(text).length, 0, text);
  }
});
