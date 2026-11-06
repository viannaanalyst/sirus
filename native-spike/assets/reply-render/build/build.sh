#!/bin/sh
# Rebuilds the committed Mermaid and KaTeX files in the parent folder: the two libraries the
# window's hidden page loads (native-spike/src/reply_render.rs). They are committed so that a
# release needs no npm or Vite; run this only to update the libraries, then review the result.
set -e
cd "$(dirname "$0")"
WORK="$(mktemp -d)"
cp mermaid-entry.js katex-entry.js vite.config.mjs licenses.cjs "$WORK/"
(cd "$WORK" && npm init -y >/dev/null && npm i --no-audit --no-fund mermaid@12.1.0 katex@0.16.47 >/dev/null && npm i -D --no-audit --no-fund vite >/dev/null && npx vite build -c vite.config.mjs && node licenses.cjs out/LICENSES.txt)
# KaTeX ships woff2, woff and ttf of every font; WebKit takes the woff2, so only that stays.
sed -i '' -E 's/,url\([^)]*\.woff\)format\("woff"\),url\([^)]*\.ttf\)format\("truetype"\)//g' "$WORK/out/katex.css"
sed -i '' -E 's#url\(/#url(./#g' "$WORK/out/katex.css"
rm -f "$WORK"/out/*.woff "$WORK"/out/*.ttf
cd ..
rm -rf chunks mermaid.js katex.js katex.css LICENSES.txt KaTeX_*
cp -R "$WORK"/out/. .
rm -rf "$WORK"
