import { formatUnknownError } from "../lib/format-error";
import type { Transport, Unlisten } from "./transport";
import type { DeviceHubAccess } from "./types";

/**
 * The UI served to another device (ADR-080): commands and events travel over one
 * WebSocket to the Mac, which runs them as its own window would. The socket opens
 * on first use, says hello with the device token, and reconnects with backoff while
 * anything listens; subscriptions are renewed after every reconnect.
 */
export type RemoteConnection = "idle" | "connecting" | "open" | "lost" | "unauthorized";

/** The part of `WebSocket` this transport uses, so tests can provide their own. */
export interface RemoteSocket {
  readonly readyState: number;
  onopen: (() => void) | null;
  onmessage: ((event: { data: unknown }) => void) | null;
  onclose: (() => void) | null;
  onerror: (() => void) | null;
  /** Set to "arraybuffer" so gzipped frames (ADR-164) arrive as bytes. */
  binaryType?: string;
  send(data: string): void;
  close(): void;
}

export interface RemoteTransportOptions {
  url: string;
  /** The computer's origin when it is not this page's own (ADR-132); empty or missing means this page's. */
  baseUrl?: string;
  token: () => string | null;
  createSocket?: (url: string) => RemoteSocket;
  /** Called when the connection changes, e.g. to reload state after a reconnect. */
  onConnection?: (state: RemoteConnection, previous: RemoteConnection) => void;
  setTimer?: (run: () => void, ms: number) => unknown;
}

type Message =
  | { type: "ready" }
  | { type: "unauthorized" }
  | { type: "result"; id: number; ok: true; value?: unknown; raw?: string }
  | { type: "result"; id: number; ok: false; error: unknown }
  | { type: "event"; event: string; payload: unknown };

type Pending = { resolve: (value: unknown) => void; reject: (error: Error) => void };

const OPEN = 1;

const MAX_DELAY = 30_000;

const UNREACHABLE = "Cannot reach the Mac. Check that Sirus is open.";

const LOST = "The connection to the Mac was lost.";

const TIMED_OUT = "The Mac did not answer in time.";

/** How long a command may wait for its result before it fails (a silent socket would hang it forever). */
const INVOKE_TIMEOUT = 2 * 60_000;

/** Commands that legitimately run long on the Mac: Setup scripts, network Git, installs, logins. */
const LONG_INVOKE_TIMEOUT = 15 * 60_000;

const LONG_COMMANDS = new Set([
  "send_prompt", "retry_last_turn", "clone_project", "create_project", "update_providers",
  "login_provider_account", "git_push", "git_workspace_action", "pull_request_action",
  "project_scripts_action", "worktree_cleanup_action", "device_action", "connector_action",
  "prepare_session_move", "import_session", "commit_title_action",
]);

export function invokeTimeout(command: string): number {
  return LONG_COMMANDS.has(command) ? LONG_INVOKE_TIMEOUT : INVOKE_TIMEOUT;
}

export function reconnectDelay(attempt: number): number {
  return Math.min(MAX_DELAY, 1000 * 2 ** Math.max(0, attempt));
}

function decodeRaw(raw: string): ArrayBuffer {
  const binary = atob(raw);
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index);
  return bytes.buffer;
}

export class RemoteTransport implements Transport {
  private socket: RemoteSocket | null = null;
  private opening: Promise<void> | null = null;
  private readonly pending = new Map<number, Pending>();
  private readonly handlers = new Map<string, Set<(payload: unknown) => void>>();
  private nextId = 1;
  private attempt = 0;
  private retryScheduled = false;
  private state: RemoteConnection = "idle";
  private disposed = false;
  private readonly options: RemoteTransportOptions;

  constructor(options: RemoteTransportOptions) { this.options = options; }

  get connection(): RemoteConnection { return this.state; }

  async invoke<T>(command: string, args?: Record<string, unknown>): Promise<T> {
    await this.connect();
    if (this.socket?.readyState !== OPEN) throw new Error(LOST);
    const id = this.nextId++;
    const reply = new Promise<unknown>((resolve, reject) => this.pending.set(id, { resolve, reject }));
    const timer = setTimeout(() => {
      const waiting = this.pending.get(id);
      if (!waiting) return;
      this.pending.delete(id);
      waiting.reject(new Error(TIMED_OUT));
    }, invokeTimeout(command));
    (timer as { unref?: () => void }).unref?.();
    this.socket!.send(JSON.stringify({ type: "invoke", id, cmd: command, args: args ?? {} }));
    return (reply as Promise<T>).finally(() => clearTimeout(timer));
  }

  async listen<T>(event: string, handler: (payload: T) => void): Promise<Unlisten> {
    let set = this.handlers.get(event);
    const first = !set;
    if (!set) { set = new Set(); this.handlers.set(event, set); }
    const own = handler as (payload: unknown) => void;
    set.add(own);
    if (first && this.state === "open") this.send({ type: "listen", event });
    void this.connect().catch(() => undefined);
    return () => {
      const current = this.handlers.get(event);
      if (!current?.delete(own) || current.size) return;
      this.handlers.delete(event);
      if (this.state === "open") this.send({ type: "unlisten", event });
    };
  }

  /** An `<img>` cannot send a header: the device token travels in the query of a same-origin URL. */
  thumbnailUrl(id: string): string | undefined {
    const token = this.options.token();
    return token ? `${this.options.baseUrl ?? ""}/api/thumbnail/${encodeURIComponent(id)}?token=${encodeURIComponent(token)}` : undefined;
  }

  /** Device streams (ADR-128) go through this same origin, behind the device token. */
  deviceHubAccess(): DeviceHubAccess | null {
    const token = this.options.token();
    if (!token || typeof location === "undefined") return null;
    const base = new URL(this.options.baseUrl || location.origin);
    const ws = base.protocol === "https:" ? "wss:" : "ws:";
    return { httpBase: `${base.origin}/api/device-hub`, wsBase: `${ws}//${base.host}/api/device-hub`, token };
  }

  /**
   * The page came back to the front (an iPhone app resumed): reconnect now instead of
   * waiting for the backoff timer.
   */
  wake() {
    if (this.state !== "lost" || !this.handlers.size) return;
    this.attempt = 0;
    void this.connect().catch(() => undefined);
  }

  /** A computer forgotten on this phone (ADR-132): no more reconnects, and the socket closes. */
  dispose() {
    this.disposed = true;
    this.handlers.clear();
    const socket = this.socket;
    this.socket = null;
    this.opening = null;
    for (const [, waiting] of this.pending) waiting.reject(new Error(LOST));
    this.pending.clear();
    socket?.close();
  }

  private send(message: object) {
    if (this.socket?.readyState === OPEN) this.socket.send(JSON.stringify(message));
  }

  private setState(next: RemoteConnection) {
    const previous = this.state;
    if (previous === next) return;
    this.state = next;
    this.options.onConnection?.(next, previous);
  }

  private connect(): Promise<void> {
    if (this.disposed) return Promise.reject(new Error(LOST));
    if (this.state === "open") return Promise.resolve();
    if (this.state === "unauthorized") return Promise.reject(new Error("This device is no longer connected to the Mac. Pair it again."));
    if (this.opening) return this.opening;
    const token = this.options.token();
    if (!token) return Promise.reject(new Error("This device is not paired with the Mac yet."));
    this.setState("connecting");
    const socket = (this.options.createSocket ?? ((url) => new WebSocket(url) as unknown as RemoteSocket))(this.options.url);
    this.socket = socket;
    this.opening = new Promise<void>((resolve, reject) => {
      let settled = false;
      // Large replies come gzipped when this runtime can inflate them (ADR-164).
      const gzip = typeof DecompressionStream !== "undefined";
      if (gzip) socket.binaryType = "arraybuffer";
      socket.onopen = () => socket.send(JSON.stringify({ type: "hello", token, ...(gzip ? { gzip: true } : {}) }));
      const handle = (data: string) => {
        let message: Message;
        try { message = JSON.parse(data) as Message; } catch { return; }
        if (message.type === "ready") {
          settled = true;
          this.opening = null;
          this.attempt = 0;
          this.setState("open");
          for (const name of this.handlers.keys()) this.send({ type: "listen", event: name });
          resolve();
        } else if (message.type === "unauthorized") {
          this.setState("unauthorized");
          if (!settled) { settled = true; reject(new Error("This device is no longer connected to the Mac. Pair it again.")); }
          socket.close();
        } else this.receive(message);
      };
      // Inflating is asynchronous: while a frame inflates, later ones wait behind it, so
      // results and events keep the order they were sent in.
      let inbox: Promise<void> = Promise.resolve();
      let waiting = 0;
      socket.onmessage = (event) => {
        if (typeof event.data === "string" && waiting === 0) { handle(event.data); return; }
        waiting += 1;
        inbox = inbox.then(async () => {
          try { handle(typeof event.data === "string" ? event.data : await inflate(event.data)); } catch { /* a frame that does not inflate is dropped, like bad JSON */ } finally { waiting -= 1; }
        });
      };
      socket.onerror = () => undefined;
      socket.onclose = () => {
        if (this.socket !== socket) return;
        this.socket = null;
        this.opening = null;
        for (const [, waiting] of this.pending) waiting.reject(new Error(LOST));
        this.pending.clear();
        if (!settled) { settled = true; reject(new Error(UNREACHABLE)); }
        if (this.state === "unauthorized") return;
        this.setState("lost");
        this.scheduleRetry();
      };
    });
    return this.opening;
  }

  private scheduleRetry() {
    if (this.retryScheduled || !this.handlers.size) return;
    this.retryScheduled = true;
    const delay = reconnectDelay(this.attempt++);
    (this.options.setTimer ?? setTimeout)(() => {
      this.retryScheduled = false;
      void this.connect().catch(() => undefined);
    }, delay);
  }

  private receive(message: Message) {
    if (message.type === "result") {
      const waiting = this.pending.get(message.id);
      if (!waiting) return;
      this.pending.delete(message.id);
      if (message.ok) waiting.resolve(message.raw !== undefined ? decodeRaw(message.raw) : message.value);
      else waiting.reject(new Error(formatUnknownError(message.error), { cause: message.error }));
    } else if (message.type === "event") {
      for (const handler of [...(this.handlers.get(message.event) ?? [])]) handler(message.payload);
    }
  }
}

/** A gzipped binary frame (ADR-164) as text. */
export async function inflate(data: unknown): Promise<string> {
  const blob = data instanceof Blob ? data : new Blob([data as ArrayBuffer]);
  return new Response(blob.stream().pipeThrough(new DecompressionStream("gzip"))).text();
}
