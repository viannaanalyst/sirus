export type MaterialIconPack = typeof import("react-material-icon-theme");

/**
 * The pack matches `fileExtension` only when it is given; it does not peel one off the name.
 * Try the full name, then each compound suffix (`d.ts`, then `ts`), then the plain file icon.
 */
export function materialFileIcon(icons: Pick<MaterialIconPack, "getFileIcon">, fileName: string): string {
  const key = fileName.toLowerCase();
  const byName = icons.getFileIcon({ fileName: key, fallback: "", iconPack: "" });
  if (byName) return byName;
  for (const extension of compoundExtensions(key)) {
    const byExtension = icons.getFileIcon({ fileExtension: extension, fallback: "", iconPack: "" });
    if (byExtension) return byExtension;
  }
  return "file";
}

export function compoundExtensions(fileName: string): string[] {
  const parts = fileName.split(".");
  const start = parts[0] === "" ? 1 : 0;
  const extensions: string[] = [];
  for (let index = start + 1; index < parts.length; index++) extensions.push(parts.slice(index).join("."));
  return extensions;
}
