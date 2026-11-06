import { Linking } from "react-native";

import { composerSegments } from "@sirus/lib/composer-tokens";
import { splitLinks } from "@sirus/lib/link-text";

import { useUniwindTheme } from "../lib/useUniwindTheme";
import { AppText } from "./AppText";

/**
 * A message the person sent, as the desktop shows it: web addresses are blue links, and
 * `/command` and `@file` tokens are highlighted (`composerSegments`).
 */
export function UserText(props: { readonly text: string; readonly onLines?: (lines: number) => void }) {
  const link = useUniwindTheme()["--color-md-link"];
  // The token's tinted background, as the desktop's `composer-token`.
  const tint = /^#[0-9a-f]{6}$/i.test(link) ? `${link}24` : "transparent";
  return <AppText selectable className="text-base" style={{ fontSize: 16, lineHeight: 23 }} onTextLayout={props.onLines ? (event) => props.onLines?.(event.nativeEvent.lines.length) : undefined}>
    {composerSegments(props.text).map((segment, index) => segment.kind === "text"
      ? splitLinks(segment.text).map((part, at) => part.kind === "text" ? part.text
        : <AppText key={`${index}:${at}`} className="text-base" style={{ color: link }} onPress={() => void Linking.openURL(part.url).catch(() => undefined)}>{part.text}</AppText>)
      : <AppText key={index} className="text-base" style={{ color: link, backgroundColor: tint }}>{segment.text}</AppText>)}
  </AppText>;
}
