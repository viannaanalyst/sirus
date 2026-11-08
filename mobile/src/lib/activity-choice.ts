/** Which conversation the lock screen shows: the one that spoke last, of those still working. */
export function activityChoice<T extends { readonly status: string; readonly lastActivityAt: string }>(sessions: readonly T[]): T | undefined {
  return sessions
    .filter((session) => session.status === "running" || session.status === "starting" || session.status === "waiting")
    .reduce<T | undefined>((latest, session) => (!latest || session.lastActivityAt > latest.lastActivityAt ? session : latest), undefined);
}
