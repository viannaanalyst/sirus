const path = require("node:path");
const { getDefaultConfig } = require("expo/metro-config");
const { withUniwindConfig } = require("uniwind/metro");
const extraThemes = require("./generated-uniwind-theme-names.json");

/** @type {import("expo/metro-config").MetroConfig} */
const config = getDefaultConfig(__dirname);

// The pure client code the phone shares with the Mac's protocol lives in `src/shared` (ADR-135, ADR-191): `@sirus/…` points there.
const sharedRoot = path.resolve(__dirname, "src/shared");
const resolveDefault = config.resolver.resolveRequest;
config.resolver.resolveRequest = (context, moduleName, platform) => {
  const resolve = resolveDefault ?? context.resolveRequest;
  // The icon pack's CommonJS build exports nothing usable; its ES module does (file icons).
  if (moduleName === "react-material-icon-theme") return { type: "sourceFile", filePath: path.join(__dirname, "node_modules/react-material-icon-theme/dist/index.esm.js") };
  if (moduleName.startsWith("@sirus/")) return resolve(context, path.join(sharedRoot, moduleName.slice("@sirus/".length)), platform);
  return resolve(context, moduleName, platform);
};

module.exports = withUniwindConfig(config, {
  cssEntryFile: "./global.css",
  extraThemes,
  polyfills: { rem: 14 },
});
