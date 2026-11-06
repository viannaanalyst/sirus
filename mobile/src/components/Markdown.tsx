import { useMemo } from "react";
import { Linking, Platform, StyleSheet, View } from "react-native";
import { EnrichedMarkdownText, type MarkdownStyle } from "react-native-enriched-markdown";

import { fileReference } from "@sirus/lib/chat-markdown";

import { useAppearancePreferences } from "../features/settings/appearance/AppearancePreferencesProvider";
import { resolveNativeMarkdownTypography } from "../lib/appearancePreferences";
import { useUniwindTheme } from "../lib/useUniwindTheme";

const MONOSPACE = Platform.OS === "ios" ? "Menlo" : "monospace";
const FILE_SCHEME = "sirus-file:";

/**
 * Inline code naming a file of the conversation's folder becomes a link, as the desktop's file
 * references (`fileReference`): blue, and a tap opens the file. Fenced code stays as written.
 */
export function linkFileReferences(markdown: string, cwd: string): string {
  return markdown.split(/(^ {0,3}(?:```|~~~)[^\n]*\n[\s\S]*?(?:^ {0,3}(?:```|~~~)[ \t]*$|(?![\s\S])))/m).map((part, index) => index % 2 ? part
    : part.replace(/(?<!`)`([^`\n]+)`(?!`)/g, (code, text: string) => {
      const reference = fileReference(text, cwd);
      return reference ? `[${text.replace(/([\\[\]*_<>])/g, "\\$1")}](${FILE_SCHEME}${encodeURIComponent(reference.path)})` : code;
    })).join("");
}

/**
 * A reply's markdown drawn natively: selectable
 * text, GitHub flavor, code blocks with syntax colors, tables and task lists, in the theme's
 * markdown colors and the phone's own font.
 */
export function Markdown(props: {
  readonly markdown: string;
  readonly onLinkPress?: (url: string) => void;
  /** The conversation's folder and how to open one of its files, for file references. */
  readonly files?: { readonly cwd: string; readonly open: (path: string) => void };
}) {
  const theme = useUniwindTheme();
  const { appearance, themeAppearance } = useAppearancePreferences();
  const markdownStyle = useMemo<MarkdownStyle>(() => {
    const typography = resolveNativeMarkdownTypography(appearance.baseFontSize);
    const body = { color: theme["--color-md-body"], fontFamily: "System", fontWeight: "400", fontSize: typography.fontSize, lineHeight: Math.round(typography.fontSize * 1.5), marginTop: 0, marginBottom: 12 };
    // A modest radius: iOS clips square table-cell borders without stroking the corner arcs.
    const block = { marginTop: 4, marginBottom: 12, borderRadius: 8, borderWidth: StyleSheet.hairlineWidth, borderColor: theme["--color-md-hr"] };
    const heading = (level: number) => {
      // the two top levels a little larger, the rest bold lines of body size.
      const fontSize = level <= 2 ? typography.fontSize + 2 : typography.fontSize;
      return { ...body, color: theme["--color-md-strong"], fontFamily: "System", fontWeight: "600", fontSize, lineHeight: Math.round(fontSize * 1.45), marginTop: 4 };
    };
    return {
      paragraph: body,
      h1: heading(1), h2: heading(2), h3: heading(3), h4: heading(4), h5: heading(5), h6: heading(6),
      strong: { color: theme["--color-md-strong"], fontFamily: "System", fontWeight: "bold" },
      link: { color: theme["--color-md-link"], underline: false },
      code: { fontFamily: MONOSPACE, color: theme["--color-foreground-secondary"], backgroundColor: "transparent", borderColor: "transparent", fontSize: typography.fontSize * 0.9 },
      codeBlock: {
        ...body, ...block, padding: 12,
        color: theme["--color-md-code-text"], backgroundColor: theme["--color-md-code-bg"], fontFamily: MONOSPACE, fontSize: typography.fontSize * 0.9,
        syntaxColors: themeAppearance === "dark"
          ? { keyword: "#ff7b72", string: "#a5d6ff", number: "#79c0ff", constant: "#79c0ff", comment: "#8b949e", function: "#d2a8ff", type: "#ffa657", property: "#79c0ff", tag: "#7ee787", attribute: "#79c0ff" }
          : { keyword: "#cf222e", string: "#0a3069", number: "#0550ae", constant: "#0550ae", comment: "#6e7781", function: "#8250df", type: "#953800", property: "#0550ae", tag: "#116329", attribute: "#0550ae" },
      },
      blockquote: { ...body, color: theme["--color-foreground-muted"], backgroundColor: "transparent", borderColor: theme["--color-md-blockquote-border"], borderWidth: 2, gapWidth: 12 },
      list: { ...body, bulletColor: theme["--color-foreground-muted"], markerColor: theme["--color-foreground-muted"], gapWidth: 6 },
      thematicBreak: { color: theme["--color-md-hr"], height: 1, marginTop: 8, marginBottom: 8 },
      table: {
        ...body, ...block,
        headerFontFamily: "System", headerTextColor: theme["--color-md-strong"], headerBackgroundColor: theme["--color-md-code-bg"],
        rowEvenBackgroundColor: "transparent", rowOddBackgroundColor: "transparent", cellPaddingHorizontal: 10, cellPaddingVertical: 7,
      },
      taskList: { checkedColor: theme["--color-md-link"], checkedTextColor: theme["--color-md-body"], borderColor: theme["--color-foreground-muted"], checkedStrikethrough: false },
    };
  }, [theme, appearance.baseFontSize, themeAppearance]);
  const { files } = props;
  const markdown = useMemo(() => files ? linkFileReferences(props.markdown, files.cwd) : props.markdown, [props.markdown, files]);
  const open = (url: string) => {
    if (url.startsWith(FILE_SCHEME)) { files?.open(decodeURIComponent(url.slice(FILE_SCHEME.length))); return; }
    if (props.onLinkPress) props.onLinkPress(url);
    else void Linking.openURL(url).catch(() => undefined);
  };
  return <View style={{ flexShrink: 1, minWidth: 0 }}>
    <EnrichedMarkdownText
      markdown={markdown}
      markdownStyle={markdownStyle}
      containerStyle={{ flexShrink: 1, minWidth: 0 }}
      flavor="github"
      selectable
      md4cFlags={{ latexMath: false, hardSoftBreaks: false, admonitions: true }}
      enableTaskListItemToggle={false}
      // a code block's button switches between scrolling and wrapped lines.
      enableCodeBlockWrapToggle
      onLinkPress={({ url }) => open(url)}
    />
  </View>;
}
