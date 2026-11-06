import type { ApprovalMode, ResponseStreaming, AgentProviderId, AppSettings } from "../client/types";

/** Settings → Default permissions (ADR-130): the composer's own modes, labels and lines. */
export const APPROVAL_CHOICES: readonly { mode: ApprovalMode; label: string; help: string }[] = [
  { mode: "ask", label: "composer.askApproval", help: "composer.askApprovalDescription" },
  { mode: "auto", label: "composer.autoReview", help: "composer.autoReviewDescription" },
  { mode: "full", label: "composer.fullAccess", help: "composer.fullAccessDescription" },
];

/** Settings → Response streaming (ADR-130), plus the word-by-word default. */
export const STREAMING_CHOICES: readonly { mode: ResponseStreaming; label: string; help: string }[] = [
  { mode: "turn", label: "settings.streaming.turn", help: "settings.streaming.turnHelp" },
  { mode: "paragraph", label: "settings.streaming.paragraph", help: "settings.streaming.paragraphHelp" },
  { mode: "token", label: "settings.streaming.token", help: "settings.streaming.tokenHelp" },
];

/** Settings → Default session workspace, with a line for each choice. */
export const WORKSPACE_CHOICES: readonly { mode: AppSettings["defaultSessionWorkspace"]; label: string; help: string }[] = [
  { mode: "ask", label: "Ask every time", help: "settings.workspace.askHelp" },
  { mode: "checkout", label: "Current checkout", help: "settings.workspace.checkoutHelp" },
  { mode: "worktree", label: "New worktree", help: "settings.workspace.worktreeHelp" },
];

export function isProviderEnabled(settings: AppSettings, id: AgentProviderId) {
  return !settings.disabledProviders.includes(id);
}

export function modelKey(provider: AgentProviderId, modelId: string) {
  return `${provider}::${modelId}`;
}

export function isModelFavorite(settings: AppSettings, provider: AgentProviderId, modelId: string) {
  return settings.favoriteModels.includes(modelKey(provider, modelId));
}
