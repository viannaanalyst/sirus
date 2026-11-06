export type LinkSegment = { kind: "text"; text: string } | { kind: "link"; text: string; url: string };

// Top-level domains a bare address may end in. Kept explicit so file names such as
// "package.json", "index.ts" or "README.md" never read as links.
const TLDS = "com|net|org|br|io|dev|app|gov|edu|co|ai|info|tech|xyz|me|gg|tv|us|uk|ca|de|fr|es|it|pt|nl|eu|ar|mx|cl|jp|in|au|ch|site|online|store|cloud|blog|so|ly|to|fm|am|vc|ws";

/** An http(s) URL, or a bare address like "portaldatransparencia.gov.br/api-de-dados" (opened as https). */
const URL_PATTERN = new RegExp(`\\bhttps?:\\/\\/[^\\s<>"'\`]+|(?<![@\\w./-])(?:[a-z0-9](?:[a-z0-9-]*[a-z0-9])?\\.)+(?:${TLDS})\\b(?![.-]?\\w)(?:\\/[^\\s<>"'\`]*)?`, "gi");

/** Closing punctuation that ends a sentence rather than the address. */
const TRAILING = /[.,;:!?)\]}'"»”]+$/;

/** Splits plain text into text and link segments (http(s) URLs and bare web addresses), trimming sentence punctuation off links. */
export function splitLinks(text: string): LinkSegment[] {
  const segments: LinkSegment[] = [];
  let cursor = 0;
  for (const match of text.matchAll(URL_PATTERN)) {
    let url = match[0];
    // Keep a closing parenthesis that belongs to the URL (Wikipedia-style addresses).
    const trailing = TRAILING.exec(url)?.[0] ?? "";
    if (trailing) {
      const balanced = trailing.startsWith(")") && (url.match(/\(/g)?.length ?? 0) >= (url.match(/\)/g)?.length ?? 0);
      url = balanced ? url : url.slice(0, url.length - trailing.length);
    }
    const start = match.index ?? 0;
    if (start > cursor) segments.push({ kind: "text", text: text.slice(cursor, start) });
    segments.push({ kind: "link", text: url, url: /^https?:\/\//i.test(url) ? url : `https://${url}` });
    cursor = start + url.length;
  }
  if (cursor < text.length) segments.push({ kind: "text", text: text.slice(cursor) });
  return segments;
}
