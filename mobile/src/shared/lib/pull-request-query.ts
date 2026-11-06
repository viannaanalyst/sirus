import type { GithubItem } from "../client/types";
import { parseGithubReference } from "./github-inbox";

/**
 * GitHub-style qualifiers typed into a pull request search: `label:`, `-label:`, `author:`,
 * `draft:`, `review:` and `status:`/`checks:`. Any other `key:value` is read as a
 * colon-namespaced label (`size:XS`, `area:web`), which is what such a token almost always is;
 * a quoted token stays plain text.
 */
export interface PullRequestQueryFilters {
  /** Each inner list is one `label:` qualifier: any of its names matches; the lists all must. */
  readonly labels?: readonly (readonly string[])[];
  readonly excludedLabels?: readonly string[];
  /** A login, or `@me` / `me` for the person signed in to GitHub. */
  readonly author?: string;
  readonly draft?: "only" | "hide";
  readonly review?: "approved" | "changes-requested" | "review-required" | "none";
  readonly checks?: "passing" | "failing" | "pending";
}

export interface PullRequestQuery {
  /** The words left once the qualifiers are taken out. */
  readonly text: string;
  readonly filters: PullRequestQueryFilters;
}

const REVIEW_VALUES: Record<string, PullRequestQueryFilters["review"]> = {
  approved: "approved",
  changes_requested: "changes-requested",
  "changes-requested": "changes-requested",
  required: "review-required",
  review_required: "review-required",
  "review-required": "review-required",
  none: "none",
};

const CHECKS_VALUES: Record<string, PullRequestQueryFilters["checks"]> = {
  success: "passing",
  passing: "passing",
  failure: "failing",
  failing: "failing",
  pending: "pending",
};

const QUERY_TOKEN = /(?:[^\s"]|"[^"]*")+/g;

const MAX_QUALIFIER_VALUES = 10;

const MAX_QUALIFIER_LENGTH = 200;

const qualifierValue = (raw: string) => raw.replaceAll('"', "").trim();

function splitQualifierList(raw: string): string[] {
  return (raw.match(/(?:[^,"]|"[^"]*")+/g) ?? []).map(qualifierValue).filter((part) => part.length > 0);
}

function boundedNames(names: readonly string[]): string[] {
  return names.slice(0, MAX_QUALIFIER_VALUES).map((name) => name.slice(0, MAX_QUALIFIER_LENGTH).trim()).filter((name) => name.length > 0);
}

export function parsePullRequestQuery(raw: string): PullRequestQuery {
  const text: string[] = [];
  const labels: string[][] = [];
  const excludedLabels: string[] = [];
  let author: string | undefined;
  let draft: PullRequestQueryFilters["draft"];
  let review: PullRequestQueryFilters["review"];
  let checks: PullRequestQueryFilters["checks"];
  for (const [token] of raw.matchAll(QUERY_TOKEN)) {
    const qualifier = /^(-?)([A-Za-z][A-Za-z0-9_-]*):(.*)$/.exec(token);
    const rawValue = qualifier?.[3] ?? "";
    const value = qualifier ? qualifierValue(rawValue) : "";
    const negated = qualifier?.[1] === "-";
    const key = value ? (qualifier?.[2]?.toLowerCase() ?? "") : "";
    if (key === "label") {
      const names = boundedNames(splitQualifierList(rawValue));
      if (names.length) {
        if (negated) excludedLabels.push(...names);
        else labels.push(names);
        continue;
      }
    } else if (key === "author") {
      if (!negated) { author = value.slice(0, MAX_QUALIFIER_LENGTH).trim(); continue; }
    } else if (key === "draft") {
      const lower = value.toLowerCase();
      if (!negated && (lower === "true" || lower === "false")) { draft = lower === "true" ? "only" : "hide"; continue; }
    } else if (key === "review") {
      const decision = negated ? undefined : REVIEW_VALUES[value.toLowerCase()];
      if (decision) { review = decision; continue; }
    } else if (key === "status" || key === "checks") {
      const state = negated ? undefined : CHECKS_VALUES[value.toLowerCase()];
      if (state) { checks = state; continue; }
    } else if (key && !value.startsWith("/")) {
      // `size:S,XS` names `size:S` and `size:XS`; a link's `https://` stays text.
      const names = boundedNames(splitQualifierList(rawValue).map((name) => name.includes(":") ? name : `${qualifier?.[2] ?? ""}:${name}`));
      if (names.length) {
        if (negated) excludedLabels.push(...names);
        else labels.push(names);
        continue;
      }
    }
    text.push(token);
  }
  return {
    text: text.join(" "),
    filters: {
      ...(labels.length ? { labels: labels.slice(0, MAX_QUALIFIER_VALUES) } : {}),
      ...(excludedLabels.length ? { excludedLabels: excludedLabels.slice(0, MAX_QUALIFIER_VALUES) } : {}),
      ...(author === undefined ? {} : { author }),
      ...(draft === undefined ? {} : { draft }),
      ...(review === undefined ? {} : { review }),
      ...(checks === undefined ? {} : { checks }),
    },
  };
}

const same = (a: string | null | undefined, b: string | null | undefined) => !!a && !!b && a.toLowerCase() === b.toLowerCase();

/** GitHub's rolled-up check state (`SUCCESS`, `FAILURE`, `PENDING`, …) in the query's words. */
function checksState(checks: string | null): PullRequestQueryFilters["checks"] | null {
  const value = (checks ?? "").toLowerCase();
  if (value.includes("fail") || value.includes("error")) return "failing";
  if (value.includes("pend") || value.includes("progress") || value.includes("queued") || value.includes("expected")) return "pending";
  if (value.includes("success") || value.includes("pass")) return "passing";
  return null;
}

const REVIEW_DECISIONS: Record<NonNullable<PullRequestQueryFilters["review"]>, string | null> = {
  approved: "APPROVED",
  "changes-requested": "CHANGES_REQUESTED",
  "review-required": "REVIEW_REQUIRED",
  none: null,
};

/**
 * Whether a pull request matches a typed search: every qualifier holds and every remaining word
 * appears in its title, number, repository, author or branch. A pasted GitHub link or
 * `owner/repo#n` finds that one pull request.
 */
export function matchesPullRequestQuery(item: GithubItem, query: PullRequestQuery, viewer: string | null): boolean {
  const { filters } = query;
  const names = item.labels.map((label) => label.name.toLowerCase());
  if (filters.labels?.some((group) => !group.some((name) => names.includes(name.toLowerCase())))) return false;
  if (filters.excludedLabels?.some((name) => names.includes(name.toLowerCase()))) return false;
  if (filters.author) {
    const author = /^@?me$/i.test(filters.author) ? viewer : filters.author.replace(/^@/, "");
    if (!same(item.author, author)) return false;
  }
  if ((filters.draft === "only" && !item.isDraft) || (filters.draft === "hide" && item.isDraft)) return false;
  if (filters.review && (item.reviewDecision || null) !== REVIEW_DECISIONS[filters.review]) return false;
  if (filters.checks && checksState(item.checks) !== filters.checks) return false;
  const reference = parseGithubReference(query.text);
  if (reference) return same(item.repository, reference.repository) && item.number === reference.number;
  const haystack = `${item.title} #${item.number} ${item.repository} ${item.author ?? ""} ${item.headRef ?? ""}`.toLocaleLowerCase();
  return query.text.replaceAll('"', "").toLocaleLowerCase().split(/\s+/).filter(Boolean).every((word) => haystack.includes(word));
}
