import type { ActivityItem, PendingRequest, TurnActivity } from "@sirus/client/types";

/** Pure derivations behind the Painel: greeting, request and step wording, hourly activity, the feed. */
const WEEKDAYS = ["domingo", "segunda", "terça", "quarta", "quinta", "sexta", "sábado"];
const MONTHS = ["janeiro", "fevereiro", "março", "abril", "maio", "junho", "julho", "agosto", "setembro", "outubro", "novembro", "dezembro"];

export function greetingFor(now: Date): string {
  const hour = now.getHours();
  return hour < 5 ? "Boa noite" : hour < 12 ? "Bom dia" : hour < 18 ? "Boa tarde" : "Boa noite";
}

/** "sexta, 6 de novembro". */
export function dateLine(now: Date): string {
  return `${WEEKDAYS[now.getDay()]}, ${now.getDate()} de ${MONTHS[now.getMonth()]}`;
}

/** "agora", "9 min", "2 h", "3 d": how long ago, short. */
export function agoShort(at: number, now: number): string {
  const seconds = Math.max(0, Math.round((now - at) / 1000));
  if (seconds < 60) return "agora";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours} h`;
  return `${Math.floor(hours / 24)} d`;
}

const basename = (path: string) => path.split("/").filter(Boolean).pop() ?? path;
const oneLine = (text: string, max = 60) => { const line = text.trim().split("\n")[0] ?? ""; return line.length > max ? `${line.slice(0, max - 1)}…` : line; };

/** What a pending request asks, in a few words: the verb and the thing in code style. */
export function describeRequest(request: PendingRequest): { readonly text: string; readonly code: string | null; readonly question: boolean } {
  const kind = request.kind;
  if (kind.type === "command") return { text: "quer rodar", code: oneLine(kind.command), question: false };
  if (kind.type === "fileChange") {
    const first = kind.changes[0];
    return { text: kind.changes.length > 1 ? `quer alterar ${kind.changes.length} arquivos` : "quer alterar", code: kind.changes.length > 1 || !first ? null : basename(first.path), question: false };
  }
  if (kind.type === "tool") return { text: "quer usar", code: kind.name, question: false };
  return { text: "tem uma pergunta", code: null, question: true };
}

/** A running step in pt-BR ("Editando AreaLogada.tsx"), from the turn's newest running item. */
export function currentStep(turn: TurnActivity | undefined): string | null {
  if (!turn || turn.endedAt) return null;
  const running = [...turn.items].reverse().find((item) => item.state === "running" && !item.finishedTask);
  const edited = new Set(turn.items.filter((item) => item.kind === "edit" && item.detail).map((item) => item.detail!)).size;
  const base = running ? stepWords(running) : turn.thought ? oneLine(turn.thought, 80) : null;
  if (!base) return null;
  return edited > 1 && running?.kind === "edit" ? `${base} · ${edited} arquivos` : base;
}

function stepWords(item: ActivityItem): string {
  const detail = item.detail ? item.kind === "command" ? oneLine(item.detail, 48) : basename(item.detail) : "";
  switch (item.kind) {
    case "edit": return detail ? `Editando ${detail}` : "Editando arquivos";
    case "read": return detail ? `Lendo ${detail}` : "Lendo arquivos";
    case "command": return detail ? `Rodando ${detail}` : "Rodando um comando";
    case "agent": return item.label ? `Subagente: ${oneLine(item.label, 40)}` : "Subagente trabalhando";
    case "skill": return `Usando ${detail || item.label || "uma skill"}`;
    default: return item.label ? oneLine(item.label, 50) : "Trabalhando";
  }
}

/** Conversations that moved in each hour of today (24 buckets), from last activity and the latest turn's start. */
export function hourlyActivity(moments: readonly number[], now: number): number[] {
  const startOfDay = new Date(now);
  startOfDay.setHours(0, 0, 0, 0);
  const from = startOfDay.getTime();
  const counts = Array.from({ length: 24 }, () => 0);
  for (const moment of moments) {
    if (moment < from || moment > now + 60_000) continue;
    counts[new Date(moment).getHours()]! += 1;
  }
  return counts;
}

/** Lines a turn added and removed, from its review; null without one. */
export function turnLines(turn: TurnActivity | undefined): { readonly additions: number; readonly deletions: number; readonly files: number } | null {
  const files = turn?.review?.files;
  if (!files?.length) return null;
  return { additions: files.reduce((sum, file) => sum + file.additions, 0), deletions: files.reduce((sum, file) => sum + file.deletions, 0), files: files.length };
}

export interface FeedInput {
  readonly key: string;
  readonly projectName: string;
  readonly title: string;
  readonly status: "approval" | "input" | "working" | "waiting" | "failed" | null;
  readonly lastActivityAt: string;
  readonly turn: TurnActivity | undefined;
  readonly request: PendingRequest | undefined;
  readonly done: boolean;
}
export interface FeedEvent {
  readonly key: string;
  readonly at: number;
  readonly working: boolean;
  /** Plain text pieces so the view can bold names and colour the line counts. */
  readonly parts: readonly { readonly text: string; readonly strong?: boolean; readonly tone?: "add" | "del" | "danger" | "warn" }[];
}

/** The newest events across conversations, from what each one's state says right now. */
export function buildFeed(inputs: readonly FeedInput[], now: number, limit = 6): FeedEvent[] {
  const events: FeedEvent[] = [];
  for (const input of inputs) {
    const at = (value: number | null | undefined) => value ?? Date.parse(input.lastActivityAt);
    if (input.status === "approval" || input.status === "input") {
      const ask = input.request ? describeRequest(input.request) : null;
      events.push({ key: `${input.key}:ask`, at: at(input.turn?.waitingSince), working: false, parts: [
        { text: input.projectName, strong: true }, { text: ask ? ` ${ask.text} ` : input.status === "input" ? " tem uma pergunta" : " pede aprovação" }, ...(ask?.code ? [{ text: ask.code, strong: true }] : []),
      ] });
    } else if (input.status === "working" || input.status === "waiting") {
      const step = currentStep(input.turn);
      const lines = turnLines(input.turn);
      events.push({ key: `${input.key}:work`, at: Date.parse(input.lastActivityAt), working: input.status === "working", parts: lines
        ? [{ text: "Editou " }, { text: `${lines.files} ${lines.files === 1 ? "arquivo" : "arquivos"}`, strong: true }, { text: ` +${lines.additions}`, tone: "add" }, { text: ` −${lines.deletions}`, tone: "del" }]
        : [{ text: step ?? `${input.projectName} está trabalhando` }] });
    } else if (input.status === "failed") {
      events.push({ key: `${input.key}:fail`, at: Date.parse(input.lastActivityAt), working: false, parts: [{ text: "Falhou: ", tone: "danger" }, { text: input.title, strong: true }] });
    } else if (input.done) {
      events.push({ key: `${input.key}:done`, at: at(input.turn?.endedAt), working: false, parts: [{ text: "Terminou " }, { text: input.title, strong: true }] });
    }
  }
  return events.filter((event) => Number.isFinite(event.at) && now - event.at < 24 * 3_600_000).sort((a, b) => b.at - a.at).slice(0, limit);
}
