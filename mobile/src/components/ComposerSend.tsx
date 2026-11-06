import * as Haptics from "expo-haptics";
import { ExpoSpeechRecognitionModule, useSpeechRecognitionEvent } from "expo-speech-recognition";
import { useRef, useState } from "react";
import { ActivityIndicator, Alert, Pressable, View } from "react-native";

import { inkOn, useMacAccent } from "../data/mac-accent";
import { SymbolView } from "./AppSymbol";
import { ICON } from "../lib/icon-sizes";

/**
 * The composer's trailing controls: the microphone, and a round button filled with the Mac's
 * accent (ink chosen to contrast): send once there is something to send, stop while the agent
 * works and the field is empty. Dictation is the iPhone's own speech recognition: words land
 * in the field as they are spoken.
 */
export function ComposerSend(props: {
  readonly text: string;
  readonly onTextChange: (text: string) => void;
  readonly onSend: () => void;
  readonly sending?: boolean;
  /** The agent is working and the field is empty: offer stop instead. */
  readonly onStop?: () => void;
  readonly disabled?: boolean;
  /** Whether there is something to send (text by default; attachments count too). */
  readonly canSend?: boolean;
  /** The Mac's accent colour (`#rrggbb`), the fill of send and stop; the first computer's by default. */
  readonly accent?: string;
}) {
  const macAccent = useMacAccent();
  const accent = props.accent ?? macAccent;
  const [listening, setListening] = useState(false);
  // What was in the field when dictation began; the live words follow it.
  const base = useRef("");
  const join = (spoken: string) => [base.current.trimEnd(), spoken.trim()].filter(Boolean).join(" ");
  const ink = inkOn(accent);

  useSpeechRecognitionEvent("result", (event) => {
    const spoken = event.results[0]?.transcript ?? "";
    props.onTextChange(join(spoken));
  });
  useSpeechRecognitionEvent("end", () => setListening(false));
  useSpeechRecognitionEvent("error", (event) => {
    setListening(false);
    if (event.error !== "aborted" && event.error !== "no-speech") Alert.alert("Ditado", event.message || "Não foi possível ouvir agora.");
  });

  const startDictation = async () => {
    const permission = await ExpoSpeechRecognitionModule.requestPermissionsAsync();
    if (!permission.granted) {
      Alert.alert("Ditado", "Permita o microfone e o reconhecimento de fala em Ajustes → Sirus.");
      return;
    }
    base.current = props.text;
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    ExpoSpeechRecognitionModule.start({ lang: "pt-BR", interimResults: true, continuous: true, addsPunctuation: true });
    setListening(true);
  };
  const stopDictation = () => {
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    ExpoSpeechRecognitionModule.stop();
  };

  const hasText = props.text.trim().length > 0;
  const canSend = props.canSend ?? hasText;
  // the microphone and the send button are both always there; while the agent works
  // and nothing is typed, send becomes stop.
  const mic = listening
    ? <Pressable onPress={stopDictation} accessibilityLabel="Concluir ditado" hitSlop={6} className="size-9 items-center justify-center rounded-full bg-foreground active:opacity-70">
        <SymbolView name="waveform" size={17} weight="semibold" tintColorClassName="accent-screen" />
      </Pressable>
    : <Pressable onPress={() => void startDictation()} accessibilityLabel="Ditar" hitSlop={6} disabled={props.disabled}
        className="size-9 items-center justify-center rounded-full active:bg-row-hover">
        <SymbolView name="mic" size={ICON.composerMic} tintColorClassName="accent-foreground-muted" />
      </Pressable>;
  const action = !hasText && props.onStop && !listening
    ? <Pressable onPress={props.onStop} accessibilityLabel="Parar" hitSlop={6} className="size-[42px] items-center justify-center rounded-full active:opacity-70" style={{ backgroundColor: accent }}>
        <View className="size-3.5 rounded" style={{ backgroundColor: ink }} />
      </Pressable>
    : <Pressable onPress={props.onSend} disabled={!canSend || props.disabled || props.sending || listening} accessibilityLabel="Enviar" hitSlop={6}
        className="size-[42px] items-center justify-center rounded-full active:opacity-70" style={{ backgroundColor: accent, opacity: !canSend || props.disabled || listening ? 0.35 : 1 }}>
        {props.sending ? <ActivityIndicator color={ink} size="small" /> : <SymbolView name="arrow.up" size={ICON.composerSend} weight="bold" tintColor={ink} />}
      </Pressable>;
  return <View className="flex-row items-center gap-0.5">{mic}{action}</View>;
}
