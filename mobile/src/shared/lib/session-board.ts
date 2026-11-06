/** Compact relative time for switcher and inbox rows (6d, 2h, 5m). */
export function relativeTime(at: string, now = Date.now()): string {
  const time = Date.parse(at);
  if (!Number.isFinite(time)) return "";
  const seconds = Math.max(0, Math.round((now - time) / 1000));
  if (seconds < 60) return "now";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d`;
  const months = Math.floor(days / 30);
  if (months < 12) return `${months}mo`;
  return `${Math.floor(months / 12)}y`;
}
