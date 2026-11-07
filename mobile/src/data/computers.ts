import * as SecureStore from "expo-secure-store";

import { redeemPairingAt } from "@sirus/client/remote-pairing";

import { accessFromLink, accessHeaders, pairingFromLink, type AccessToken } from "./pairing-link";

export { accessFromLink, accessHeaders, pairingFromLink };
export type { AccessToken };

/**
 * The owner's computers this phone is paired with (ADR-135): the Mac and any Sirus host,
 * reached over their secure addresses. The list, tokens included, lives in the iPhone's
 * keychain.
 */
export interface Computer {
  /** `host:<id>` from the computer's own identity, else `url:<address>`. */
  readonly id: string;
  readonly baseUrl: string;
  readonly token: string;
  readonly hostName: string;
  readonly kind: "mac" | "server";
  /** The Cloudflare Access service token of a public address (ADR-155), sent with every request. */
  readonly access?: AccessToken;
  /** The address and token this computer had before (the one it used first), kept for app updates. */
  readonly previous?: { readonly baseUrl: string; readonly token: string };
}


const KEY = "sirus.computers";

export async function readComputers(): Promise<Computer[]> {
  try {
    const raw = await SecureStore.getItemAsync(KEY);
    const value = raw ? JSON.parse(raw) as unknown : [];
    return Array.isArray(value) ? value.filter((item): item is Computer => typeof item?.id === "string" && typeof item.baseUrl === "string" && typeof item.token === "string") : [];
  } catch {
    return [];
  }
}

export async function writeComputers(computers: readonly Computer[]) {
  await SecureStore.setItemAsync(KEY, JSON.stringify(computers));
}

/** `mac.example.com` or a full address, as an `https://` origin; null when it is not one. */
export function normalizeAddress(input: string): string | null {
  const value = input.trim();
  if (!value) return null;
  try {
    const url = new URL(/^[a-z][a-z0-9+.-]*:\/\//i.test(value) ? value : `https://${value}`);
    if (url.protocol !== "https:" || url.username || url.password || !url.hostname.includes(".")) return null;
    return url.origin;
  } catch {
    return null;
  }
}

/** Exchanges a pairing code at `address` for this phone's token and that computer's identity. */
export async function pairComputer(address: string, code: string, access?: AccessToken): Promise<Computer> {
  const baseUrl = normalizeAddress(address);
  if (!baseUrl) throw new Error("Use o endereço https:// do computador.");
  const headers = accessHeaders(access);
  const request: typeof fetch = (input, init) => fetch(input, { ...init, headers: { ...(init?.headers as Record<string, string> | undefined), ...headers } });
  const paired = await redeemPairingAt(baseUrl, code.replace(/\D/g, ""), "iPhone · Sirus", request);
  return {
    id: paired.host?.id ? `host:${paired.host.id}` : `url:${baseUrl}`,
    baseUrl,
    token: paired.token,
    hostName: paired.host?.hostName?.trim() ?? "",
    kind: paired.host?.kind === "server" ? "server" : "mac",
    ...(access ? { access } : {}),
  };
}

/** What the phone calls a computer. */
export function computerName(computer: Pick<Computer, "hostName" | "kind">): string {
  return computer.hostName || (computer.kind === "mac" ? "Mac" : "Servidor");
}
