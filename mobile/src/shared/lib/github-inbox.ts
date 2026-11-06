import type { GithubItemKind } from "../client/types";

/** A pasted GitHub link or `owner/repo#n` reference. */
export function parseGithubReference(text: string): { repository: string; number: number; kind: GithubItemKind | null } | null {
  const value = text.trim();
  const link = /^https:\/\/github\.com\/([A-Za-z0-9-]{1,100}\/[A-Za-z0-9._-]{1,100})\/(pull|issues)\/(\d{1,8})(?:[/?#].*)?$/.exec(value);
  if (link) return { repository: link[1], number: Number(link[3]), kind: link[2] === "pull" ? "pullRequest" : "issue" };
  const short = /^([A-Za-z0-9-]{1,100}\/[A-Za-z0-9._-]{1,100})#(\d{1,8})$/.exec(value);
  return short ? { repository: short[1], number: Number(short[2]), kind: null } : null;
}
