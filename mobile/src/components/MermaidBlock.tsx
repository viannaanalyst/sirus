import * as Clipboard from "expo-clipboard";
import * as Haptics from "expo-haptics";
import { useEffect, useMemo, useState } from "react";
import { Modal, Platform, Pressable, ScrollView, StyleSheet, useWindowDimensions, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { SvgXml } from "react-native-svg";

import { translate } from "@sirus/i18n";

import { mermaidSvgForNative } from "../lib/mermaid-svg";
import { themeColorWithAlpha } from "../lib/mobileTheme";
import { useUniwindTheme } from "../lib/useUniwindTheme";
import { SymbolView } from "./AppSymbol";
import { AppText } from "./AppText";

const t = (key: string) => translate("pt-BR", key);
const MONOSPACE = Platform.OS === "ios" ? "Menlo" : "monospace";
// Pele measures labels with metrics close to Helvetica when no canvas is available.
const DIAGRAM_FONT = Platform.OS === "ios" ? "Helvetica" : "sans-serif";

type Diagram =
  | { readonly status: "ready"; readonly svg: string; readonly width: number; readonly height: number }
  | { readonly status: "error"; readonly message: string };

type Pele = typeof import("pele/lazy");
let peleLoad: Promise<Pele | null> | null = null;
// Each diagram type is its own chunk, so a reply with one flowchart only evaluates that parser.
const typeLoads = new Map<string, Promise<boolean>>();
// A reply scrolled back into view reuses its settled result instead of flashing the source.
const diagrams = new Map<string, Promise<Diagram>>();
const CACHE_LIMIT = 64;

function loadDiagram(source: string): Promise<Diagram> {
  let load = diagrams.get(source);
  if (!load) {
    if (diagrams.size >= CACHE_LIMIT) diagrams.delete(diagrams.keys().next().value!);
    load = drawDiagram(source);
    diagrams.set(source, load);
  }
  return load;
}

async function drawDiagram(source: string): Promise<Diagram> {
  const failed = (message: string): Diagram => { peleLoad = null; diagrams.delete(source); return { status: "error", message }; };
  peleLoad ??= import("pele/lazy").catch(() => null);
  const pele = await peleLoad;
  if (!pele) return failed("o desenhista de diagramas não carregou.");
  let type: ReturnType<Pele["detectType"]> = null;
  try { type = pele.detectType(source); } catch { /* an unknown type is reported by render */ }
  if (type) {
    let load = typeLoads.get(type);
    if (!load) { load = pele.load(type).then(() => true, () => false); typeLoads.set(type, load); }
    if (!(await load)) { typeLoads.delete(type); return failed("o desenhista de diagramas não carregou."); }
  }
  try {
    // The author's direction is kept: a wide diagram scales down instead of turning vertical.
    const { svg, width, height } = pele.render(source, { links: false, images: false, autoDirection: false });
    return { status: "ready", svg, width, height };
  } catch (reason) {
    return { status: "error", message: reason instanceof Error ? reason.message : "não foi possível desenhar." };
  }
}

/**
 * A closed ```mermaid fence drawn as its diagram (Pele turns the source into SVG on the phone, with
 * no web view), with the code block's header: the name, a switch back to the source and Copiar.
 * A tap opens the diagram full screen, to pinch and pan. Until it is drawn the source shows.
 */
export function MermaidBlock(props: { readonly code: string }) {
  const theme = useUniwindTheme();
  const [diagram, setDiagram] = useState<Diagram | null>(null);
  const [showCode, setShowCode] = useState(false);
  const [open, setOpen] = useState(false);
  const [copied, setCopied] = useState(false);
  const source = props.code.trim();
  useEffect(() => {
    let live = true;
    setDiagram(null);
    void loadDiagram(source).then((next) => live && setDiagram(next));
    return () => { live = false; };
  }, [source]);
  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1800);
    return () => clearTimeout(timer);
  }, [copied]);

  const ink = theme["--color-md-body"];
  const muted = themeColorWithAlpha(theme["--color-md-code-text"], 0.6);
  const svg = useMemo(() => diagram?.status === "ready" ? mermaidSvgForNative(diagram.svg, {
    "--pele-bg": theme["--color-md-code-bg"],
    "--pele-fg": ink,
    "--pele-muted": themeColorWithAlpha(ink, 0.65),
    "--pele-line": themeColorWithAlpha(ink, 0.55),
    "--pele-surface": themeColorWithAlpha(ink, 0.06),
    "--pele-surface-alt": themeColorWithAlpha(ink, 0.12),
    "--pele-border": themeColorWithAlpha(ink, 0.28),
    "--pele-accent": theme["--color-md-link"],
    "--pele-font": DIAGRAM_FONT,
    "--pele-font-mono": MONOSPACE,
  }) : null, [diagram, theme, ink]);

  const copy = async () => {
    try {
      await Clipboard.setStringAsync(props.code);
      setCopied(true);
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(() => undefined);
    } catch { /* the copy stays available */ }
  };
  const drawn = svg && diagram?.status === "ready" && !showCode ? diagram : null;
  return <View style={{ backgroundColor: theme["--color-md-code-bg"], borderColor: theme["--color-md-hr"], borderWidth: StyleSheet.hairlineWidth }} className="mb-3 mt-1 overflow-hidden rounded-lg">
    <View style={{ borderBottomColor: themeColorWithAlpha(theme["--color-md-code-text"], 0.2), borderBottomWidth: StyleSheet.hairlineWidth }} className="min-h-[36px] flex-row items-center justify-between pl-3">
      <AppText style={{ color: muted }} className="text-[12px]">{t("reply.mermaid")}</AppText>
      <View className="flex-row items-center">
        {svg ? <HeaderButton icon={showCode ? "photo" : "chevron.left.forwardslash.chevron.right"} label={showCode ? t("reply.showDiagram") : t("reply.showCode")} color={muted} onPress={() => setShowCode((value) => !value)} /> : null}
        <HeaderButton icon={copied ? "checkmark" : "doc.on.doc"} label={copied ? t("reply.copied") : t("reply.copyCode")} color={muted} onPress={() => void copy()} />
      </View>
    </View>
    <View className="p-3">
      {diagram?.status === "error" ? <AppText style={{ color: muted }} className="mb-2 text-[12px]">{`${t("reply.diagramFailed")} ${diagram.message}`}</AppText> : null}
      {drawn ? (
        // In a row the width resolves first, so the height follows the shrunk width.
        <View className="flex-row">
          <Pressable accessibilityRole="imagebutton" accessibilityLabel={t("reply.diagram")} accessibilityHint={t("reply.diagramOpen")}
            onPress={() => { void Haptics.selectionAsync().catch(() => undefined); setOpen(true); }}
            style={{ flexBasis: drawn.width, flexShrink: 1, aspectRatio: drawn.width / drawn.height }}>
            <SvgXml xml={svg!} width="100%" height="100%" />
          </Pressable>
        </View>
      ) : <AppText selectable style={{ color: theme["--color-md-code-text"], fontFamily: MONOSPACE, fontSize: 13, lineHeight: 19 }}>{props.code}</AppText>}
    </View>
    {open && drawn ? <DiagramViewer xml={svg!} width={drawn.width} height={drawn.height} background={theme["--color-md-code-bg"]} onClose={() => setOpen(false)} /> : null}
  </View>;
}

function HeaderButton(props: { readonly icon: "photo" | "chevron.left.forwardslash.chevron.right" | "checkmark" | "doc.on.doc"; readonly label: string; readonly color: string; readonly onPress: () => void }) {
  return <Pressable accessibilityRole="button" accessibilityLabel={props.label} onPress={props.onPress} hitSlop={4} className="size-9 items-center justify-center active:opacity-60">
    <SymbolView name={props.icon} size={14} tintColor={props.color} />
  </Pressable>;
}

/** The diagram alone on the code block's background: pinch to zoom, drag to pan, the button to leave. */
function DiagramViewer(props: { readonly xml: string; readonly width: number; readonly height: number; readonly background: string; readonly onClose: () => void }) {
  const { width, height } = useWindowDimensions();
  const insets = useSafeAreaInsets();
  const fit = Math.min((width - 24) / props.width, (height - insets.top - insets.bottom - 120) / props.height);
  const drawnWidth = props.width * fit;
  const drawnHeight = props.height * fit;
  return <Modal visible transparent animationType="fade" onRequestClose={props.onClose} statusBarTranslucent>
    <View style={{ flex: 1, backgroundColor: props.background }}>
      <ScrollView style={{ width, height }} contentContainerStyle={{ width, height, alignItems: "center", justifyContent: "center" }}
        maximumZoomScale={5} minimumZoomScale={1} centerContent showsHorizontalScrollIndicator={false} showsVerticalScrollIndicator={false} bouncesZoom>
        <SvgXml xml={props.xml} width={drawnWidth} height={drawnHeight} />
      </ScrollView>
      <View pointerEvents="box-none" style={[StyleSheet.absoluteFill, { top: insets.top + 8, left: 16, right: 16, bottom: undefined }]}>
        <Pressable onPress={props.onClose} accessibilityRole="button" accessibilityLabel={t("reply.closeDiagram")} hitSlop={8} className="size-11 items-center justify-center rounded-full bg-card active:opacity-60">
          <SymbolView name="xmark" size={18} weight="semibold" tintColorClassName="accent-foreground" />
        </Pressable>
      </View>
    </View>
  </Modal>;
}
