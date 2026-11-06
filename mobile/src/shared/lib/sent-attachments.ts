import type { MessageAttachment } from "../client/types";

export interface SentImage { readonly key: string; readonly name: string; readonly small: string; readonly full: string }

/** The pictures of a sent message with a thumbnail the computer serves (`/api/thumbnail/<id>`), and the other files. */
export function splitAttachments(attachments: readonly MessageAttachment[], thumbnailUrl: (id: string) => string | undefined): { images: SentImage[]; files: MessageAttachment[] } {
  const images: SentImage[] = [];
  const files: MessageAttachment[] = [];
  for (const [index, item] of attachments.entries()) {
    const full = item.id && item.hasThumbnail ? thumbnailUrl(item.id) : undefined;
    if (full) images.push({ key: `${item.id}`, name: item.name, small: `${full}${full.includes("?") ? "&" : "?"}size=small`, full });
    // A legacy message keeps its picture inline.
    else if (item.thumbnail?.startsWith("data:image/")) images.push({ key: `${item.id ?? index}`, name: item.name, small: item.thumbnail, full: item.thumbnail });
    else files.push(item);
  }
  return { images, files };
}
