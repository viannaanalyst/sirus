/**
 * Pairs with the Sirus at `baseUrl` (empty: this page's own), cross-origin for another of the
 * owner's computers (ADR-132); also returns what that computer says about itself.
 */
export async function redeemPairingAt(baseUrl: string, code: string, name: string, request: typeof fetch = fetch): Promise<{ token: string; host?: { id?: string; hostName?: string; kind?: string } }> {
  const response = await request(`${baseUrl}/api/pair`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ code, name }),
  });
  const body = await response.json().catch(() => ({})) as { token?: unknown; message?: unknown; host?: unknown };
  if (!response.ok || typeof body.token !== "string") {
    throw new Error(typeof body.message === "string" ? body.message : "The Mac refused this pairing code.");
  }
  const host = body.host && typeof body.host === "object" ? body.host as { id?: string; hostName?: string; kind?: string } : undefined;
  return { token: body.token, host };
}

/** The address of the Mac's socket for a page it served. */
export function socketUrl(location: Pick<Location, "protocol" | "host">): string {
  return `${location.protocol === "https:" ? "wss:" : "ws:"}//${location.host}/api/socket`;
}

/** The socket of the Sirus at an origin such as `https://vps.example.com` (ADR-132). */
export function socketUrlFor(baseUrl: string): string {
  const url = new URL(baseUrl);
  return socketUrl({ protocol: url.protocol, host: url.host });
}
