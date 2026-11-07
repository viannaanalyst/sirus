import Constants from "expo-constants";
import * as Updates from "expo-updates";
import { Alert } from "react-native";

import { useSirus } from "./store";

/**
 * Updates from the owner's computer (ADR-135 addendum). The build names one computer's update
 * address; at launch the app gives `expo-updates` its device token for that computer, then asks
 * for a newer update. One that arrives is offered right away and otherwise runs at the next
 * launch. Nothing happens in development or when the build has no update address.
 */
export async function checkForAppUpdate(interactive = false): Promise<"none" | "ready" | "unavailable"> {
  if (__DEV__ || !Updates.isEnabled) return "unavailable";
  const url = (Constants.expoConfig?.updates as { url?: string } | undefined)?.url;
  if (!url) return "unavailable";
  const origin = new URL(url).origin;
  // A computer paired again at its public address keeps its old address for this.
  const token = useSirus.getState().computers
    .flatMap((item) => [item, ...(item.previous ? [item.previous] : [])])
    .find((item) => new URL(item.baseUrl).origin === origin)?.token;
  if (!token) return "unavailable";
  // The header is declared empty in the build; only its value changes here.
  Updates.setUpdateRequestHeadersOverride({ Authorization: `Bearer ${token}` });
  try {
    const check = await Updates.checkForUpdateAsync();
    if (!check.isAvailable) return "none";
    const fetched = await Updates.fetchUpdateAsync();
    if (!fetched.isNew) return "none";
    Alert.alert("Atualização pronta", "Uma versão nova do Sirus foi baixada do seu computador.", [
      { text: "Depois", style: "cancel" },
      { text: "Reiniciar agora", onPress: () => void Updates.reloadAsync() },
    ]);
    return "ready";
  } catch (reason) {
    if (interactive) Alert.alert("Atualização", reason instanceof Error ? reason.message : String(reason));
    return "unavailable";
  }
}

/** What the app is running: the build's own code, or an update from the computer. */
export function runningUpdateLabel(): string {
  if (__DEV__) return "desenvolvimento";
  if (Updates.isEmbeddedLaunch || !Updates.createdAt) return "do aplicativo";
  return Updates.createdAt.toLocaleString("pt-BR", { day: "2-digit", month: "2-digit", hour: "2-digit", minute: "2-digit" });
}
