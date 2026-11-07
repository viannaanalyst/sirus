import { create } from "zustand";

import { RemoteTransport, type RemoteConnection, type RemoteSocket } from "@sirus/client/remote-transport";
import { socketUrlFor } from "@sirus/client/remote-pairing";
import type { AgentEvent, AgentInstall, AgentProviderId, AgentResponse, AppData, AppSettings, ApprovalMode, ExecutionOptions, Message, PendingRequest, Project, ProviderModelList, Session, TranscriptResponse, TurnActivity } from "@sirus/client/types";
import { applyAgentOutput } from "@sirus/lib/agent-events";
import { applyRowSettingsAction } from "@sirus/lib/environments";
import { newRequestId } from "@sirus/lib/ids";
import type { PromptExtras } from "@sirus/lib/composer-modes";
import { isSnoozed } from "@sirus/lib/snooze";
import * as SecureStore from "expo-secure-store";

import { accessHeaders, readComputers, writeComputers, type Computer } from "./computers";
import { childRequests } from "./thread-status";

/**
 * The phone's view of every paired computer at once (ADR-135), as the desktop's phone UI merges
 * them (ADR-132): each computer keeps one socket, its projects and conversations stay fresh
 * through `session-updated`, and an open conversation's transcript streams through
 * `agent-output`. Nothing here needs a computer to be the "active" one.
 */
interface ComputerData {
  readonly projects: Project[];
  readonly sessions: Session[];
  readonly settings: AppSettings | null;
}

/** A conversation as the lists show it: which computer it is on, and its project. */
export interface SessionRow {
  readonly computerId: string;
  readonly session: Session;
  readonly project: Project | undefined;
  readonly pinned: boolean;
  /** A delegated session or team worker under it waits on the person. */
  readonly childRequest?: "approval" | "input";
}

/** A conversation's long-press actions, as the phone web app's (`rowMenuItems`). */
export type RowAction =
  | { type: "pin" | "unpin" }
  | { type: "rename"; title: string }
  | { type: "snooze"; until: string | null }
  | { type: "delete"; removeWorktree: boolean };

/** This iPhone's own preferences (not a computer's). */
export interface PhonePrefs {
  readonly threadOrder: "updated_at" | "created_at";
  readonly colorScheme: "system" | "light" | "dark";
  /** What Return does in the composer. */
  readonly returnKey: "send" | "newline";
  /** Appearance preferences (themes per appearance, text, terminal and code sizes). */
  readonly appearance: {
    readonly lightThemeId?: string;
    readonly darkThemeId?: string;
    readonly baseFontSize?: number;
    readonly terminalFontSize?: number | null;
    readonly codeFontSize?: number | null;
    readonly codeWordBreak?: boolean;
  };
}
/** A message written while the agent works; it goes once the reply ends (as the desktop's queue). */
export interface QueuedPrompt {
  readonly id: string;
  /** The send's id, the same on every try: a send whose answer was lost is not run twice. */
  readonly clientRequestId?: string;
  readonly prompt: string;
  readonly attachmentIds: string[];
  readonly execution?: ExecutionOptions;
  /** Goal, debug and team of the composer's modes, sent with the prompt. */
  readonly extras?: PromptExtras;
  /** Why the last try failed; it tries again the next time the conversation changes. */
  readonly failed?: string;
}

/** A conversation is busy while it starts, answers or waits on an approval. */
export const ACTIVE_STATUS = new Set(["starting", "running", "waiting"]);

const PREFS_KEY = "sirus.prefs";
const DEFAULT_PREFS: PhonePrefs = { threadOrder: "updated_at", colorScheme: "system", returnKey: "newline", appearance: {} };

interface SirusState {
  readonly ready: boolean;
  readonly computers: Computer[];
  readonly connection: Record<string, RemoteConnection>;
  readonly data: Record<string, ComputerData>;
  /** Loaded transcripts by `computerId:sessionId`: the newest page first, older pages on request. */
  readonly transcripts: Record<string, Message[]>;
  /** Index of each loaded transcript's first message in the whole conversation (0: all loaded). */
  readonly transcriptStart: Record<string, number>;
  /** Agent CLIs found on each computer. */
  readonly agents: Record<string, AgentInstall[]>;
  /** Model catalogs by `computerId:provider`. */
  readonly models: Record<string, ProviderModelList>;
  readonly prefs: PhonePrefs;
  /** Access chosen per provider in each conversation (`computerId:sessionId`), as the composer's. */
  readonly approvals: Record<string, Partial<Record<AgentProviderId, ApprovalMode>>>;
  /** Messages waiting for the reply to end, by `computerId:sessionId`; kept only while the app runs. */
  readonly queue: Record<string, QueuedPrompt[]>;
  /** Each conversation's latest turn (when it began, ended or waits, and its background work), by `computerId:sessionId`. */
  readonly turns: Record<string, TurnActivity>;
  start: () => Promise<void>;
  addComputer: (computer: Computer) => Promise<void>;
  forgetComputer: (id: string) => Promise<void>;
  loadTranscript: (computerId: string, sessionId: string) => Promise<void>;
  /** Loads the page before the oldest loaded message; false when nothing older remains. */
  loadOlder: (computerId: string, sessionId: string) => Promise<boolean>;
  /** One send; a retry of it passes the same `clientRequestId` (otherwise a new one is made). */
  sendPrompt: (computerId: string, sessionId: string, prompt: string, attachmentIds?: string[], execution?: ExecutionOptions, clientRequestId?: string, extras?: PromptExtras) => Promise<void>;
  stopAgent: (computerId: string, sessionId: string) => Promise<void>;
  /** Sends now, or queues it while the agent works. */
  submitPrompt: (computerId: string, sessionId: string, prompt: string, attachmentIds?: string[], execution?: ExecutionOptions, extras?: PromptExtras) => Promise<"sent" | "queued">;
  /** Drops a queued message; with `steer`, its text goes into the running reply instead. */
  unqueue: (computerId: string, sessionId: string, id: string, steer?: boolean) => Promise<void>;
  /** Sends the next queued message when the conversation is free. */
  drainQueue: (computerId: string, sessionId: string) => Promise<void>;
  /** Answers an agent's approval or question (the desktop's request cards). */
  respondRequest: (computerId: string, sessionId: string, request: PendingRequest, response: AgentResponse) => Promise<void>;
  loadAgents: (computerId: string) => Promise<void>;
  rowAction: (computerId: string, sessionId: string, action: RowAction) => Promise<void>;
  saveSettings: (computerId: string, patch: Partial<AppSettings>) => Promise<void>;
  setPrefs: (patch: Partial<PhonePrefs>) => void;
  setApproval: (computerId: string, sessionId: string, provider: AgentProviderId, mode: ApprovalMode) => void;
  /** The conversation's agent and model, as the desktop's model picker changes them. */
  setSessionModel: (computerId: string, sessionId: string, provider: AgentProviderId, model: string | null) => Promise<void>;
  loadModels: (computerId: string, provider: AgentProviderId) => Promise<void>;
  /** Starts a conversation in a project and sends its first message; resolves to the new session. */
  startSession: (computerId: string, request: { projectId: string; agent: AgentProviderId; model: string | null; isolatedWorktree: boolean; prompt: string; approval: ApprovalMode; planning?: boolean; extras?: PromptExtras; files?: { name: string; data: string }[] }) => Promise<Session>;
  invoke: <T>(computerId: string, command: string, args?: Record<string, unknown>) => Promise<T>;
  /** A computer's socket, for screens that follow their own events (the terminal). */
  transportFor: (computerId: string) => RemoteTransport | null;
}

const transports = new Map<string, RemoteTransport>();
const olderLoading = new Set<string>();
/** Messages in the first page of a conversation, and in each older page. */
const FIRST_PAGE = 40;
const OLDER_PAGE = 60;

/**
 * One page of a transcript, light (no command output, no review diffs): the newest when
 * `before` is absent. A computer on an older Sirus refuses the paging fields; then it sends
 * the whole transcript as before.
 */
async function loadPage(transport: RemoteTransport, sessionId: string, before?: number): Promise<TranscriptResponse> {
  try {
    return await transport.invoke<TranscriptResponse>("transcript_action", { action: { type: "load", sessionId, limit: before === undefined ? FIRST_PAGE : OLDER_PAGE, lite: true, ...(before === undefined ? {} : { before }) } });
  } catch (reason) {
    if (!/unknown field|invalid args/i.test(reason instanceof Error ? reason.message : String(reason))) throw reason;
    return transport.invoke<TranscriptResponse>("transcript_action", { action: { type: "load", sessionId } });
  }
}
const draining = new Set<string>();
const key = (computerId: string, sessionId: string) => `${computerId}:${sessionId}`;

export const useSirus = create<SirusState>()((set, get) => {
  const transportOf = (computer: Computer) => {
    const existing = transports.get(computer.id);
    if (existing) return existing;
    const transport = new RemoteTransport({
      url: socketUrlFor(computer.baseUrl),
      baseUrl: computer.baseUrl,
      // Behind Cloudflare Access the socket carries the service token too (React Native's
      // WebSocket takes headers as its third argument).
      createSocket: (url) => new (WebSocket as unknown as new (url: string, protocols: undefined, options: { headers: Record<string, string> }) => RemoteSocket)(url, undefined, { headers: accessHeaders(get().computers.find((item) => item.id === computer.id)?.access) }),
      token: () => get().computers.find((item) => item.id === computer.id)?.token ?? null,
      onConnection: (state, previous) => {
        set((current) => ({ connection: { ...current.connection, [computer.id]: state } }));
        // Back after a gap: what changed meanwhile loads again.
        if (state === "open" && previous !== "open") void load(computer);
      },
    });
    transports.set(computer.id, transport);
    void transport.listen<Session>("session-updated", (session) => set((current) => {
      const data = current.data[computer.id];
      if (!data) return {};
      const sessions = data.sessions.some((item) => item.id === session.id)
        ? data.sessions.map((item) => item.id === session.id ? { ...session, messages: item.messages } : item)
        : [session, ...data.sessions];
      const transcriptKey = key(computer.id, session.id);
      const transcript = current.transcripts[transcriptKey];
      // A session update carries its newest messages; a loaded transcript takes them.
      const transcripts = transcript && session.messages?.length ? { ...current.transcripts, [transcriptKey]: mergeMessages(transcript, session.messages) } : current.transcripts;
      // The list's status line reads the newest turn (its clock and background work).
      const turn = latestTurn(session.messages);
      const turns = turn ? { ...current.turns, [transcriptKey]: turn } : current.turns;
      return { data: { ...current.data, [computer.id]: { ...data, sessions } }, transcripts, turns };
    }));
    // The reply ended: the next queued message goes.
    void transport.listen<Session>("session-updated", (session) => {
      if (!ACTIVE_STATUS.has(session.status) && get().queue[key(computer.id, session.id)]?.length) void get().drainQueue(computer.id, session.id);
    });
    void transport.listen<AgentEvent>("agent-output", (event) => set((current) => {
      const transcriptKey = key(computer.id, event.sessionId);
      const transcript = current.transcripts[transcriptKey];
      if (!transcript) return {};
      const next = applyAgentOutput(transcript, event);
      return next === transcript ? {} : { transcripts: { ...current.transcripts, [transcriptKey]: next } };
    }));
    void transport.listen<AppSettings>("settings-changed", (settings) => set((current) => {
      const data = current.data[computer.id];
      return data ? { data: { ...current.data, [computer.id]: { ...data, settings } } } : {};
    }));
    return transport;
  };

  const load = async (computer: Computer) => {
    try {
      const state = await transportOf(computer).invoke<AppData>("load_state");
      set((current) => {
        const turns = { ...current.turns };
        for (const session of state.sessions) {
          const turn = latestTurn(session.messages);
          if (turn) turns[key(computer.id, session.id)] = turn;
        }
        return { data: { ...current.data, [computer.id]: { projects: state.projects, sessions: state.sessions, settings: state.settings } }, turns };
      });
    } catch { /* out of reach: the socket retries and loads again when it opens */ }
  };

  return {
    ready: false,
    computers: [],
    connection: {},
    data: {},
    transcripts: {},
    transcriptStart: {},
    agents: {},
    models: {},
    prefs: DEFAULT_PREFS,
    approvals: {},
    queue: {},
    turns: {},

    start: async () => {
      try {
        const raw = await SecureStore.getItemAsync(PREFS_KEY);
        if (raw) set({ prefs: { ...DEFAULT_PREFS, ...(JSON.parse(raw) as Partial<PhonePrefs>) } });
      } catch { /* defaults */ }
      const computers = await readComputers();
      set({ computers, ready: true });
      for (const computer of computers) void load(computer);
    },

    addComputer: async (computer) => {
      // Paired again at another address (its public one): the old address and token stay for
      // app updates, which the build fetches from the old address.
      const before = get().computers.find((item) => item.id === computer.id);
      const previous = before && before.baseUrl !== computer.baseUrl ? { baseUrl: before.baseUrl, token: before.token } : before?.previous;
      const next = previous ? { ...computer, previous } : computer;
      const computers = [...get().computers.filter((item) => item.id !== computer.id && item.baseUrl !== computer.baseUrl), next];
      await writeComputers(computers);
      transports.get(computer.id)?.dispose();
      transports.delete(computer.id);
      set({ computers });
      void load(next);
    },

    forgetComputer: async (id) => {
      const computers = get().computers.filter((item) => item.id !== id);
      await writeComputers(computers);
      transports.get(id)?.dispose();
      transports.delete(id);
      set((current) => {
        const data = { ...current.data };
        delete data[id];
        return { computers, data };
      });
    },

    loadTranscript: async (computerId, sessionId) => {
      const computer = get().computers.find((item) => item.id === computerId);
      if (!computer) return;
      const result = await loadPage(transportOf(computer), sessionId);
      if (result.type !== "transcript") return;
      const turn = latestTurn(result.messages);
      const sessionKey = key(computerId, sessionId);
      set((current) => ({
        transcripts: { ...current.transcripts, [sessionKey]: result.messages },
        transcriptStart: { ...current.transcriptStart, [sessionKey]: result.start ?? 0 },
        ...(turn ? { turns: { ...current.turns, [sessionKey]: turn } } : {}),
      }));
    },

    loadOlder: async (computerId, sessionId) => {
      const computer = get().computers.find((item) => item.id === computerId);
      const sessionKey = key(computerId, sessionId);
      const start = get().transcriptStart[sessionKey] ?? 0;
      if (!computer || start <= 0 || olderLoading.has(sessionKey)) return false;
      olderLoading.add(sessionKey);
      try {
        const result = await loadPage(transportOf(computer), sessionId, start);
        if (result.type !== "transcript") return false;
        set((current) => {
          const loaded = current.transcripts[sessionKey] ?? [];
          const known = new Set(loaded.map((message) => message.id));
          return {
            transcripts: { ...current.transcripts, [sessionKey]: [...result.messages.filter((message) => !known.has(message.id)), ...loaded] },
            transcriptStart: { ...current.transcriptStart, [sessionKey]: result.start ?? 0 },
          };
        });
        return (result.start ?? 0) > 0;
      } finally {
        olderLoading.delete(sessionKey);
      }
    },

    sendPrompt: async (computerId, sessionId, prompt, attachmentIds = [], execution, clientRequestId = newRequestId(), extras) => {
      await get().invoke<Session>(computerId, "send_prompt", { request: { sessionId, prompt: prompt.trim(), attachmentIds, attachmentOwner: `session:${sessionId}`, clientRequestId, ...(execution ? { execution } : {}), ...extras } });
    },

    stopAgent: async (computerId, sessionId) => {
      await get().invoke<void>(computerId, "stop_agent", { sessionId });
    },

    submitPrompt: async (computerId, sessionId, prompt, attachmentIds = [], execution, extras) => {
      const sessionKey = key(computerId, sessionId);
      // One id for this send, whether it goes now or after waiting in the queue.
      const clientRequestId = newRequestId();
      const enqueue = () => set((state) => ({ queue: { ...state.queue, [sessionKey]: [...(state.queue[sessionKey] ?? []), { id: `${Date.now()}-${Math.random().toString(36).slice(2, 8)}`, clientRequestId, prompt: prompt.trim(), attachmentIds, execution, extras }] } }));
      const session = get().data[computerId]?.sessions.find((item) => item.id === sessionId);
      // Behind a running reply, or behind messages already waiting, it waits its turn.
      if ((session && ACTIVE_STATUS.has(session.status)) || get().queue[sessionKey]?.length) {
        enqueue();
        if (session && !ACTIVE_STATUS.has(session.status)) void get().drainQueue(computerId, sessionId);
        return "queued";
      }
      try {
        await get().sendPrompt(computerId, sessionId, prompt, attachmentIds, execution, clientRequestId, extras);
        return "sent";
      } catch (reason) {
        // The list had not caught up with a reply that had just started.
        if (!/running agent/i.test(reason instanceof Error ? reason.message : String(reason))) throw reason;
        enqueue();
        return "queued";
      }
    },

    unqueue: async (computerId, sessionId, id, steer = false) => {
      const sessionKey = key(computerId, sessionId);
      const item = get().queue[sessionKey]?.find((entry) => entry.id === id);
      if (!item) return;
      if (steer) await get().invoke<void>(computerId, "steer_turn", { sessionId, text: item.prompt });
      set((state) => ({ queue: { ...state.queue, [sessionKey]: (state.queue[sessionKey] ?? []).filter((entry) => entry.id !== id) } }));
      if (!steer) void get().drainQueue(computerId, sessionId);
    },

    drainQueue: async (computerId, sessionId) => {
      const sessionKey = key(computerId, sessionId);
      const next = get().queue[sessionKey]?.[0];
      const session = get().data[computerId]?.sessions.find((item) => item.id === sessionId);
      if (!next || draining.has(sessionKey) || (session && ACTIVE_STATUS.has(session.status))) return;
      draining.add(sessionKey);
      const without = (items: QueuedPrompt[] | undefined) => (items ?? []).filter((entry) => entry.id !== next.id);
      try {
        await get().sendPrompt(computerId, sessionId, next.prompt, next.attachmentIds, next.execution, next.clientRequestId, next.extras);
        set((state) => ({ queue: { ...state.queue, [sessionKey]: without(state.queue[sessionKey]) } }));
      } catch (reason) {
        const message = reason instanceof Error ? reason.message : String(reason);
        // Still busy: it stays first and goes when this reply ends.
        if (!/running agent/i.test(message)) {
          set((state) => ({ queue: { ...state.queue, [sessionKey]: (state.queue[sessionKey] ?? []).map((entry) => entry.id === next.id ? { ...entry, failed: message } : entry) } }));
        }
      } finally {
        draining.delete(sessionKey);
      }
    },

    respondRequest: async (computerId, sessionId, request, response) => {
      await get().invoke<void>(computerId, "respond_agent_request", { request: { sessionId, requestId: request.requestId, generation: request.generation, turnId: request.turnId, response } });
      // The card goes at once; the next session update confirms it.
      set((state) => {
        const data = state.data[computerId];
        return data ? { data: { ...state.data, [computerId]: { ...data, sessions: data.sessions.map((item) => item.id === sessionId ? { ...item, pendingRequests: (item.pendingRequests ?? []).filter((entry) => entry.requestId !== request.requestId) } : item) } } } : {};
      });
    },

    setApproval: (computerId, sessionId, provider, mode) => set((state) => {
      const sessionKey = key(computerId, sessionId);
      return { approvals: { ...state.approvals, [sessionKey]: { ...state.approvals[sessionKey], [provider]: mode } } };
    }),

    setSessionModel: async (computerId, sessionId, provider, model) => {
      const session = await get().invoke<Session>(computerId, "set_session_model", { sessionId, agent: provider, model });
      set((state) => {
        const data = state.data[computerId];
        return data ? { data: { ...state.data, [computerId]: { ...data, sessions: data.sessions.map((item) => item.id === session.id ? { ...session, messages: item.messages } : item) } } } : {};
      });
    },

    setPrefs: (patch) => {
      const prefs = { ...get().prefs, ...patch };
      set({ prefs });
      void SecureStore.setItemAsync(PREFS_KEY, JSON.stringify(prefs)).catch(() => undefined);
    },

    saveSettings: async (computerId, patch) => {
      const current = get().data[computerId]?.settings;
      if (!current) throw new Error("Computador fora de alcance.");
      const saved = await get().invoke<AppSettings>(computerId, "save_settings", { settings: { ...current, ...patch } });
      set((state) => {
        const data = state.data[computerId];
        return data ? { data: { ...state.data, [computerId]: { ...data, settings: saved } } } : {};
      });
    },

    rowAction: async (computerId, sessionId, action) => {
      const data = get().data[computerId];
      if (!data?.settings) throw new Error("Computador fora de alcance.");
      const replace = (session: Session) => set((state) => {
        const current = state.data[computerId];
        return current ? { data: { ...state.data, [computerId]: { ...current, sessions: current.sessions.map((item) => item.id === session.id ? { ...session, messages: item.messages } : item) } } } : {};
      });
      switch (action.type) {
        case "pin": case "unpin": {
          const settings = applyRowSettingsAction(data.settings, action.type, sessionId);
          await get().saveSettings(computerId, settings);
          break;
        }
        case "rename": replace(await get().invoke<Session>(computerId, "rename_session", { sessionId, title: action.title.trim() })); break;
        case "snooze": replace(await get().invoke<Session>(computerId, "snooze_session", { sessionId, until: action.until })); break;
        case "delete":
          await get().invoke<void>(computerId, "delete_session", { sessionId, removeWorktree: action.removeWorktree, confirm: true });
          set((state) => {
            const current = state.data[computerId];
            return current ? { data: { ...state.data, [computerId]: { ...current, sessions: current.sessions.filter((item) => item.id !== sessionId) } } } : {};
          });
          break;
      }
    },

    loadAgents: async (computerId) => {
      const agents = await get().invoke<AgentInstall[]>(computerId, "detect_agents");
      set((current) => ({ agents: { ...current.agents, [computerId]: agents } }));
    },

    loadModels: async (computerId, provider) => {
      const list = await get().invoke<ProviderModelList>(computerId, "list_provider_models", { id: provider });
      set((current) => ({ models: { ...current.models, [`${computerId}:${provider}`]: list } }));
    },

    startSession: async (computerId, { prompt, approval, planning, extras, files = [], ...request }) => {
      const session = await get().invoke<Session>(computerId, "create_session", { request });
      set((current) => {
        const data = current.data[computerId];
        return data && !data.sessions.some((item) => item.id === session.id)
          ? { data: { ...current.data, [computerId]: { ...data, sessions: [session, ...data.sessions] } } } : {};
      });
      // Attachments picked before the conversation existed go to it now, then the first message.
      const owner = `session:${session.id}`;
      const attached = files.length ? await get().invoke<{ id: string }[]>(computerId, "paste_prompt_attachments", { owner, files: files.map(({ name, data }) => ({ name, data })), expectedText: null }) : [];
      await get().invoke<Session>(computerId, "send_prompt", { request: { sessionId: session.id, prompt: prompt.trim(), attachmentIds: attached.map((item) => item.id), attachmentOwner: owner, execution: { approval, ...(planning ? { planning: true } : {}) }, ...extras } });
      return session;
    },

    transportFor: (computerId) => {
      const computer = get().computers.find((item) => item.id === computerId);
      return computer ? transportOf(computer) : null;
    },

    invoke: async (computerId, command, args) => {
      const computer = get().computers.find((item) => item.id === computerId);
      if (!computer) throw new Error("Computador não pareado.");
      return transportOf(computer).invoke(command, args);
    },
  };
});

/** The newest agent turn among some messages. */
function latestTurn(messages: Message[] | undefined): TurnActivity | null {
  for (let index = (messages?.length ?? 0) - 1; index >= 0; index -= 1) {
    const activity = messages![index]!.activity;
    if (messages![index]!.role === "agent" && activity) return activity;
  }
  return null;
}

/** Newer snapshots of the same messages replace older ones; new ones are added in order. */
function mergeMessages(current: Message[], incoming: Message[]): Message[] {
  const byId = new Map(current.map((message, index) => [message.id, index]));
  const next = [...current];
  for (const message of incoming) {
    const at = byId.get(message.id);
    if (at === undefined) next.push(message);
    else if (message.content.length >= (next[at]?.content.length ?? 0) || !message.streaming) next[at] = message;
  }
  return next;
}

/**
 * Every conversation of every computer: pinned first, then by the chosen order; without
 * snoozed ones and without side chats or pull request Ask conversations.
 */
export function selectRows(state: Pick<SirusState, "data">, order: PhonePrefs["threadOrder"] = "updated_at"): SessionRow[] {
  const rows: SessionRow[] = [];
  for (const [computerId, data] of Object.entries(state.data)) {
    const pinned = new Set(data.settings?.pinnedSessionIds ?? []);
    const projects = new Map(data.projects.map((project) => [project.id, project]));
    const children = childRequests(data.sessions);
    for (const session of data.sessions) {
      if (session.sideChat || session.pullAsk || isSnoozed(session)) continue;
      rows.push({ computerId, session, project: projects.get(session.projectId), pinned: pinned.has(session.id), childRequest: children.get(session.id) });
    }
  }
  const key = (row: SessionRow) => order === "created_at" ? row.session.createdAt : row.session.lastActivityAt;
  return rows.sort((a, b) => Number(b.pinned) - Number(a.pinned) || key(b).localeCompare(key(a)));
}

export const transcriptKey = key;
