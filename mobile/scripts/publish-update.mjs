// Publishes the app's JavaScript as an update the computers serve (ADR-135 addendum):
// `expo export` for iOS, then `manifest.json` (Expo Updates protocol v1) plus the files go to
// `<Sirus data dir>/app-updates/ios/<runtime version>/`, swapped in whole so the app never sees
// half an update. `--data-dir <path>` picks another data folder (a host's); the default is the
// Mac app's. The runtime version is `RUNTIME_VERSION` in app.config.ts.
import { execFileSync } from "node:child_process";
import { createHash, randomUUID } from "node:crypto";
import { cpSync, existsSync, mkdirSync, readFileSync, renameSync, rmSync, writeFileSync } from "node:fs";
import { homedir, tmpdir } from "node:os";
import { dirname, join } from "node:path";

const root = join(import.meta.dirname, "..");
const args = process.argv.slice(2);
const flag = (name) => { const at = args.indexOf(name); return at >= 0 ? args[at + 1] : undefined; };
const dataDir = flag("--data-dir") ?? join(homedir(), "Library", "Application Support", "com.siruscode.native");
const runtime = /const RUNTIME_VERSION = "([^"]+)"/.exec(readFileSync(join(root, "app.config.ts"), "utf8"))?.[1];
if (!runtime) throw new Error("RUNTIME_VERSION not found in app.config.ts");
if (!existsSync(dataDir)) throw new Error(`No Sirus data folder at ${dataDir}`);

const exported = join(tmpdir(), `sirus-update-${process.pid}`);
rmSync(exported, { recursive: true, force: true });
execFileSync("npx", ["expo", "export", "--platform", "ios", "--output-dir", exported], { cwd: root, stdio: "inherit" });
const metadata = JSON.parse(readFileSync(join(exported, "metadata.json"), "utf8")).fileMetadata.ios;

const sha256 = (path) => createHash("sha256").update(readFileSync(join(exported, path)));
const entry = (path, ext, contentType) => ({
  hash: sha256(path).digest("base64url"),
  key: sha256(path).digest("hex").slice(0, 32),
  contentType,
  fileExtension: `.${ext}`,
  path,
});
const TYPES = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", ttf: "font/ttf", otf: "font/otf", svg: "image/svg+xml", json: "application/json" };
const manifest = {
  id: randomUUID(),
  createdAt: new Date().toISOString(),
  runtimeVersion: runtime,
  launchAsset: entry(metadata.bundle, "bundle", "application/javascript"),
  assets: metadata.assets.map((asset) => entry(asset.path, asset.ext, TYPES[asset.ext] ?? "application/octet-stream")),
  metadata: {},
  extra: {},
};

const target = join(dataDir, "app-updates", "ios", runtime);
const staging = `${target}.staging`;
rmSync(staging, { recursive: true, force: true });
mkdirSync(join(staging, "files"), { recursive: true });
for (const path of [metadata.bundle, ...metadata.assets.map((asset) => asset.path)]) {
  mkdirSync(dirname(join(staging, "files", path)), { recursive: true });
  cpSync(join(exported, path), join(staging, "files", path));
}
writeFileSync(join(staging, "manifest.json"), JSON.stringify(manifest, null, 2));
rmSync(`${target}.old`, { recursive: true, force: true });
if (existsSync(target)) renameSync(target, `${target}.old`);
renameSync(staging, target);
rmSync(`${target}.old`, { recursive: true, force: true });
rmSync(exported, { recursive: true, force: true });
console.log(`Published update ${manifest.id} (runtime ${runtime}, ${manifest.assets.length} assets) to ${target}`);
