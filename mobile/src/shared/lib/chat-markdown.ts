/**
 * A workspace file reference in inline code or a link target: a relative path
 * (or one inside `cwd`) with a name that looks like a file, optionally `:line`.
 */
export function fileReference(raw: string, cwd: string | null): { path: string; line: number | null } | null {
  let value = raw.trim().replace(/^["']|["']$/g, "");
  if (!value || value.length > 300 || /\s|:\/\//.test(value)) return null;
  if (value.startsWith("file://")) value = decodeURI(value.slice(7));
  const position = /:(\d+)(?::\d+)?$/.exec(value);
  const line = position ? Number(position[1]) : null;
  if (position) value = value.slice(0, position.index);
  value = value.replace(/#L(\d+).*$/, "");
  if (value.startsWith("/")) {
    if (!cwd || !value.startsWith(`${cwd.replace(/\/$/, "")}/`)) return null;
    value = value.slice(cwd.replace(/\/$/, "").length + 1);
  }
  value = value.replace(/^\.\//, "");
  if (!value || value.split("/").some((part) => part === ".." || part === "." || part === "")) {
    return value.endsWith("/") && !value.slice(0, -1).split("/").some((part) => !part || part === "..") ? { path: value.slice(0, -1), line } : null;
  }
  const name = value.split("/").pop()!;
  const looksLikeFile = /\.[A-Za-z][\w-]{0,9}$/.test(name) || /^(Makefile|Dockerfile|LICENSE|README|Gemfile|Procfile|\.env\.example|\.gitignore)$/.test(name);
  if (!looksLikeFile && !value.includes("/")) return null;
  if (!/^[\w@.+~-]+(\/[\w@.+~ -]+)*$/.test(value)) return null;
  return { path: value, line };
}
