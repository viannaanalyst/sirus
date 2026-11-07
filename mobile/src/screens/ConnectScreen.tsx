import { CameraView, useCameraPermissions } from "expo-camera";
import * as Haptics from "expo-haptics";
import { useRef, useState } from "react";
import { ActivityIndicator, KeyboardAvoidingView, Linking, Pressable, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { SymbolView } from "../components/AppSymbol";

import { AppText, AppTextInput } from "../components/AppText";
import { GlassSurface } from "../components/GlassSurface";
import { pairComputer, pairingFromLink } from "../data/computers";
import { useSirus } from "../data/store";

type Mode = "start" | "scan" | "type";

/**
 * Connecting a computer (ADR-135, ADR-198): a button that opens the camera on the QR of the
 * computer's Settings → Connections, or its link or address and code typed in. More computers
 * are added later in Settings.
 */
export function ConnectScreen(props: { readonly title?: string; readonly onDone?: () => void } = {}) {
  const [mode, setMode] = useState<Mode>("start");
  const [address, setAddress] = useState("");
  const [code, setCode] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [permission, askPermission] = useCameraPermissions();
  // The camera reports a code many times a second; only the first one counts.
  const scanning = useRef(false);
  const insets = useSafeAreaInsets();

  const pasted = pairingFromLink(address);
  const ready = pasted !== null || (address.trim().length > 3 && code.replace(/\D/g, "").length === 6);

  const pair = async (place: string, digits: string, access?: Parameters<typeof pairComputer>[2]) => {
    setBusy(true);
    setError(null);
    try {
      await useSirus.getState().addComputer(await pairComputer(place, digits, access));
      void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success);
      props.onDone?.();
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
      setMode((now) => (now === "scan" ? "start" : now));
    } finally {
      setBusy(false);
    }
  };

  const openCamera = async () => {
    setError(null);
    const granted = permission?.granted || (await askPermission()).granted;
    if (!granted) {
      setError("O Sirus precisa da câmera para ler o QR. Libere em Ajustes do iPhone.");
      return;
    }
    scanning.current = false;
    setMode("scan");
  };

  const scanned = (data: string) => {
    if (scanning.current) return;
    const found = pairingFromLink(data);
    if (!found) {
      setError("Esse QR não é do Sirus.");
      return;
    }
    scanning.current = true;
    void pair(found.address, found.code, found.access);
  };

  if (mode === "scan") {
    return <View className="flex-1 bg-black">
      <CameraView
        style={{ flex: 1 }}
        facing="back"
        barcodeScannerSettings={{ barcodeTypes: ["qr"] }}
        onBarcodeScanned={busy ? undefined : ({ data }) => scanned(data)}
      />
      <View pointerEvents="box-none" style={{ position: "absolute", top: insets.top + 12, bottom: insets.bottom + 24, left: 0, right: 0, alignItems: "center", justifyContent: "space-between" }}>
        <View className="items-center gap-1">
          <AppText className="font-app-bold text-xl text-white">Escaneie o QR</AppText>
          <AppText className="text-sm text-white/70">Ajustes → Conexões, no Mac</AppText>
        </View>
        <View pointerEvents="none" style={{ width: 230, height: 230, borderRadius: 24, borderWidth: 3, borderColor: "white" }} />
        <View className="items-center gap-3">
          {busy ? <ActivityIndicator color="white" /> : null}
          {error ? <AppText className="text-center text-sm text-white">{error}</AppText> : null}
          <Pressable onPress={() => { setError(null); setMode("type"); }} className="h-11 justify-center rounded-full bg-white/20 px-5">
            <AppText className="font-app-medium text-sm text-white">Digitar código</AppText>
          </Pressable>
          <Pressable onPress={() => { setError(null); setMode("start"); }} className="h-9 justify-center px-4">
            <AppText className="text-sm text-white/80">Cancelar</AppText>
          </Pressable>
        </View>
      </View>
    </View>;
  }

  const denied = permission && !permission.granted && !permission.canAskAgain;
  return <View className="flex-1 bg-screen" style={{ paddingTop: insets.top, paddingBottom: insets.bottom }}>
    <KeyboardAvoidingView behavior="padding" style={{ flex: 1, justifyContent: "center", gap: 24, paddingHorizontal: 24 }}>
      <View className="items-center gap-3">
        <SymbolView name="desktopcomputer" size={44} tintColorClassName="accent-icon" />
        <AppText className="font-app-bold text-2xl">{props.title ?? "Conectar seu Mac"}</AppText>
        <AppText className="text-center text-sm text-foreground-muted">
          {mode === "start" ? "No Mac, abra Ajustes → Conexões e escaneie o QR." : "No Mac, toque em Copiar link e cole aqui, ou digite o endereço e o código."}
        </AppText>
      </View>
      {mode === "type" ? <View className="gap-3">
        <GlassSurface className="px-4 py-3" style={{ borderRadius: 18 }}>
          <AppTextInput value={address} onChangeText={setAddress} placeholder="Link ou endereço" autoCapitalize="none" autoCorrect={false} keyboardType="url" className="text-base" />
        </GlassSurface>
        {pasted ? null : <GlassSurface className="px-4 py-3" style={{ borderRadius: 18 }}>
          <AppTextInput value={code} onChangeText={setCode} placeholder="Código de 6 dígitos" keyboardType="number-pad" maxLength={7} className="text-base" />
        </GlassSurface>}
      </View> : null}
      {error ? <AppText className="text-center text-sm text-danger-foreground">{error}</AppText> : null}
      {mode === "start"
        ? <View className="items-center gap-3">
            <Pressable disabled={busy} onPress={() => void (denied ? Linking.openSettings() : openCamera())} className="h-12 w-full items-center justify-center rounded-full bg-foreground">
              {busy ? <ActivityIndicator /> : <AppText className="font-app-medium text-base text-screen">{denied ? "Abrir Ajustes" : "Escanear QR"}</AppText>}
            </Pressable>
            <Pressable onPress={() => { setError(null); setMode("type"); }} className="h-10 justify-center px-4">
              <AppText className="text-sm text-foreground-muted">Digitar código</AppText>
            </Pressable>
          </View>
        : <View className="items-center gap-3">
            <Pressable disabled={!ready || busy} onPress={() => void (pasted ? pair(pasted.address, pasted.code, pasted.access) : pair(address, code))} className={`h-12 w-full items-center justify-center rounded-full bg-foreground${ready ? "" : " opacity-40"}`}>
              {busy ? <ActivityIndicator /> : <AppText className="font-app-medium text-base text-screen">Conectar</AppText>}
            </Pressable>
            <Pressable onPress={() => { setError(null); setMode("start"); }} className="h-10 justify-center px-4">
              <AppText className="text-sm text-foreground-muted">Voltar</AppText>
            </Pressable>
          </View>}
    </KeyboardAvoidingView>
  </View>;
}
