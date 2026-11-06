import type { AppSettings } from "../client/types";

export type ThreadOrder = AppSettings["sidebarThreadSortOrder"];

export type RowMenuId = "copy-id" | "return" | "arrange" | "pin" | "unpin" | "rename" | "delete";

export interface RowMenuItem {
  id: RowMenuId;
  disabled?: boolean;
  destructive?: boolean;
  /** Opens a second card with these choices, the current one checked. */
  submenu?: { id: string; checked: boolean }[];
}

export interface RowMenuInput {
  /** Where the row is listed: Home offers "Arrange", snoozed rows their own way back. */
  place: "home" | "list" | "snoozed";
  pinned: boolean;
  /** Running or waiting: it cannot be deleted. */
  active: boolean;
  order: ThreadOrder;
}

/**
 * A conversation's long-press menu, in this order: copy the id, its lifecycle step
 * (return from snooze), arrange, pin, rename, delete.
 */
export function rowMenuItems(input: RowMenuInput): RowMenuItem[] {
  const listed = input.place === "home" || input.place === "list";
  const items: RowMenuItem[] = [{ id: "copy-id" }];
  if (input.place === "snoozed") items.push({ id: "return" });
  if (input.place === "home") items.push({ id: "arrange", submenu: (["updated_at", "created_at"] as const).map((id) => ({ id, checked: input.order === id })) });
  if (listed) items.push({ id: input.pinned ? "unpin" : "pin" });
  items.push({ id: "rename" });
  items.push({ id: "delete", destructive: true, disabled: input.active });
  return items;
}
