import { registerHooks } from "node:module";
import { existsSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { resolve, extname } from "node:path";
// Lets `node --test` load the phone's TypeScript: `@sirus/…` points at src/shared, and extensionless relative imports get `.ts`.
const shared = fileURLToPath(new URL("../src/shared/", import.meta.url));
registerHooks({
  resolve(specifier, context, nextResolve) {
    let candidate;
    if (specifier.startsWith("@sirus/")) candidate = resolve(shared, specifier.slice(7));
    else if (specifier.startsWith(".") && context.parentURL?.startsWith("file:") && !extname(specifier)) candidate = fileURLToPath(new URL(specifier, context.parentURL));
    if (candidate) {
      for (const path of [candidate + ".ts", resolve(candidate, "index.ts")]) {
        if (existsSync(path)) return nextResolve(pathToFileURL(path).href, context);
      }
    }
    return nextResolve(specifier, context);
  },
});
