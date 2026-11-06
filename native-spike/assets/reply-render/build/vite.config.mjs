import { defineConfig } from "vite";
// Builds the two libraries `reply_render.rs` loads in its hidden page. See build.sh.
export default defineConfig({
  logLevel: "warn",
  build: {
    base: "./", outDir: "out", emptyOutDir: true, minify: true, modulePreload: false, cssCodeSplit: true, target: "safari16",
    rollupOptions: {
      input: { mermaid: "mermaid-entry.js", katex: "katex-entry.js" },
      preserveEntrySignatures: "exports-only",
      output: {
        entryFileNames: "[name].js",
        chunkFileNames: "chunks/[name]-[hash].js",
        assetFileNames: (asset) => (asset.names?.[0] ?? "").endsWith(".css") ? "katex.css" : "[name]-[hash][extname]",
      },
    },
  },
});
