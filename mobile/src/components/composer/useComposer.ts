import * as Haptics from "expo-haptics";
import { useCallback, useEffect, useRef, useState } from "react";

import type { AgentProviderId, SkillOwner } from "@sirus/client/types";
import { NO_MODES, type ComposerModes } from "@sirus/lib/composer-modes";

import type { ComposerFieldHandle } from "./ComposerField";
import { useComposerSuggestions, type Selection, type Suggestion } from "./useComposerSuggestions";

/**
 * What both composers (a conversation's and Novo's) share: the caret, the `/` and `@` lists, the
 * + sheet and the modes (goal, planning, debug, team). The caller owns the text and what a send
 * does; this owns how the field, the list and the sheet behave.
 */
export function useComposer(args: {
  readonly computerId: string | undefined;
  readonly owner: SkillOwner | null;
  readonly provider: AgentProviderId;
  readonly model: string | null;
  readonly text: string;
  readonly setText: (text: string) => void;
  /** A conversation's saved goal, taken once while the person has not set one. */
  readonly savedGoal?: string | null;
}) {
  const { text, setText } = args;
  const field = useRef<ComposerFieldHandle>(null);
  const [selection, setSelection] = useState<Selection>({ start: 0, end: 0 });
  const [caret, setCaret] = useState<Selection | null>(null);
  const [focused, setFocused] = useState(false);
  const blurTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined);
  const [modes, setModes] = useState<ComposerModes>({ ...NO_MODES, goal: args.savedGoal ?? "" });
  const [sheet, setSheet] = useState(false);
  const goalTouched = useRef(false);

  useEffect(() => {
    if (args.savedGoal && !goalTouched.current) setModes((current) => current.goal ? current : { ...current, goal: args.savedGoal ?? "" });
  }, [args.savedGoal]);
  useEffect(() => () => clearTimeout(blurTimer.current), []);

  const suggestions = useComposerSuggestions({ computerId: args.computerId, owner: args.owner, text, selection, focused });

  const onChangeText = useCallback((next: string) => { setCaret(null); setText(next); }, [setText]);
  const onFocus = useCallback(() => { clearTimeout(blurTimer.current); setFocused(true); }, []);
  // A short grace so a tap on a suggestion lands before the list goes away with the focus.
  const onBlur = useCallback(() => { blurTimer.current = setTimeout(() => setFocused(false), 160); }, []);

  const pick = (row: Suggestion) => {
    const result = suggestions.choose(row);
    if (!result) return;
    void Haptics.selectionAsync().catch(() => undefined);
    const at = { start: result.caret, end: result.caret };
    setText(result.value);
    setSelection(at);
    setCaret(at);
    if (!result.keepOpen) suggestions.settle(result.value, result.caret);
  };

  /** "Usar uma skill": opens the `/` list at the end of the draft, as typing `/` would. */
  const startSkill = () => {
    setSheet(false);
    const spaced = text && !/\s$/.test(text) ? `${text} ` : text;
    const next = `${spaced}/`;
    const at = { start: next.length, end: next.length };
    setText(next);
    setSelection(at);
    setCaret(at);
    setTimeout(() => field.current?.focus(), 350);
  };

  const changeModes = (update: (current: ComposerModes) => ComposerModes) => {
    goalTouched.current = true;
    void Haptics.selectionAsync().catch(() => undefined);
    setModes(update);
  };

  return { field, selection, setSelection, caret, onChangeText, onFocus, onBlur, suggestions, pick, startSkill, modes, setModes, changeModes, sheet, openSheet: () => setSheet(true), closeSheet: () => setSheet(false) };
}

export type Composer = ReturnType<typeof useComposer>;
