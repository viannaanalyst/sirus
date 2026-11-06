/**
 * Credential masking for approval cards and activity rows (ADR-077). Known token shapes keep
 * their prefix and last four characters (`sk-••••abcd`); credential-named flags, assignments,
 * headers and URL passwords lose their value. Mirrors `src-tauri/src/redact.rs`, which masks
 * activity natively before it is stored.
 */

export const DOTS = "••••";

/** `••••` plus the last four characters, or only `••••` under 16 characters. */
export function maskValue(value: string): string {
  const chars = Array.from(value);
  return chars.length < 16 ? DOTS : `${DOTS}${chars.slice(-4).join("")}`;
}

function words(name: string): string[] {
  return name.replace(/([A-Z]+)([A-Z][a-z])/g, "$1 $2").replace(/([a-z0-9])([A-Z])/g, "$1 $2").toLowerCase().match(/[a-z0-9]+/g) ?? [];
}

const exactNames = new Set(["apikey", "accesskey", "accesskeyid", "authtoken", "authorization", "clientsecret", "privatekey", "secretkey", "sessiontoken", "refreshtoken", "idtoken", "token", "pat"]);

const secretWords = new Set(["password", "passwd", "pwd", "pass", "passphrase", "secret", "secrets", "credential", "credentials", "authorization", "cookie"]);

const counterQualifiers = new Set(["max", "prompt", "completion", "total", "input", "output", "csrf", "next"]);

const keyQualifiers = new Set(["api", "private", "secret", "access", "signing", "encryption", "master"]);

/** True when a key, flag or variable names a credential (`GITHUB_TOKEN`, `apiKey`), not a counter (`max_tokens`). */
export function sensitiveName(name: string): boolean {
  const parts = words(name);
  if (exactNames.has(parts.join(""))) return true;
  const last = parts.at(-1);
  if (!last) return false;
  if (secretWords.has(last)) return true;
  if (last === "token") { const qualifier = parts.at(-2); return qualifier === undefined || !counterQualifiers.has(qualifier); }
  return last === "key" && parts.slice(0, -1).some((part) => keyQualifiers.has(part));
}

function entropy(value: string): number {
  const counts = new Map<string, number>();
  for (const char of value) counts.set(char, (counts.get(char) ?? 0) + 1);
  let sum = 0;
  for (const count of counts.values()) { const p = count / value.length; sum -= p * Math.log2(p); }
  return sum;
}

/** A long random-looking value: letters and digits mixed, no spaces or paths, high entropy. */
export function looksRandom(value: string): boolean {
  return value.length >= 24 && !value.includes("://") && !/^[/.~$]/.test(value) && /^[A-Za-z0-9+/=_.-]+$/.test(value) && /\d/.test(value) && /[A-Za-z]/.test(value) && entropy(value) >= 3.5;
}

function placeholder(value: string): boolean {
  const bare = value.replace(/^["']+|["']+$/g, "");
  return !bare || /^[$<-]/.test(bare) || bare.includes(DOTS) || ["true", "false", "null", "none", "***", "bearer", "basic", "digest", "token"].includes(bare.toLowerCase());
}

function maskQuoted(value: string): string {
  const quote = value[0];
  return (quote === "\"" || quote === "'") && value.length >= 2 && value.endsWith(quote) ? `${quote}${maskValue(value.slice(1, -1))}${quote}` : maskValue(value);
}

const urlPassword = /([a-zA-Z][a-zA-Z0-9+.-]*:\/\/[^\s:/@]+:)([^\s@/]+)@/g;

const prefixed = /\b(sk-(?:proj-|ant-(?:api\d\d-)?|svcacct-)?|gh[pousr]_|github_pat_|xox[abposr]-|glpat-|npm_|[sr]k_(?:live|test)_|AIza)([A-Za-z0-9_-]{12,})/g;

const aws = /\b(AKIA|ASIA)([0-9A-Z]{16})\b/g;

const authorization = /(\b(?:proxy-)?authorization\b["']?\s*[:=]\s*["']?(?:(?:bearer|basic|token|digest)\s+)?|\bbearer\s+)([A-Za-z0-9._~+/=-]{6,})/gi;

const flag = /(\s|^)(--?[A-Za-z][A-Za-z0-9_-]*)(=|\s+)("[^"]*"|'[^']*'|[^\s"'-][^\s"']*)/g;

const assignment = /(["']?)([A-Za-z_][A-Za-z0-9_.-]*)(["']?\s*(?::|=)\s*)("[^"\n]*"|'[^'\n]*'|[^\s"',;&|)}\]]+)/g;

/** Masks credentials in free text; returns the same string when nothing matches. */
export function maskSecrets(text: string): string {
  if (!text) return text;
  return text
    .replace(urlPassword, (_, head: string, password: string) => `${head}${maskValue(password)}@`)
    .replace(prefixed, (_, prefix: string, rest: string) => `${prefix}${maskValue(rest)}`)
    .replace(aws, (_, prefix: string, rest: string) => `${prefix}${maskValue(rest)}`)
    .replace(authorization, (match: string, head: string, value: string) => placeholder(value) ? match : `${head}${maskValue(value)}`)
    .replace(flag, (match: string, space: string, name: string, separator: string, value: string) => sensitiveName(name.replace(/^-+/, "")) && !placeholder(value) ? `${space}${name}${separator}${maskQuoted(value)}` : match)
    .replace(assignment, (match: string, quote: string, name: string, separator: string, value: string) => {
      const bare = value.replace(/^["']+|["']+$/g, "");
      const envStyle = separator.trim() === "=" && /^[A-Z0-9_]+$/.test(name);
      return !placeholder(value) && (sensitiveName(name) || (envStyle && looksRandom(bare))) ? `${quote}${name}${separator}${maskQuoted(value)}` : match;
    });
}

/** A tool's input as shown on an approval card: credential-named fields hidden, strings masked. */
export function maskInput(value: unknown, key = ""): unknown {
  if (typeof value === "string") return key && sensitiveName(key) && value ? maskValue(value) : maskSecrets(value);
  if (Array.isArray(value)) return value.map((item) => maskInput(item));
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([name, item]) => [name, maskInput(item, name)]));
  return value;
}
