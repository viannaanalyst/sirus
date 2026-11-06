import { useEffect, useMemo, useState } from "react";

import type { SkillActionResponse, SkillOwner, SkillsCatalog, WorkspaceFiles } from "@sirus/client/types";
import { completeComposerToken, composerTrigger, type ComposerTrigger } from "@sirus/lib/composer-suggestions";
import { filterSkills } from "@sirus/lib/skills";

import { useSirus } from "../../data/store";

export interface Suggestion {
  readonly key: string;
  /** `/review` or the file's name (a folder ends in `/`). */
  readonly label: string;
  readonly description: string;
  /** What goes into the draft: the skill's name or the file's path. */
  readonly value: string;
  readonly path?: string;
  readonly directory?: boolean;
}

export interface Selection { readonly start: number; readonly end: number }

/** The most rows a list shows; the host caps file hits itself and says when it did. */
const MAX_ROWS = 40;

/** Skill catalogs by owner, so reopening `/` shows the last list at once while a fresh one loads. */
const catalogs = new Map<string, SkillsCatalog>();

const ownerKey = (computerId: string, owner: SkillOwner) => `${computerId}:${owner.projectId ?? ""}:${owner.sessionId ?? ""}`;
const messageOf = (reason: unknown) => reason instanceof Error ? reason.message : String(reason);

/**
 * The `/` and `@` lists of the composer, with the Mac's sources: `/` lists the owner's skills
 * (`skill_action` catalog), `@` searches the project's files (`workspace_files`) while the
 * person types. `owner` is the conversation (or, before one exists, the project); with none,
 * only `/` works. Choosing a row returns the draft with the token completed.
 */
export function useComposerSuggestions(args: {
  readonly computerId: string | undefined;
  readonly owner: SkillOwner | null;
  readonly text: string;
  readonly selection: Selection;
  readonly focused: boolean;
}) {
  const { computerId, owner, text, selection, focused } = args;
  const disabled = useSirus((state) => (computerId ? state.data[computerId]?.settings?.disabledSkills : undefined)) ?? NONE;
  const [dismissed, setDismissed] = useState("");
  const [attempt, setAttempt] = useState(0);
  const trigger: ComposerTrigger | null = useMemo(() => focused ? composerTrigger(text, selection.start, selection.end) : null, [focused, text, selection.start, selection.end]);
  const identity = `${text}|${selection.start}`;
  const visible = !!trigger && identity !== dismissed;
  const kind = visible ? trigger.kind : null;
  const query = visible ? trigger.query : "";
  const key = computerId && owner ? ownerKey(computerId, owner) : "";

  const [catalog, setCatalog] = useState<SkillsCatalog | undefined>(() => catalogs.get(key));
  const [skillError, setSkillError] = useState<string | null>(null);
  useEffect(() => {
    if (kind !== "skill" || !computerId || !owner) return;
    let live = true;
    setCatalog(catalogs.get(key));
    setSkillError(null);
    useSirus.getState().invoke<SkillActionResponse>(computerId, "skill_action", { action: { type: "catalog", owner } }).then((result) => {
      if (result.type !== "catalog") return;
      catalogs.set(key, result.catalog);
      if (live) setCatalog(result.catalog);
    }, (reason) => { if (live) setSkillError(messageOf(reason)); });
    return () => { live = false; };
  }, [kind, key, attempt, computerId, owner]);

  const [files, setFiles] = useState<{ readonly key: string; readonly data: WorkspaceFiles | null; readonly error: string | null }>({ key: "", data: null, error: null });
  const fileRequest = `${key}|${query}|${attempt}`;
  useEffect(() => {
    if (kind !== "file" || !computerId || !owner || (!owner.projectId && !owner.sessionId)) return;
    let live = true;
    // Edit-driven debounce: a search per pause in typing, never a poll or an index.
    const timer = setTimeout(() => {
      useSirus.getState().invoke<WorkspaceFiles>(computerId, "workspace_files", { owner, query }).then(
        (data) => { if (live) setFiles({ key: fileRequest, data, error: null }); },
        (reason) => { if (live) setFiles({ key: fileRequest, data: null, error: messageOf(reason) }); },
      );
    }, 120);
    return () => { live = false; clearTimeout(timer); };
  }, [kind, query, key, attempt, computerId, owner, fileRequest]);

  const rows = useMemo<Suggestion[]>(() => {
    if (kind === "skill") {
      return filterSkills(catalog?.skills ?? [], query, disabled, true).slice(0, MAX_ROWS)
        .map((skill) => ({ key: skill.name, label: `/${skill.name}`, description: skill.description, value: skill.name }));
    }
    if (kind === "file" && files.key === fileRequest) {
      return (files.data?.entries ?? []).slice(0, MAX_ROWS).map((entry) => ({
        key: entry.path, label: entry.path.split("/").at(-1)! + (entry.isDir ? "/" : ""), value: entry.path, path: entry.path, directory: entry.isDir,
        description: entry.path.includes("/") ? entry.path.slice(0, entry.path.lastIndexOf("/")) : "",
      }));
    }
    return [];
  }, [kind, catalog, query, disabled, files, fileRequest]);

  const available = kind === "file" ? !!owner && !!(owner.projectId || owner.sessionId) : true;
  const loading = kind === "skill" ? !catalog && !skillError : kind === "file" ? available && files.key !== fileRequest : false;
  const error = kind === "skill" ? (catalog ? null : skillError) : kind === "file" && files.key === fileRequest ? files.error : null;
  const truncated = kind === "skill" ? catalog?.truncated ?? false : files.key === fileRequest ? files.data?.truncated ?? false : false;

  return {
    visible: visible && available,
    kind,
    rows,
    loading,
    error,
    truncated,
    /** The draft and caret after choosing `row`. */
    choose: (row: Suggestion): { value: string; caret: number; keepOpen: boolean } | null => {
      if (!trigger) return null;
      const next = completeComposerToken(text, trigger, row.value, row.directory);
      return { ...next, keepOpen: !!row.directory };
    },
    /** Hides the list until the draft or caret changes. */
    dismiss: () => setDismissed(identity),
    /** After a choice that finishes the token, so the list does not reopen on it. */
    settle: (value: string, caret: number) => setDismissed(`${value}|${caret}`),
    retry: () => setAttempt((count) => count + 1),
  };
}

const NONE: readonly string[] = [];
