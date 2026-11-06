export function toggleSidebarId(ids: readonly string[], id: string): string[] {
  return ids.includes(id) ? ids.filter((value) => value !== id) : [id, ...ids].slice(0, 4096);
}
