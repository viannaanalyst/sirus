// Pure parts of `npm run release` (ADR-140): versions, release notes, latest.json and the
// update signature check. scripts/release.ts does the side effects; tests/release.test.ts covers these.
import { createHash, createPublicKey, verify } from "node:crypto";

export const REPOSITORY = "viannaanalyst/sirus";
/** The updater's platform key for Apple silicon Macs; the only build published today. */
export const PLATFORM = "darwin-aarch64";
/** Asset names without spaces; the website's download button points at the DMG name. */
export const DMG_ASSET = "SirusCode-arm64.dmg";
export const UPDATE_ASSET = "SirusCode-arm64.app.tar.gz";
/** Release notes longer than this are cut in latest.json (the full list stays on the GitHub release). */
const MAX_FEED_NOTES = 4000;

export interface Version { major: number; minor: number; patch: number; pre: string[] }

/** `1.2.3`, `v1.2.3` or `1.2.3-beta.1`; null when it is not a release version. */
export function parseVersion(raw: string): Version | null {
  const match = /^v?(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(raw.trim());
  if (!match) return null;
  return { major: Number(match[1]), minor: Number(match[2]), patch: Number(match[3]), pre: match[4] ? match[4].split(".") : [] };
}

export function formatVersion(version: Version): string {
  return `${version.major}.${version.minor}.${version.patch}${version.pre.length ? `-${version.pre.join(".")}` : ""}`;
}

/** Semantic version order: negative when a is older than b. */
export function compareVersions(a: Version, b: Version): number {
  for (const key of ["major", "minor", "patch"] as const) {
    if (a[key] !== b[key]) return a[key] - b[key];
  }
  // A version without a prerelease is newer than any of its prereleases.
  if (!a.pre.length || !b.pre.length) return (a.pre.length ? -1 : 0) + (b.pre.length ? 1 : 0);
  for (let index = 0; index < Math.max(a.pre.length, b.pre.length); index++) {
    const left = a.pre[index];
    const right = b.pre[index];
    if (left === undefined) return -1;
    if (right === undefined) return 1;
    const leftNumber = /^\d+$/.test(left);
    const rightNumber = /^\d+$/.test(right);
    if (leftNumber && rightNumber && Number(left) !== Number(right)) return Number(left) - Number(right);
    if (leftNumber !== rightNumber) return leftNumber ? -1 : 1;
    if (left !== right) return left < right ? -1 : 1;
  }
  return 0;
}

/** The newest `vX.Y.Z` tag among `git tag` lines, or null. */
export function latestTag(tags: string[]): string | null {
  let best: { tag: string; version: Version } | null = null;
  for (const tag of tags.map((line) => line.trim()).filter(Boolean)) {
    const version = tag.startsWith("v") ? parseVersion(tag) : null;
    if (version && (!best || compareVersions(version, best.version) > 0)) best = { tag, version };
  }
  return best?.tag ?? null;
}

/** Release notes from commit subjects (newest first), skipping merges and earlier release commits. */
export function notesFromCommits(subjects: string[]): string {
  const lines = subjects
    .map((subject) => subject.trim())
    .filter((subject) => subject && !/^Merge (branch|pull request) /.test(subject) && !/^Release v\d/.test(subject))
    .map((subject) => `- ${subject}`);
  return lines.length ? lines.join("\n") : "Maintenance release.";
}

export interface UpdateFeed {
  version: string;
  notes: string;
  pub_date: string;
  platforms: Record<string, { signature: string; url: string }>;
}

/** The download URL of a release asset (spaces are never used in asset names). */
export function assetUrl(version: string, asset: string, repository = REPOSITORY): string {
  return `https://github.com/${repository}/releases/download/v${version}/${encodeURIComponent(asset)}`;
}

/** latest.json in the shape tauri-plugin-updater reads from a static endpoint. */
export function updateFeed(input: { version: string; notes: string; date: Date; signature: string; repository?: string }): UpdateFeed {
  const notes = input.notes.trim();
  return {
    version: input.version,
    notes: notes.length > MAX_FEED_NOTES ? `${notes.slice(0, MAX_FEED_NOTES - 1).trimEnd()}…` : notes,
    pub_date: input.date.toISOString().replace(/\.\d{3}Z$/, "Z"),
    platforms: {
      [PLATFORM]: { signature: input.signature.trim(), url: assetUrl(input.version, UPDATE_ASSET, input.repository) },
    },
  };
}

/** Problems that make a feed unusable by the updater; empty when it is fine. */
export function feedProblems(feed: UpdateFeed): string[] {
  const problems: string[] = [];
  if (!parseVersion(feed.version) || feed.version.startsWith("v")) problems.push("version is not X.Y.Z");
  if (Number.isNaN(Date.parse(feed.pub_date))) problems.push("pub_date is not RFC 3339");
  const platform = feed.platforms[PLATFORM];
  if (!platform) problems.push(`no ${PLATFORM} platform`);
  else {
    if (!platform.url.startsWith("https://")) problems.push("url is not https");
    if (!platform.signature) problems.push("signature is empty");
  }
  return problems;
}

/** Bumps the top-level `"version"` of package.json or tauri.conf.json, keeping the formatting. */
export function bumpJsonVersion(text: string, version: string): string {
  const next = text.replace(/^(\s{2}"version":\s*")[^"]*(")/m, `$1${version}$2`);
  if (next === text && !text.includes(`"version": "${version}"`)) throw new Error("no top-level version field");
  return next;
}

/** Bumps `version` in Cargo.toml's [package] table only. */
export function bumpCargoVersion(text: string, version: string): string {
  const start = text.indexOf("[package]");
  if (start < 0) throw new Error("no [package] table");
  const end = text.indexOf("\n[", start + 1);
  const table = text.slice(start, end < 0 ? undefined : end);
  const bumped = table.replace(/^version\s*=\s*"[^"]*"/m, `version = "${version}"`);
  if (bumped === table && !table.includes(`version = "${version}"`)) throw new Error("no package version");
  return text.slice(0, start) + bumped + (end < 0 ? "" : text.slice(end));
}

/** Bumps this package's entry in Cargo.lock (the build would do it too). */
export function bumpCargoLock(text: string, name: string, version: string): string {
  const pattern = new RegExp(`(\\[\\[package\\]\\]\\nname = "${name.replace(/[-]/g, "\\-")}"\\nversion = ")[^"]*(")`);
  if (!pattern.test(text)) throw new Error(`${name} is not in Cargo.lock`);
  return text.replace(pattern, `$1${version}$2`);
}

interface Decoded { algorithm: string; keyId: string; payload: Buffer }

function minisignLine(base64: string, size: number): Decoded {
  const bytes = Buffer.from(base64.trim(), "base64");
  if (bytes.length !== size) throw new Error("malformed minisign data");
  return { algorithm: bytes.subarray(0, 2).toString("latin1"), keyId: bytes.subarray(2, 10).toString("hex"), payload: bytes.subarray(10) };
}

function textLines(encoded: string): string[] {
  return Buffer.from(encoded.trim(), "base64").toString("utf8").split("\n").map((line) => line.replace(/\r$/, ""));
}

function ed25519Key(raw: Buffer) {
  return createPublicKey({ key: { kty: "OKP", crv: "Ed25519", x: raw.toString("base64url") }, format: "jwk" });
}

/** The key id of a Tauri public key (base64 of a minisign public key file). */
export function publicKeyId(pubkey: string): string {
  return minisignLine(textLines(pubkey)[1] ?? "", 42).keyId;
}

/**
 * Checks an update signature the way the updater will: the `.sig` (base64 of a minisign
 * signature file) must come from the configured public key and sign exactly these bytes.
 */
export function verifyUpdateSignature(data: Buffer, signature: string, pubkey: string): { ok: boolean; reason?: string } {
  try {
    const keyLines = textLines(pubkey);
    const key = minisignLine(keyLines[1] ?? "", 42);
    const lines = textLines(signature);
    const sig = minisignLine(lines[1] ?? "", 74);
    const trusted = lines[2] ?? "";
    const global = Buffer.from((lines[3] ?? "").trim(), "base64");
    if (!trusted.startsWith("trusted comment: ")) return { ok: false, reason: "no trusted comment" };
    if (sig.keyId !== key.keyId) return { ok: false, reason: `signed with key ${sig.keyId}, the app trusts ${key.keyId}` };
    const publicKey = ed25519Key(key.payload);
    const message = sig.algorithm === "ED" ? createHash("blake2b512").update(data).digest() : data;
    if (sig.algorithm !== "ED" && sig.algorithm !== "Ed") return { ok: false, reason: `unknown algorithm ${sig.algorithm}` };
    if (!verify(null, message, publicKey, sig.payload)) return { ok: false, reason: "signature does not match the file" };
    const comment = Buffer.from(trusted.slice("trusted comment: ".length), "utf8");
    if (!verify(null, Buffer.concat([sig.payload, comment]), publicKey, global)) return { ok: false, reason: "trusted comment signature does not match" };
    return { ok: true };
  } catch (error) {
    return { ok: false, reason: error instanceof Error ? error.message : String(error) };
  }
}

export interface ReleaseOptions { version: string; dryRun: boolean; skipChecks: boolean; notes: string | null; key: string | null }

/** `npm run release -- <version> [--dry-run] [--skip-checks] [--notes "…"] [--key path]`. */
export function parseArgs(argv: string[]): ReleaseOptions {
  const options: ReleaseOptions = { version: "", dryRun: false, skipChecks: false, notes: null, key: null };
  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index];
    const value = () => {
      const next = argv[++index];
      if (next === undefined || next.startsWith("--")) throw new Error(`${arg} needs a value`);
      return next;
    };
    if (arg === "--dry-run") options.dryRun = true;
    else if (arg === "--skip-checks") options.skipChecks = true;
    else if (arg === "--notes") options.notes = value();
    else if (arg.startsWith("--notes=")) options.notes = arg.slice("--notes=".length);
    else if (arg === "--key") options.key = value();
    else if (arg.startsWith("--")) throw new Error(`unknown option ${arg}`);
    else if (!options.version) options.version = arg;
    else throw new Error(`unexpected argument ${arg}`);
  }
  if (!options.version) throw new Error("usage: npm run release -- <version> [--dry-run] [--skip-checks] [--notes \"…\"]");
  const parsed = parseVersion(options.version);
  if (!parsed) throw new Error(`${options.version} is not a version like 1.2.3`);
  options.version = formatVersion(parsed);
  return options;
}
