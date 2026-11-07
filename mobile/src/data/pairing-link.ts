// Reading a pairing link or QR, free of the phone's storage so tests can run it.

export interface AccessToken { readonly clientId: string; readonly clientSecret: string }

/** The headers Cloudflare Access checks for a service token; none without one. */
export function accessHeaders(access: AccessToken | undefined): Record<string, string> {
  return access ? { "CF-Access-Client-Id": access.clientId, "CF-Access-Client-Secret": access.clientSecret } : {};
}

/** The service token in a `sirus://pair` link (`cfid` and `cfsecret`), when both are there. */
export function accessFromLink(query: URLSearchParams): AccessToken | undefined {
  const clientId = query.get("cfid")?.trim(), clientSecret = query.get("cfsecret")?.trim();
  const valid = (value: string | undefined): value is string => !!value && value.length <= 200 && /^[A-Za-z0-9._-]+$/.test(value);
  return valid(clientId) && valid(clientSecret) ? { clientId, clientSecret } : undefined;
}

/** What a pairing QR or link holds; null when it isn't one of ours. */
export function pairingFromLink(text: string): { address: string; code: string; access?: AccessToken } | null {
  const value = text.trim();
  try {
    const app = value.startsWith("sirus://pair");
    const url = new URL(app ? value.replace("sirus://", "https://sirus.invalid/") : value);
    const address = app ? url.searchParams.get("address") ?? "" : url.origin;
    const code = url.searchParams.get(app ? "code" : "pair") ?? "";
    if (!address || !/^\d{6}$/.test(code.replace(/\D/g, ""))) return null;
    return { address, code, access: accessFromLink(url.searchParams) };
  } catch {
    return null;
  }
}
