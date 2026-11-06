import { createContext } from "react";

/** Opens the sheet of a subagent: the reply that launched it and its activity row. */
export const OpenSubagent = createContext<(messageId: string, itemId: string) => void>(() => undefined);
