import { Linking } from "react-native";

import { composerSegments } from "@sirus/lib/composer-tokens";
import { splitLinks } from "@sirus/lib/link-text";

import { useMacAccent } from "../data/mac-accent";
import { useUniwindTheme } from "../lib/useUniwindTheme";
import { AppText } from "./AppText";

/**
 * A message the person sent, as the desktop shows it: web addresses are blue links, and
 * `/command` and `@file` tokens are highlighted (`composerSegments`).
 */
export function UserText(props: { readonly text: string; readonly onLines?: (lines: number) => void }) {
  const link = useUniwindTheme()["--color-md-link"];
  // Tokens take the Mac's accent, as its composer and bubbles paint them.
  const accent = useMacAccent();
  const tint = `${accent}2e`;
  return <AppText selectable className="text-base" style={{ fontSize: 16, lineHeight: 23 }} onTextLayout={props.onLines ? (event) => props.onLines?.(event.nativeEvent.lines.length) : undefined}>
    {composerSegments(props.text).map((segment, index) => segment.kind === "text"
      ? splitLinks(segment.text).map((part, at) => part.kind === "text" ? part.text
        : <AppText key={`${index}:${at}`} className="text-base" style={{ color: link }} onPress={() => void Linking.openURL(part.url).catch(() => undefined)}>{part.text}</AppText>)
      : <AppText key={index} className="text-base" style={{ color: accent, backgroundColor: tint }}>{segment.text}</AppText>)}
  </AppText>;
}
