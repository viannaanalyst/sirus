import { forwardRef, useImperativeHandle, useMemo, useRef, type ReactNode } from "react";
import type { TextInputSelectionChangeEvent } from "react-native";

import { composerSegments } from "@sirus/lib/composer-tokens";

import { useUniwindTheme } from "../../lib/useUniwindTheme";
import { AppText, AppTextInput } from "../AppText";
import type { Selection } from "./useComposerSuggestions";

/** The colours of a `/skill` or `@file` token, in the field and in the sent bubble (the desktop's `composer-token`). */
export function useTokenColors() {
  const link = useUniwindTheme()["--color-md-link"];
  return { color: link, backgroundColor: /^#[0-9a-f]{6}$/i.test(link) ? `${link}24` : "transparent" };
}

export interface ComposerFieldHandle { focus: () => void }

/**
 * The composer's text field with its `/skill` and `@file` tokens highlighted as the Mac's does.
 * The text is the field's children (styled runs), so it stays one native text input: the draft
 * and the caret live in the parent; `caret` moves the caret after a suggestion is inserted.
 */
export const ComposerField = forwardRef<ComposerFieldHandle, {
  readonly value: string;
  readonly onChangeText: (text: string) => void;
  readonly onSelection: (selection: Selection) => void;
  /** A caret to place (after inserting a suggestion); the field forgets it on the next edit. */
  readonly caret: Selection | null;
  readonly onFocus?: () => void;
  readonly onBlur?: () => void;
  readonly placeholder: string;
  readonly className: string;
  readonly autoFocus?: boolean;
  readonly submit?: () => void;
}>(function ComposerField(props, ref) {
  const input = useRef<{ focus: () => void }>(null);
  useImperativeHandle(ref, () => ({ focus: () => input.current?.focus() }), []);
  const tokens = useTokenColors();
  const runs = useMemo<ReactNode[]>(() => composerSegments(props.value).map((segment, index) => segment.kind === "text"
    ? segment.text
    : <AppText key={index} style={{ color: tokens.color, backgroundColor: tokens.backgroundColor }}>{segment.text}</AppText>), [props.value, tokens.color, tokens.backgroundColor]);
  const onSelectionChange = (event: TextInputSelectionChangeEvent) => props.onSelection(event.nativeEvent.selection);
  return <AppTextInput ref={input as never} multiline placeholder={props.placeholder} autoFocus={props.autoFocus}
    onChangeText={props.onChangeText} onSelectionChange={onSelectionChange} onFocus={props.onFocus} onBlur={props.onBlur}
    selection={props.caret ?? undefined} className={props.className}
    {...(props.submit ? { submitBehavior: "submit" as const, returnKeyType: "send" as const, onSubmitEditing: props.submit } : {})}>
    {runs}
  </AppTextInput>;
});
