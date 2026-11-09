import { File, Paths } from "expo-file-system";
import * as Haptics from "expo-haptics";
import { requestPermissionsAsync, saveToLibraryAsync } from "expo-media-library/legacy";
import { ActionSheetIOS, Alert } from "react-native";

import { translate } from "@sirus/i18n";

import { imageExtension } from "../lib/image-extension";

const t = (key: string) => translate("pt-BR", key);

/**
 * Adds a picture to the Photos library: a `data:` URI (what the computer sends for a file in the
 * folder), an `http(s)` address or a local file. Asks only to add, never to read the library.
 * Returns false when the person has not allowed it (an alert says where to).
 */
export async function saveImageToPhotos(uri: string): Promise<boolean> {
  const permission = await requestPermissionsAsync(true);
  if (!permission.granted) {
    Alert.alert(t("reply.imageSaveFailed"), t("reply.photosDenied"));
    return false;
  }
  const name = `sirus-${Date.now().toString(36)}.${imageExtension(uri)}`;
  const file = new File(Paths.cache, name);
  try {
    if (uri.startsWith("data:")) file.write(uri.slice(uri.indexOf(",") + 1), { encoding: "base64" });
    else if (/^https?:/i.test(uri)) await File.downloadFileAsync(uri, file);
    else new File(uri).copy(file);
    await saveToLibraryAsync(file.uri);
    void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success).catch(() => undefined);
    return true;
  } finally {
    try { if (file.exists) file.delete(); } catch { /* only a cache file */ }
  }
}

/** The long-press menu of a picture: one action, "Salvar em Fotos", with the result in an alert on failure. */
export function offerSaveImage(uri: string): void {
  const save = () => void saveImageToPhotos(uri).catch((reason) => Alert.alert(t("reply.imageSaveFailed"), reason instanceof Error ? reason.message : String(reason)));
  void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium).catch(() => undefined);
  ActionSheetIOS.showActionSheetWithOptions({ options: [t("reply.saveImage"), "Cancelar"], cancelButtonIndex: 1 }, (index) => { if (index === 0) save(); });
}
