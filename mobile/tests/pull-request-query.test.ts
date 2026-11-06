// @ts-nocheck -- run with `npm test` (Node's own types are not part of the app's tsconfig)
import { test } from "node:test";
import assert from "node:assert/strict";
import { matchesPullRequestQuery, parsePullRequestQuery } from "../src/shared/lib/pull-request-query.ts";
import type { GithubItem } from "../src/shared/client/types.ts";

const item = (number: number, extra: Partial<GithubItem> = {}): GithubItem => ({ kind: "pullRequest", repository: "acme/app", number, title: `PR ${number}`, url: `https://github.com/acme/app/pull/${number}`, state: "open", isDraft: false, author: "other", createdAt: "t", updatedAt: "t", headRef: "feature", baseRef: "main", additions: 1, deletions: 1, reviewDecision: null, mergeable: "MERGEABLE", labels: [], commentCount: 0, assignees: [], reviewRequests: [], checks: null, ...extra });
const labelled = (...names: string[]) => names.map((name) => ({ name, color: null }));
const matches = (query: string, entry: GithubItem, viewer: string | null = "Me") => matchesPullRequestQuery(entry, parsePullRequestQuery(query), viewer);

test("quoted values stay whole and a negated label is excluded", () => {
  assert.deepEqual(parsePullRequestQuery('label:"needs design" -label:wip Fix the parser'), { text: "Fix the parser", filters: { labels: [["needs design"]], excludedLabels: ["wip"] } });
});

test("a comma list is one group, each label qualifier another, and quoted commas stay in the name", () => {
  assert.deepEqual(parsePullRequestQuery("label:size:S,size:XS").filters.labels, [["size:S", "size:XS"]]);
  assert.deepEqual(parsePullRequestQuery("label:bug label:a,,b").filters.labels, [["bug"], ["a", "b"]]);
  assert.deepEqual(parsePullRequestQuery('label:"needs,triage",wip').filters.labels, [["needs,triage", "wip"]]);
  assert.deepEqual(parsePullRequestQuery("-label:a,b").filters, { excludedLabels: ["a", "b"] });
});

test("scalar qualifiers read GitHub's own spelling", () => {
  assert.deepEqual(parsePullRequestQuery("author:octocat DRAFT:false review:changes_requested status:failure").filters, { author: "octocat", draft: "hide", review: "changes-requested", checks: "failing" });
  assert.deepEqual(parsePullRequestQuery("draft:true checks:pending review:none").filters, { draft: "only", checks: "pending", review: "none" });
});

test("an unknown value or a stray colon stays text; an unknown key is a namespaced label", () => {
  const parsed = parsePullRequestQuery("milestone:v2 draft:maybe status: parser");
  assert.deepEqual(parsed.filters, { labels: [["milestone:v2"]] });
  assert.equal(parsed.text, "draft:maybe status: parser");
  assert.deepEqual(parsePullRequestQuery("size:S,XS").filters.labels, [["size:S", "size:XS"]]);
  assert.deepEqual(parsePullRequestQuery("-size:XXL").filters.excludedLabels, ["size:XXL"]);
  assert.equal(parsePullRequestQuery('"size:XXL"').text, '"size:XXL"');
  assert.equal(parsePullRequestQuery("https://github.com/acme/app/pull/1").text, "https://github.com/acme/app/pull/1");
});

test("long and many values are cut back, and an empty query is empty", () => {
  const many = Array.from({ length: 14 }, (_, index) => `l${index}`).join(",");
  const long = "x".repeat(240);
  const parsed = parsePullRequestQuery(`label:${many} label:${long} author:${long}`);
  assert.equal(parsed.filters.labels?.[0]?.length, 10);
  assert.deepEqual(parsed.filters.labels?.[1], ["x".repeat(200)]);
  assert.equal(parsed.filters.author, "x".repeat(200));
  assert.deepEqual(parsePullRequestQuery("   "), { text: "", filters: {} });
});

test("matching applies every qualifier and every remaining word", () => {
  const bug = item(1, { title: "Fix the parser", labels: labelled("Bug", "size:S"), author: "octocat", reviewDecision: "APPROVED", checks: "SUCCESS" });
  const draft = item(2, { title: "Draft parser", isDraft: true, labels: labelled("wip"), author: "me", checks: "FAILURE" });
  assert.equal(matches("label:bug parser", bug), true);
  assert.equal(matches("label:bug label:size:M,size:S", bug), true);
  assert.equal(matches("label:bug label:size:M", bug), false);
  assert.equal(matches("-label:wip", draft), false);
  assert.equal(matches("draft:true", draft), true);
  assert.equal(matches("draft:false", draft), false);
  assert.equal(matches("author:@me", draft), true);
  assert.equal(matches("author:@me", bug), false);
  assert.equal(matches("author:OctoCat review:approved status:success", bug), true);
  assert.equal(matches("review:none", bug), false);
  assert.equal(matches("review:none", draft), true);
  assert.equal(matches("status:failing", draft), true);
  assert.equal(matches("status:failing", bug), false);
  assert.equal(matches("fix parser", bug), true);
  assert.equal(matches("fix parser", draft), false);
  assert.equal(matches("acme/app#2", draft), true);
  assert.equal(matches("acme/app#2", bug), false);
});
