import { createContext, useContext, useEffect, useRef, type RefObject } from "react";
import type { ViewInstance } from "react-native";

/**
 * Where the current work sits on screen, for the "Trabalhando" pill to jump to. Rows that are
 * live (a running step, "Pensando") and team cards with subagents running register their view
 * while mounted; the thread asks for the lowest one (the end of the running tree). A virtualised
 * list keeps only nearby cells mounted, which always includes the newest turn.
 */
export interface WorkAnchor {
  readonly kind: "live" | "team";
  readonly ref: RefObject<ViewInstance | null>;
  /** A team card opens itself so the subagents are in view. */
  readonly expand?: () => void;
}

export interface WorkAnchors {
  add(anchor: WorkAnchor): () => void;
  all(): readonly WorkAnchor[];
}

export function createWorkAnchors(): WorkAnchors {
  const set = new Set<WorkAnchor>();
  return {
    add: (anchor) => { set.add(anchor); return () => { set.delete(anchor); }; },
    all: () => [...set],
  };
}

export const WorkAnchorsContext = createContext<WorkAnchors | null>(null);

/** Registers a view as a work anchor while `active`; returns the ref to put on the view. */
export function useWorkAnchor(kind: WorkAnchor["kind"], active: boolean, expand?: () => void) {
  const anchors = useContext(WorkAnchorsContext);
  const ref = useRef<ViewInstance>(null);
  const latest = useRef(expand);
  latest.current = expand;
  useEffect(() => {
    if (!anchors || !active) return;
    return anchors.add({ kind, ref, expand: () => latest.current?.() });
  }, [anchors, kind, active]);
  return ref;
}

/** Where a view of `height` should scroll so it is fully visible (bottom-aligned) or, if taller than the room, top-aligned. */
export function scrollTargetFor(input: { readonly y: number; readonly height: number; readonly viewport: number; readonly topPad: number; readonly bottomReserve: number; readonly contentHeight: number }): number {
  const room = input.viewport - input.topPad - input.bottomReserve;
  const wanted = input.height <= room ? input.y + input.height - (input.viewport - input.bottomReserve) : input.y - input.topPad;
  return Math.max(0, Math.min(wanted, Math.max(0, input.contentHeight - input.viewport)));
}
