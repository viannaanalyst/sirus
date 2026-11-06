export const KEYBINDINGS = [
  { id: "new-session", label: "New Session", combo: "meta+n", group: "Session" },
  { id: "palette", label: "Command Palette", combo: "meta+k", group: "App" },
  { id: "open-project", label: "Open Project", combo: "meta+o", group: "Project" },
  { id: "settings", label: "Settings", combo: "meta+,", group: "App" },
  { id: "toggle-sidebar", label: "Toggle Sidebar", combo: "meta+s", group: "View" },
  { id: "toggle-context", label: "Toggle right panel", combo: "meta+\\", group: "View" },
  { id: "open-terminal", label: "Open Terminal", combo: "meta+`", group: "View" },
  { id: "stop-agent", label: "Stop Agent", combo: "meta+.", group: "Session" },
  { id: "back", label: "Back", combo: "meta+[", group: "App" },
  { id: "forward", label: "Forward", combo: "meta+]", group: "App" },
  { id: "open-files", label: "Open Files", combo: "meta+alt+o", group: "View" },
  { id: "open-browser", label: "Open Browser", combo: "meta+alt+b", group: "View" },
  { id: "toggle-environment", label: "Toggle Environment", combo: "meta+alt+e", group: "View" },
  { id: "find-in-conversation", label: "Find in conversation", combo: "meta+f", group: "Session" },
  { id: "search-conversations", label: "Search all conversations", combo: "meta+shift+f", group: "Session" },
  { id: "toggle-side-chat", label: "Toggle side chat", combo: "meta+alt+s", group: "Session" },
  { id: "send-new-thread", label: "Send and start new thread", combo: "meta+alt+enter", group: "Session" },
  { id: "cycle-effort", label: "Cycle reasoning effort", combo: "shift+tab", group: "Session" },
  { id: "reopen-closed", label: "Reopen closed tab", combo: "meta+shift+t", group: "View" },
] as const;

export type ShortcutId = (typeof KEYBINDINGS)[number]["id"];
