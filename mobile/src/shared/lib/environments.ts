import type { AppSettings } from "../client/types";
import { toggleSidebarId } from "./sidebar-layout";

/** The settings-backed row actions, applied to the settings of the computer the row lives on. */
export type RowSettingsAction = "pin" | "unpin";

export function applyRowSettingsAction(settings: AppSettings, action: RowSettingsAction, sessionId: string): AppSettings {
  switch (action) {
    case "pin": return settings.pinnedSessionIds.includes(sessionId) ? settings : { ...settings, pinnedSessionIds: toggleSidebarId(settings.pinnedSessionIds, sessionId) };
    case "unpin": return { ...settings, pinnedSessionIds: settings.pinnedSessionIds.filter((id) => id !== sessionId) };
  }
}
