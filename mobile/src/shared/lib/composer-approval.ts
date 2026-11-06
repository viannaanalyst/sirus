import type { AgentProviderId, ApprovalMode, Session } from "../client/types";
import { providerById } from "./provider-registry";

/** From most to least permissive. */
const APPROVAL_ORDER: readonly ApprovalMode[] = ["full", "auto", "ask"];

/**
 * The mode a new conversation starts on (ADR-130): the owner's default (Settings → Default
 * permissions, Full access unless changed) when the provider offers it, else the next safer
 * mode it offers (step down, never up), else its first mode.
 */
export function defaultApprovalFor(modes: readonly ApprovalMode[], preferred: ApprovalMode = "full"): ApprovalMode {
  const safer = APPROVAL_ORDER.slice(APPROVAL_ORDER.indexOf(preferred)).find((mode) => modes.includes(mode));
  return safer ?? modes[0] ?? "ask";
}

/**
 * The approval mode a composer sends with (shared by the desktop composer and the phone's
 * Thread settings sheet). A session keeps the mode it last used; anything new starts on the
 * owner's default (`preferred`, Full access unless changed in Settings) when the provider
 * offers it. Planning never runs on Full access: it asks instead (Cursor reviews automatically).
 */
export function composerApproval(
  provider: AgentProviderId,
  session: Pick<Session, "agent" | "execution"> | null | undefined,
  chosen: Partial<Record<AgentProviderId, ApprovalMode>> | undefined,
  planning: boolean,
  preferred: ApprovalMode = "full",
): { requested: ApprovalMode; effective: ApprovalMode } {
  const definition = providerById(provider);
  const remembered = session && session.agent === provider ? session.execution?.approval ?? undefined : undefined;
  const selected = chosen?.[provider] ?? remembered;
  const fallback = defaultApprovalFor(definition.approvalModes, preferred);
  const requested = selected && definition.approvalModes.includes(selected) ? selected : fallback;
  const effective = planning && requested === "full" ? provider === "cursor" ? "auto" : "ask" : requested;
  return { requested, effective };
}
