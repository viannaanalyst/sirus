import type { PendingRequest, Session, SessionStatus } from "@sirus/client/types";

import { describeRequest } from "./dashboard";

/**
 * When does another conversation deserve a banner? Pure transition logic over snapshots of
 * every computer's conversations (no React, no store), so it is testable and cannot double
 * fire. A conversation fires when it starts waiting for a permission, starts asking a
 * question, finishes or fails; never in the first snapshot of a computer, never for the
 * conversation on screen, and never twice for the same ask.
 */
export type BannerKind = "approval" | "input" | "done" | "failed";

/** What the banner needs to know about one conversation at one moment. */
export interface Observed {
  readonly key: string;
  readonly computerId: string;
  readonly sessionId: string;
  readonly title: string;
  readonly projectName: string;
  readonly projectId: string;
  readonly sessionStatus: SessionStatus;
  /** The conversation's own ask (not a delegated session's). */
  readonly wait: "approval" | "input" | null;
  readonly request: PendingRequest | undefined;
  readonly preview: string | null;
  readonly error: string | null;
}

export interface BannerMessage { readonly text: string; readonly code: string | null }
export interface Banner {
  readonly id: string;
  readonly kind: BannerKind;
  readonly key: string;
  readonly computerId: string;
  readonly sessionId: string;
  readonly title: string;
  readonly projectName: string;
  readonly projectId: string;
  readonly message: BannerMessage;
  readonly request: PendingRequest | undefined;
  readonly at: number;
}

const ACTIVE = new Set<SessionStatus>(["starting", "running", "waiting"]);

/** A conversation's own wait: an approval or a question; a bare `waiting` status counts as an approval. */
export function ownWait(session: Pick<Session, "status" | "pendingRequests">): "approval" | "input" | null {
  const requests = session.pendingRequests ?? [];
  if (requests.some((request) => request.kind.type !== "userInput")) return "approval";
  if (requests.some((request) => request.kind.type === "userInput")) return "input";
  return session.status === "waiting" ? "approval" : null;
}

export function observe(computerId: string, session: Session, projectName: string): Observed {
  const requests = session.pendingRequests ?? [];
  const wait = ownWait(session);
  const request = wait === "input" ? requests.find((item) => item.kind.type === "userInput") : requests.find((item) => item.kind.type !== "userInput");
  return {
    key: `${computerId}:${session.id}`, computerId, sessionId: session.id, title: session.title || "Nova conversa", projectName, projectId: session.projectId,
    sessionStatus: session.status, wait, request, preview: session.lastReplyPreview ?? null, error: session.lastError,
  };
}

export interface TransitionState {
  /** The last observation of each conversation. */
  readonly previous: ReadonlyMap<string, Observed>;
  /** Computers whose first snapshot has been taken. */
  readonly seeded: ReadonlySet<string>;
  /** Ids already announced, so a reload or reconnect cannot announce them again. */
  readonly announced: ReadonlySet<string>;
}
export const INITIAL_TRANSITIONS: TransitionState = { previous: new Map(), seeded: new Set(), announced: new Set() };

export interface Snapshot {
  /** Per computer: its conversations now, or `null` while its data is unknown or settling (absorbed silently). */
  readonly computers: ReadonlyMap<string, readonly Observed[] | null>;
  /** The conversation on screen, which never gets a banner. */
  readonly onScreen: { readonly computerId: string; readonly sessionId: string } | null;
  readonly now: number;
}

const MAX_ANNOUNCED = 400;

function messageFor(kind: BannerKind, item: Observed): BannerMessage {
  if (kind === "approval") {
    if (!item.request) return { text: "Está esperando a sua permissão.", code: null };
    const ask = describeRequest(item.request);
    return { text: ask.text.charAt(0).toUpperCase() + ask.text.slice(1), code: ask.code };
  }
  if (kind === "input") {
    const kindOf = item.request?.kind;
    const question = kindOf?.type === "userInput" ? kindOf.questions[0]?.question : undefined;
    return { text: question ?? "Tem uma pergunta para você.", code: null };
  }
  if (kind === "failed") return { text: item.error?.trim() || "A conversa terminou com erro.", code: null };
  return { text: item.preview?.trim() || "Terminou o trabalho.", code: null };
}

const banner = (kind: BannerKind, item: Observed, id: string, now: number): Banner => ({
  id, kind, key: item.key, computerId: item.computerId, sessionId: item.sessionId, title: item.title, projectName: item.projectName, projectId: item.projectId,
  message: messageFor(kind, item), request: item.request, at: now,
});

/** Takes the next snapshot: the banners to show, and the new state. */
export function advance(state: TransitionState, snapshot: Snapshot): { readonly state: TransitionState; readonly fire: Banner[] } {
  const previous = new Map(state.previous);
  const seeded = new Set(state.seeded);
  const announced = new Set(state.announced);
  const fire: Banner[] = [];
  const announce = (candidate: Banner, item: Observed) => {
    if (announced.has(candidate.id)) return;
    announced.add(candidate.id);
    if (snapshot.onScreen?.computerId === item.computerId && snapshot.onScreen.sessionId === item.sessionId) return;
    fire.push(candidate);
  };
  for (const [computerId, list] of snapshot.computers) {
    if (!list) { seeded.delete(computerId); continue; }
    const first = !seeded.has(computerId);
    seeded.add(computerId);
    const alive = new Set<string>();
    for (const item of list) {
      alive.add(item.key);
      const before = previous.get(item.key);
      previous.set(item.key, item);
      if (first) continue; // the first snapshot only records what already was
      const askId = item.request?.requestId ?? "waiting";
      if (item.wait && (before?.wait !== item.wait || before.request?.requestId !== item.request?.requestId)) {
        announce(banner(item.wait, item, `${item.wait}:${item.key}:${askId}`, snapshot.now), item);
        continue;
      }
      if (!before || item.wait) continue; // a brand-new conversation only matters when it asks for something
      const wasActive = ACTIVE.has(before.sessionStatus);
      if (wasActive && item.sessionStatus === "failed") announce(banner("failed", item, `failed:${item.key}:${snapshot.now}`, snapshot.now), item);
      else if (wasActive && (item.sessionStatus === "completed" || item.sessionStatus === "idle")) announce(banner("done", item, `done:${item.key}:${snapshot.now}`, snapshot.now), item);
    }
    for (const key of [...previous.keys()]) if (key.startsWith(`${computerId}:`) && !alive.has(key)) previous.delete(key);
  }
  while (announced.size > MAX_ANNOUNCED) announced.delete(announced.values().next().value as string);
  return { state: { previous, seeded, announced }, fire };
}

/** Whether a banner still has something to show: asks vanish once answered (here or elsewhere). */
export function stillRelevant(shown: Banner, current: ReadonlyMap<string, Observed>): boolean {
  if (shown.kind !== "approval" && shown.kind !== "input") return true;
  const item = current.get(shown.key);
  if (!item || item.wait !== shown.kind) return false;
  return !shown.request || item.request?.requestId === shown.request.requestId;
}

/** Newest first; the stack keeps one banner per id and drops the oldest past `limit`. */
export function pushBanners(shown: readonly Banner[], fired: readonly Banner[], limit = 12): Banner[] {
  const ids = new Set(shown.map((item) => item.id));
  return [...shown, ...fired.filter((item) => !ids.has(item.id))].slice(-limit);
}

/** Finished banners dismiss themselves after this long. */
export const DONE_DISMISS_MS = 6_000;
