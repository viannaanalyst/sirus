# ADR-160: LaTeX math in replies

**Status:** Accepted (2026-10-21).

## Context

Agents write formulas in LaTeX (`$E = mc^2$`, `$$\int_0^1 x\,dx$$`). Replies showed the raw TeX. Another open-source client renders it on web and mobile.

## Decision

- **Parsing** (`chat-markdown.ts`): inline `$…$`, `\(…\)` and `$$…$$` become `math` inline nodes; `$$…$$` or `\[…\]` starting a line become a display `math` block, on one line or until the line that closes it, never across a blank line, so the streamed parse (`chat-markdown-stream.ts`) still settles at blank lines. Pandoc's rule keeps prices and shell variables as text: the opening `$` is followed by a non-space and not preceded by a word character or `$`, the closing one follows a non-space and is not followed by a digit; `\$` is a literal dollar. Formulas over 4,000 characters stay text. Fenced code is untouched.
- **Rendering** (`MathFormula.tsx`): KaTeX and its stylesheet load on the first formula (KaTeX already ships with Mermaid, so the bundle only gains a lazy chunk). `trust: false` refuses `\href`, `\includegraphics` and HTML commands, `maxSize` and `maxExpand` bound pathological input, and KaTeX escapes the rest, so its markup is inserted as is. TeX that does not parse, and anything shown before KaTeX loads, falls back to its source as inline code. Results are cached per source.

## Consequences

- Formulas read as math in replies, pull request bodies and anywhere else `ChatMarkdown` renders.
- Text with two `$` on a line can still read as math when it meets the rule (`$a$`); `\$` escapes it.
