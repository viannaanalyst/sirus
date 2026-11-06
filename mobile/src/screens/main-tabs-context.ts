import { createContext, use } from "react";

/** The three places of the top bar's pill: a new conversation, the conversations, the dashboard. */
export type MainTab = "new" | "conversations" | "dashboard";
/** What Conversas shows: every conversation, the ones waiting on the person, working, or done to review. */
export type ConversationFilter = "all" | "attention" | "working" | "review";

/** The main screen: which place is chosen, the project picked in the drawer, and the drawer itself. */
export const MainTabContext = createContext<{
  readonly selected: MainTab;
  readonly open: (tab: MainTab, filter?: ConversationFilter) => void;
  readonly filter: ConversationFilter;
  readonly setFilter: (filter: ConversationFilter) => void;
  /** `computerId:projectId` of the project chosen in the drawer; null shows every project. */
  readonly project: string | null;
  readonly setProject: (key: string | null) => void;
  readonly drawerOpen: boolean;
  readonly setDrawerOpen: (open: boolean) => void;
  /** Space the top bar takes (safe area plus the bar), for the screens' content to start below it. */
  readonly topInset: number;
} | null>(null);

export function useMainTabs() {
  return use(MainTabContext);
}
