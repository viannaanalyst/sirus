import * as DocumentPicker from "expo-document-picker";
import { File } from "expo-file-system";
import * as ImagePicker from "expo-image-picker";

import type { PromptAttachment } from "@sirus/client/types";

import { useSirus } from "./store";

/** A file picked on the phone, before it is sent to the computer; `uri` is the local copy a preview can show. */
export interface PickedFile { name: string; data: string; uri?: string; mimeType?: string }

/** Whether a name or type says "picture" (the composer then shows a thumbnail instead of a chip). */
export function isImageFile(file: { name: string; mimeType?: string }): boolean {
  return file.mimeType ? file.mimeType.startsWith("image/") : /\.(jpe?g|png|gif|webp|heic|heif|bmp|tiff?)$/i.test(file.name);
}

/** At most 8 per message, as the computer allows. */
const MAX_FILES = 8;

/**
 * Photos or files from the iPhone, sent to the computer as the composer's attachments
 * (`paste_prompt_attachments`, base64 like the phone web app's). Photos are re-encoded as JPEG
 * at a quality that keeps them well under the 10 MiB limit.
 */
export async function pickAttachments(kind: "photos" | "files"): Promise<PickedFile[]> {
  if (kind === "photos") {
    const result = await ImagePicker.launchImageLibraryAsync({ mediaTypes: ["images"], allowsMultipleSelection: true, selectionLimit: MAX_FILES, quality: 0.75, base64: true });
    if (result.canceled) return [];
    return result.assets.filter((asset) => asset.base64).map((asset, index) => ({
      name: asset.fileName ?? `foto-${index + 1}.jpg`,
      data: asset.base64!,
      uri: asset.uri,
      mimeType: asset.mimeType ?? "image/jpeg",
    }));
  }
  const result = await DocumentPicker.getDocumentAsync({ multiple: true, copyToCacheDirectory: true });
  if (result.canceled) return [];
  return Promise.all(result.assets.slice(0, MAX_FILES).map(async (asset) => ({ name: asset.name, data: await new File(asset.uri).base64(), uri: asset.uri, mimeType: asset.mimeType })));
}

export function attachmentOwner(sessionId: string) {
  return `session:${sessionId}`;
}

export async function uploadAttachments(computerId: string, owner: string, files: PickedFile[]): Promise<PromptAttachment[]> {
  if (!files.length) return [];
  // Only name and bytes travel; the local uri stays on the phone.
  return useSirus.getState().invoke<PromptAttachment[]>(computerId, "paste_prompt_attachments", { owner, files: files.map(({ name, data }) => ({ name, data })), expectedText: null });
}

export async function releaseAttachments(computerId: string, owner: string, ids: string[]) {
  if (ids.length) await useSirus.getState().invoke<void>(computerId, "release_prompt_attachments", { owner, ids }).catch(() => undefined);
}
