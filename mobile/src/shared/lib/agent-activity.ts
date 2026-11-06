import type { TurnActivity, InputQuestion } from "../client/types";

/** Display-only clock; native snapshots determine pauses and finality. */
export function activityElapsed(activity: TurnActivity, now: number): number {
  const end = activity.endedAt ?? activity.waitingSince ?? now;
  return Math.max(0, Math.floor((end - activity.startedAt - activity.pausedMs) / 1000));
}

export function formatActivityDuration(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  if (total < 60) return `${total}s`;
  if (total < 3600) return `${Math.floor(total / 60)}m ${total % 60}s`;
  return `${Math.floor(total / 3600)}h ${Math.floor(total / 60) % 60}m`;
}

export function canAnswer(question: InputQuestion, value: string | undefined): boolean {
  if (!value?.trim() || question.isSecret || new TextEncoder().encode(value).length > 8192) return false;
  return !question.options || question.isOther || question.options.some((option) => option.label === value);
}
