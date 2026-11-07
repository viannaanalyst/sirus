// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import assert from "node:assert/strict";
import { test } from "node:test";

import { commandLabel, commandPaths, groupTitle } from "../src/components/thread/steps.ts";

const step = (kind: string, label: string, detail = "") => ({ kind, label, detail }) as never;

test("a command reads as its program and subcommand", () => {
  assert.equal(commandLabel("git status --short"), "git status");
  assert.equal(commandLabel("/opt/homebrew/bin/cloudflared tunnel run"), "cloudflared tunnel");
  assert.equal(commandLabel("ls ~/Projetos"), "ls");
  assert.equal(commandLabel("cd /tmp && ls"), "cd");
});

test("a command names its files and folders", () => {
  assert.deepEqual(commandPaths("grep -rn x ~/app/src/move.rs ./docs/"), [
    { name: "move.rs", folder: false },
    { name: "docs", folder: true },
  ]);
  assert.deepEqual(commandPaths("echo hi && ls"), []);
});

test("a group says what it is doing, then what it did", () => {
  const items = [step("command", "Command", "ls"), step("command", "Command", "pwd"), step("edit", "File changes", "a.ts")];
  assert.equal(groupTitle(items, true), "Executando 2 comandos · Editando 1 arquivo");
  assert.equal(groupTitle(items, false), "Executou 2 comandos · Editou 1 arquivo");
});
