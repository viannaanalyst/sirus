// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";

import { gzipSync } from "node:zlib";

import assert from "node:assert/strict";
import { invokeTimeout, RemoteTransport, reconnectDelay, type RemoteConnection, type RemoteSocket } from "../src/shared/client/remote-transport.ts";


class FakeSocket implements RemoteSocket {
  readyState = 0;
  sent: Record<string, unknown>[] = [];
  onopen: (() => void) | null = null;
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  binaryType?: string;
  send(data: string) { this.sent.push(JSON.parse(data) as Record<string, unknown>); }
  close() { this.readyState = 3; this.onclose?.(); }
  open() { this.readyState = 1; this.onopen?.(); }
  receive(message: object) { this.onmessage?.({ data: JSON.stringify(message) }); }
}

function setup(token: string | null = "secret") {
  const sockets: FakeSocket[] = [];
  const timers: (() => void)[] = [];
  const states: RemoteConnection[] = [];
  const transport = new RemoteTransport({
    url: "ws://mac/api/socket",
    token: () => token,
    createSocket: () => { const socket = new FakeSocket(); sockets.push(socket); return socket; },
    onConnection: (state) => states.push(state),
    setTimer: (run) => timers.push(run),
  });
  return { transport, sockets, timers, states };
}

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

test("invokes after the hello and resolves with the Mac's reply", async () => {
  const { transport, sockets } = setup();
  const reply = transport.invoke<{ ok: boolean }>("load_state", { a: 1 });
  sockets[0].open();
  assert.deepEqual(sockets[0].sent[0], { type: "hello", token: "secret", gzip: true });
  sockets[0].receive({ type: "ready" });
  await tick();
  assert.deepEqual(sockets[0].sent[1], { type: "invoke", id: 1, cmd: "load_state", args: { a: 1 } });
  sockets[0].receive({ type: "result", id: 1, ok: true, value: { ok: true } });
  assert.deepEqual(await reply, { ok: true });
});

test("errors keep the command's message and raw replies become bytes", async () => {
  const { transport, sockets } = setup();
  const failing = transport.invoke("git_push");
  sockets[0].open();
  sockets[0].receive({ type: "ready" });
  await tick();
  sockets[0].receive({ type: "result", id: 1, ok: false, error: { code: "git", message: "Nothing to push." } });
  await assert.rejects(failing, /Nothing to push/);
  const raw = transport.invoke<ArrayBuffer>("frame");
  await tick();
  sockets[0].receive({ type: "result", id: 2, ok: true, raw: "AQI=" });
  assert.deepEqual([...new Uint8Array(await raw)], [1, 2]);
});

test("events reach listeners and subscriptions are renewed after a reconnect", async () => {
  const { transport, sockets, timers, states } = setup();
  const seen: unknown[] = [];
  const stop = await transport.listen("session-updated", (payload) => seen.push(payload));
  sockets[0].open();
  sockets[0].receive({ type: "ready" });
  await tick();
  assert.deepEqual(sockets[0].sent.at(-1), { type: "listen", event: "session-updated" });
  sockets[0].receive({ type: "event", event: "session-updated", payload: { id: "s" } });
  assert.deepEqual(seen, [{ id: "s" }]);

  const waiting = transport.invoke("load_state");
  sockets[0].close();
  await assert.rejects(waiting, /lost/);
  assert.equal(states.at(-1), "lost");
  assert.equal(timers.length, 1, "a listener keeps the connection coming back");
  timers[0]();
  sockets[1].open();
  sockets[1].receive({ type: "ready" });
  await tick();
  assert.deepEqual(sockets[1].sent.slice(1), [{ type: "listen", event: "session-updated" }]);
  stop();
  assert.deepEqual(sockets[1].sent.at(-1), { type: "unlisten", event: "session-updated" });
});

test("a refused token stops reconnecting", async () => {
  const { transport, sockets, timers, states } = setup();
  await transport.listen("agent-output", () => undefined);
  sockets[0].open();
  sockets[0].receive({ type: "unauthorized" });
  assert.equal(states.at(-1), "unauthorized");
  assert.equal(timers.length, 0);
  await assert.rejects(transport.invoke("load_state"), /Pair it again/);
});

test("without a token nothing connects", async () => {
  const { transport, sockets } = setup(null);
  await assert.rejects(transport.invoke("load_state"), /not paired/);
  assert.equal(sockets.length, 0);
});

test("reconnect waits grow and stay capped", () => {
  assert.deepEqual([0, 1, 2, 3].map(reconnectDelay), [1000, 2000, 4000, 8000]);
  assert.equal(reconnectDelay(20), 30_000);
});

test("a command the Mac never answers fails after its time limit", async (context) => {
  context.mock.timers.enable({ apis: ["setTimeout"] });
  const { transport, sockets } = setup();
  const reply = transport.invoke("load_state");
  sockets[0].open();
  sockets[0].receive({ type: "ready" });
  await new Promise((resolve) => setImmediate(resolve));
  context.mock.timers.tick(invokeTimeout("load_state"));
  await assert.rejects(reply, /did not answer in time/);
  assert.ok(invokeTimeout("send_prompt") > invokeTimeout("load_state"));
});

test("gzipped frames inflate and keep their order with the text frames after them", async () => {
  const { transport, sockets } = setup();
  const events: unknown[] = [];
  await transport.listen("agent-output", (payload) => events.push(payload));
  const big = transport.invoke<{ text: string }>("transcript_action");
  sockets[0].open();
  assert.equal(sockets[0].binaryType, "arraybuffer");
  sockets[0].receive({ type: "ready" });
  await tick();
  const result = { type: "result", id: 1, ok: true, value: { text: "x".repeat(50_000) } };
  const bytes = gzipSync(JSON.stringify(result));
  sockets[0].onmessage?.({ data: bytes.buffer.slice(bytes.byteOffset, bytes.byteOffset + bytes.byteLength) });
  // Sent after the gzipped result: handled after it, though it needs no inflating.
  sockets[0].receive({ type: "event", event: "agent-output", payload: "after" });
  assert.deepEqual(events, [], "the text frame waits for the frame before it");
  assert.equal((await big).text.length, 50_000);
  await tick();
  assert.deepEqual(events, ["after"]);
});