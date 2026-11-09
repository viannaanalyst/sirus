import { Alert, Modal, Pressable, View } from "react-native";
import Animated, { FadeIn, SlideInDown } from "react-native-reanimated";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { AgentProviderId } from "@sirus/client/types";
import { translate } from "@sirus/i18n";
import { setDebugging, setGoal, setPlanning, setTeam, supportsPlanning, supportsTeam, type ComposerModes } from "@sirus/lib/composer-modes";

import { useClipboardHasImage } from "../../data/attachments";
import { AppText } from "../AppText";
import { SymbolView } from "../AppSymbol";
import { SheetGroup, SheetRow } from "../SheetKit";
import type { Composer } from "./useComposer";

const t = (key: string) => translate("pt-BR", key);

/**
 * The + sheet, with the Mac's add-menu words and effects: photos and files, a skill, a goal, and
 * the planning, debug and team modes (active ones are checked and show as chips over the field).
 * "Anexar janela" is the Mac's own screen capture and has no phone counterpart.
 */
export function AddSheet(props: {
  readonly composer: Composer;
  readonly provider: AgentProviderId;
  readonly model: string | null;
  readonly busy?: boolean;
  readonly onPhotos: () => void;
  readonly onFiles: () => void;
  /** Attaches the clipboard's image; the row shows only while the clipboard holds one. */
  readonly onPaste?: () => void;
}) {
  const insets = useSafeAreaInsets();
  const { composer, provider, model } = props;
  const { modes } = composer;
  const clipboardImage = useClipboardHasImage(composer.sheet) && !!props.onPaste;
  const planningOk = supportsPlanning(provider, model);
  const teamOk = supportsTeam(provider);
  // iOS drops a picker presented while this sheet's modal is still on screen: wait for it to go.
  const run = (action: () => void) => () => { composer.closeSheet(); setTimeout(action, 450); };
  const mode = (update: (current: ComposerModes) => ComposerModes) => () => { composer.closeSheet(); composer.changeModes(update); };
  const check = (on: boolean) => on ? <SymbolView name="checkmark" size={15} weight="semibold" tintColorClassName="accent-foreground" /> : undefined;
  const editGoal = () => {
    composer.closeSheet();
    // Let the sheet go before the system alert comes up.
    setTimeout(() => Alert.prompt("Meta", "Defina uma meta a perseguir. Aplicada ao enviar e mantida nos seus próximos envios manuais.", [
      ...(modes.goal ? [{ text: "Remover", style: "destructive" as const, onPress: () => composer.changeModes((current) => setGoal(current, "")) }] : []),
      { text: "Cancelar", style: "cancel" as const },
      { text: "Salvar", onPress: (value?: string) => composer.changeModes((current) => setGoal(current, value ?? "")) },
    ], "plain-text", modes.goal), 300);
  };

  return <Modal visible={composer.sheet} transparent animationType="fade" onRequestClose={composer.closeSheet} statusBarTranslucent>
    <Animated.View entering={FadeIn.duration(160)} style={{ flex: 1, backgroundColor: "#000a", justifyContent: "flex-end" }}>
      <Pressable style={{ flex: 1 }} onPress={composer.closeSheet} accessibilityLabel="Fechar" accessibilityRole="button" />
      <Animated.View entering={SlideInDown.duration(260)} className="rounded-t-[30px] bg-sheet px-4 pt-3" style={{ paddingBottom: Math.max(insets.bottom, 16) + 8 }}>
        <View className="mb-2 h-1 w-9 self-center rounded-full bg-border-subtle" />
        <AppText className="mb-1.5 px-3 pt-1 text-[13px] text-foreground-muted">Adicionar</AppText>
        <SheetGroup>
          <SheetRow first icon="photo" title="Fotos" note="Da sua galeria" disabled={props.busy} onPress={run(props.onPhotos)} />
          {clipboardImage ? <SheetRow icon="doc.on.clipboard" title={t("composer.pasteImage")} note={t("composer.pasteImageNote")} disabled={props.busy} onPress={run(props.onPaste!)} /> : null}
          <SheetRow icon="folder" title="Arquivos" note="Escolher arquivos" disabled={props.busy} onPress={run(props.onFiles)} />
          <SheetRow icon="shippingbox" title="Usar uma skill" note="Adiciona /nome-da-skill ao rascunho" onPress={composer.startSkill} />
          <SheetRow icon="scope" title="Meta" note={modes.goal || "Defina uma meta a perseguir"} accessory={check(!!modes.goal)} onPress={editGoal} />
          <SheetRow icon="lightbulb" title="Modo planejamento" note={planningOk ? "Analisar e propor antes de editar" : "Indisponível para este agente ou modelo"}
            disabled={!planningOk} accessory={check(modes.planning)} onPress={mode((current) => setPlanning(current, !current.planning))} />
          <SheetRow icon="ladybug" title="Modo debug" note="Ativar modo debug" accessory={check(modes.debugging)} onPress={mode((current) => setDebugging(current, !current.debugging))} />
          <SheetRow icon="person.2" title="Equipe" note={teamOk ? "Vários agentes trabalhando juntos" : "Indisponível para este agente"}
            disabled={!teamOk} accessory={check(modes.team)} onPress={mode((current) => setTeam(current, !current.team))} />
        </SheetGroup>
      </Animated.View>
    </Animated.View>
  </Modal>;
}
