const EXTENSIONS: Record<string, string> = { "image/png": "png", "image/jpeg": "jpg", "image/gif": "gif", "image/webp": "webp", "image/heic": "heic" };

/** The file extension Photos needs, from a data URI's type, else from the address. */
export function imageExtension(uri: string): string {
  const mime = /^data:([^;,]+)/i.exec(uri)?.[1]?.toLowerCase();
  if (mime && EXTENSIONS[mime]) return EXTENSIONS[mime];
  const named = /\.(png|jpe?g|gif|webp|heic)(?:[?#]|$)/i.exec(uri)?.[1]?.toLowerCase();
  return named === "jpeg" ? "jpg" : named ?? "jpg";
}
