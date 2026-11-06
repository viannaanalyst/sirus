// `node scripts/release-sirus.ts [--publish] [--notes "…"] [--key path] [--skip-tests]` (ADR-140, ADR-190).
//
// Releases Sirus, the GPUI app in native-spike/. Without --publish (the default) it builds the
// host and the window, bundles Sirus.app, packs Sirus-arm64.dmg and Sirus-arm64.app.tar.gz, signs
// the archive with the updater key, writes sirus-latest.json under
// native-spike/target/release-upload/sirus-v<version>/ and only prints what it would publish.
// --publish needs to be explicit.
//
// Sirus installs read releases/latest/download/latest.json and the website links
// releases/latest/download/SirusCode-arm64.dmg, so a Sirus release must never become the
// repository's "latest": version releases (tag sirus-v<version>) are created with --latest=false,
// and the app reads its own feed from the fixed release `sirus-channel`, whose
// sirus-latest.json is replaced each time (the same release also keeps the newest DMG under a
// stable link).
import { spawnSync, type SpawnSyncOptions } from "node:child_process";
import { chmodSync, copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, statSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  PLATFORM,
  REPOSITORY,
  compareVersions,
  feedProblems,
  notesFromCommits,
  parseVersion,
  updateFeed,
  verifyUpdateSignature,
} from "./release-lib.ts";

const root = fileURLToPath(new URL("../", import.meta.url));
const nativeDir = path.join(root, "native-spike");
const tauriDir = path.join(root, "src-tauri");
const DEFAULT_KEY = path.join(homedir(), ".config", "sirus-code", "updater.key");
const CHANNEL = "sirus-channel";
const FEED_ASSET = "sirus-latest.json";
const DMG_ASSET = "Sirus-arm64.dmg";
const UPDATE_ASSET = "Sirus-arm64.app.tar.gz";
const BUNDLE = "Sirus.app";

const env = { ...process.env, PATH: `${path.join(homedir(), ".cargo", "bin")}:${process.env.PATH ?? ""}` };

function step(message: string) {
  console.log(`\n▸ ${message}`);
}

function fail(message: string): never {
  console.error(`\n✗ ${message}`);
  process.exit(1);
}

function run(command: string, args: string[], options: SpawnSyncOptions = {}) {
  const result = spawnSync(command, args, { cwd: root, stdio: "inherit", env, ...options });
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} failed${result.status === null ? "" : ` (exit ${result.status})`}`);
}

function capture(command: string, args: string[], options: SpawnSyncOptions = {}): string {
  const result = spawnSync(command, args, { cwd: root, encoding: "utf8", env, ...options });
  if (result.status !== 0) throw new Error(`${command} ${args.join(" ")} failed: ${String(result.stderr ?? "").trim()}`);
  return String(result.stdout ?? "").trim();
}

function succeeds(command: string, args: string[]): boolean {
  return spawnSync(command, args, { cwd: root, stdio: "ignore", env }).status === 0;
}

const quote = (arg: string) => (/[\s"']/.test(arg) ? JSON.stringify(arg) : arg);

interface Options { publish: boolean; skipTests: boolean; notes: string | null; key: string | null }

function parseArgs(argv: string[]): Options {
  const options: Options = { publish: false, skipTests: false, notes: null, key: null };
  for (let index = 0; index < argv.length; index++) {
    const arg = argv[index];
    const value = () => {
      const next = argv[++index];
      if (next === undefined || next.startsWith("--")) throw new Error(`${arg} needs a value`);
      return next;
    };
    if (arg === "--publish") options.publish = true;
    else if (arg === "--dry-run") options.publish = false;
    else if (arg === "--skip-tests") options.skipTests = true;
    else if (arg === "--notes") options.notes = value();
    else if (arg.startsWith("--notes=")) options.notes = arg.slice("--notes=".length);
    else if (arg === "--key") options.key = value();
    else throw new Error(`unknown argument ${arg} (usage: release-sirus.ts [--publish] [--notes "…"] [--key path] [--skip-tests])`);
  }
  return options;
}

const options = (() => {
  try {
    return parseArgs(process.argv.slice(2));
  } catch (error) {
    return fail(error instanceof Error ? error.message : String(error));
  }
})();
const publish = options.publish;

// The version is native-spike/Cargo.toml's [package] version.
const cargoToml = readFileSync(path.join(nativeDir, "Cargo.toml"), "utf8");
const version = /^\[package\][\s\S]*?^version\s*=\s*"([^"]+)"/m.exec(cargoToml)?.[1] ?? "";
if (!parseVersion(version)) fail(`native-spike/Cargo.toml has an unreadable version (${version || "none"}).`);
const tag = `sirus-v${version}`;
const prerelease = (parseVersion(version)?.pre.length ?? 0) > 0;
const warnOrFail = (message: string) => (publish ? fail(message) : console.warn(`  ! ${message} (allowed in a dry run)`));

console.log(`Sirus ${version} (${tag}) — ${publish ? "PUBLISHING" : "dry run: builds and prints, publishes nothing"}`);

// 1. Where we are.
step("Checking the repository");
if (process.platform !== "darwin" || process.arch !== "arm64") fail("Sirus is built on an Apple silicon Mac.");
const head = capture("git", ["rev-parse", "HEAD"]);
const dirty = capture("git", ["status", "--porcelain"]);
if (dirty) warnOrFail(`The working tree has uncommitted changes:\n${dirty}`);
if (publish) {
  if (!succeeds("gh", ["auth", "status"])) fail("The GitHub CLI is not signed in (gh auth login).");
  // The release's tag is created on GitHub at this commit, so GitHub must have it.
  run("git", ["fetch", "--quiet", "origin"]);
  if (!capture("git", ["branch", "-r", "--contains", head])) fail(`${head.slice(0, 7)} is not on origin yet; push the branch first.`);
  if (succeeds("gh", ["release", "view", tag, "--repo", REPOSITORY])) fail(`The GitHub release ${tag} already exists.`);
}
const tags = capture("git", ["tag", "--list", "sirus-v*"]).split("\n").filter(Boolean);
if (tags.includes(tag)) fail(`The tag ${tag} already exists.`);
const previous = tags
  .filter((name) => parseVersion(name.slice("sirus-v".length)))
  .sort((a, b) => compareVersions(parseVersion(a.slice(7))!, parseVersion(b.slice(7))!))
  .at(-1) ?? null;
if (previous && compareVersions(parseVersion(version)!, parseVersion(previous.slice(7))!) <= 0) fail(`${version} is not newer than the last tag ${previous}.`);
if (publish) {
  // The feed's current version, when the channel exists: a feed must only move forward.
  const feedNow = spawnSync("gh", ["release", "download", CHANNEL, "--repo", REPOSITORY, "--pattern", FEED_ASSET, "--output", "-"], { cwd: root, encoding: "utf8", env });
  if (feedNow.status === 0) {
    const live = parseVersion(String((JSON.parse(feedNow.stdout) as { version?: string }).version ?? ""));
    if (live && compareVersions(parseVersion(version)!, live) <= 0) fail(`${version} is not newer than the version the feed offers (${live.major}.${live.minor}.${live.patch}).`);
  }
}
console.log(`  head ${head.slice(0, 7)}${previous ? `, last tag ${previous}` : ""}`);

// 2. The update signing key: outside the repository, never printed. A dry run without it builds
//    everything but cannot sign.
step("Reading the update signing key");
const keyPath = path.resolve(options.key ?? process.env.SIRUS_UPDATER_KEY ?? DEFAULT_KEY);
const keyThere = existsSync(keyPath);
if (!keyThere) {
  if (publish) fail(`No update signing key at ${keyPath} (set SIRUS_UPDATER_KEY to its path, or pass --key).`);
  console.warn(`  ! No key at ${keyPath}: this dry run builds and packs, but does not sign.`);
} else {
  if (!path.relative(root, keyPath).startsWith("..")) fail("The update signing key must live outside the repository.");
  if ((statSync(keyPath).mode & 0o077) !== 0) {
    console.warn(`  ! ${keyPath} is readable by other users; restricting it to you.`);
    chmodSync(keyPath, 0o600);
  }
  console.log(`  ${keyPath}`);
}
const pubkey = readFileSync(path.join(nativeDir, "updater.pub"), "utf8").trim();
if (!pubkey) fail("native-spike/updater.pub is empty.");

// 3. Release notes.
const notes = options.notes?.trim() || notesFromCommits(
  capture("git", ["log", "--no-merges", "--pretty=%s", ...(previous ? [`${previous}..HEAD`] : ["-n", "30"])]).split("\n"),
);

const uploadDir = path.join(nativeDir, "target", "release-upload", tag);
try {
  // 4. Tests.
  if (options.skipTests) step("Skipping the tests (--skip-tests)");
  else {
    step("Running the window's tests");
    run("cargo", ["test", "--features", "gpui/test-support"], { cwd: nativeDir });
  }

  // 5. Builds: the host first (the bundle holds it), then the window, then the bundle.
  step("Building sirus-host");
  run("cargo", ["build", "--release", "--no-default-features", "--features", "host", "--bin", "sirus-host"], {
    cwd: tauriDir,
    env: { ...env, CARGO_TARGET_DIR: path.join(nativeDir, "target", "host") },
  });
  step("Building the window");
  run("cargo", ["build", "--release"], { cwd: nativeDir });
  step("Bundling Sirus.app (signed as bundle.sh signs it)");
  run("./bundle.sh", [], { cwd: nativeDir });
  const app = path.join(nativeDir, BUNDLE);
  if (!existsSync(path.join(app, "Contents", "MacOS", "sirus-native-spike"))) throw new Error("bundle.sh did not produce Sirus.app.");
  const plist = readFileSync(path.join(app, "Contents", "Info.plist"), "utf8");
  if (!plist.includes("<string>com.siruscode.native</string>")) throw new Error("The bundle id is not com.siruscode.native.");
  if (!plist.includes(`<key>CFBundleShortVersionString</key><string>${version}</string>`)) throw new Error(`The bundle's version is not ${version}.`);

  // 6. Pack: the update archive and the DMG, under names without spaces.
  step("Packing the update archive and the DMG");
  rmSync(uploadDir, { recursive: true, force: true });
  mkdirSync(uploadDir, { recursive: true });
  const archive = path.join(uploadDir, UPDATE_ASSET);
  run("tar", ["-czf", archive, "-C", nativeDir, BUNDLE], { env: { ...env, COPYFILE_DISABLE: "1" } });
  const staging = path.join(uploadDir, "dmg-staging");
  mkdirSync(staging);
  run("ditto", [app, path.join(staging, BUNDLE)]);
  run("ln", ["-s", "/Applications", path.join(staging, "Applications")]);
  run("hdiutil", ["create", "-quiet", "-volname", "Sirus", "-srcfolder", staging, "-ov", "-format", "UDZO", path.join(uploadDir, DMG_ASSET)]);
  rmSync(staging, { recursive: true, force: true });

  // 7. Sign the archive and check the signature as the app will.
  let signature = "";
  if (keyThere) {
    step("Signing the update archive");
    run("npx", ["tauri", "signer", "sign", "--private-key-path", keyPath, "--password", process.env.SIRUS_UPDATER_KEY_PASSWORD ?? "", archive]);
    signature = readFileSync(`${archive}.sig`, "utf8").trim();
    const checked = verifyUpdateSignature(readFileSync(archive), signature, pubkey);
    if (!checked.ok) throw new Error(`The update signature does not verify against updater.pub: ${checked.reason}.`);
    console.log("  The signature verifies against updater.pub.");
    const feed = updateFeed({ version, notes, date: new Date(), signature });
    // The archive lives under the version's own release, not under the shared `v<version>` tag.
    feed.platforms[PLATFORM].url = `https://github.com/${REPOSITORY}/releases/download/${tag}/${UPDATE_ASSET}`;
    const problems = feedProblems(feed);
    if (problems.length) throw new Error(`${FEED_ASSET} is not valid: ${problems.join(", ")}.`);
    writeFileSync(path.join(uploadDir, FEED_ASSET), `${JSON.stringify(feed, null, 2)}\n`);
  }
  writeFileSync(path.join(uploadDir, "notes.md"), `${notes}\n`);
  console.log(`  ${path.relative(root, uploadDir)}/`);
  for (const name of [DMG_ASSET, UPDATE_ASSET, ...(keyThere ? [`${UPDATE_ASSET}.sig`, FEED_ASSET] : [])]) console.log(`    ${name}`);
} catch (error) {
  fail(error instanceof Error ? error.message : String(error));
}

const files = (...names: string[]) => names.map((name) => path.join(uploadDir, name));
const versionRelease = [
  "release", "create", tag, ...files(DMG_ASSET, UPDATE_ASSET, `${UPDATE_ASSET}.sig`),
  "--repo", REPOSITORY,
  "--title", `Sirus ${version}`,
  "--notes-file", path.join(uploadDir, "notes.md"),
  "--target", head,
  // Never the repository's "latest": Sirus's installs and the website follow that.
  "--latest=false",
  ...(prerelease ? ["--prerelease"] : []),
];
const channelCreate = [
  "release", "create", CHANNEL,
  "--repo", REPOSITORY,
  "--title", "Sirus update feed",
  "--notes", "The feed Sirus reads (sirus-latest.json) and the newest Sirus-arm64.dmg. Replaced by every Sirus release; do not delete.",
  "--target", head,
  "--latest=false",
];
const channelUpload = ["release", "upload", CHANNEL, ...files(FEED_ASSET, DMG_ASSET), "--repo", REPOSITORY, "--clobber"];
const printed = (args: string[]) => `gh ${args.map(quote).join(" ")}`;

if (!publish) {
  step("Dry run finished. --publish would run, in this order:");
  console.log(`  ${printed(versionRelease)}`);
  console.log(`  (if the ${CHANNEL} release does not exist) ${printed(channelCreate)}`);
  console.log(`  ${printed(channelUpload)}`);
  console.log(`\nThe feed (${FEED_ASSET}) is replaced last, so it never points at a missing file.`);
  console.log(`Feed URL in the app: https://github.com/${REPOSITORY}/releases/download/${CHANNEL}/${FEED_ASSET}`);
  console.log(`Stable DMG link:     https://github.com/${REPOSITORY}/releases/download/${CHANNEL}/${DMG_ASSET}`);
  console.log(`\nRelease notes:\n${notes}`);
  console.log("\nNothing was committed, tagged, pushed or uploaded.");
  process.exit(0);
}

// 8. Publish: the version release, then the channel (created once), then the feed and DMG.
try {
  step(`Creating the GitHub release ${tag} (not marked latest)`);
  run("gh", versionRelease);
  if (!succeeds("gh", ["release", "view", CHANNEL, "--repo", REPOSITORY])) {
    step(`Creating the ${CHANNEL} release (once)`);
    run("gh", channelCreate);
  }
  step(`Replacing ${FEED_ASSET} and the stable DMG on ${CHANNEL}`);
  run("gh", channelUpload);
  // Sirus's feed and the website's DMG link follow the repository's latest release.
  const latest = capture("gh", ["api", `repos/${REPOSITORY}/releases/latest`, "--jq", ".tag_name"]);
  if (latest.startsWith("sirus-")) throw new Error(`GitHub now calls ${latest} the repository's latest release; Sirus installs would read the wrong feed. Mark the previous Sirus release as latest again.`);
  console.log(`  The repository's latest release is still ${latest}.`);
} catch (error) {
  fail(`${error instanceof Error ? error.message : String(error)}
  Finish by hand once the cause is fixed:
    ${printed(versionRelease)}
    ${printed(channelUpload)}`);
}
console.log(`\n✓ Sirus ${version} is published: https://github.com/${REPOSITORY}/releases/tag/${tag}`);
console.log("  Installed apps offer it within a few hours (or at once from Settings → Avançado → Buscar atualização).");
