import { memo, useMemo, useState, type ReactNode } from "react";
import { Pressable, View } from "react-native";

import type { ActivityItem, Message, TurnReview } from "@sirus/client/types";
import { translate } from "@sirus/i18n";
import { activityElapsed, formatActivityDuration } from "@sirus/lib/agent-activity";
import { isSubagent, teamSize } from "@sirus/lib/subagent-team";
import { foldBoundary, markerSentence, timelineParts, type TimelinePart } from "@sirus/lib/turn-timeline";

import { AppText } from "./AppText";
import { Markdown } from "./Markdown";
import { StepTree, ThinkingRow, type StepSource } from "./thread/StepTree";
import { SubagentTeam } from "./thread/SubagentTeam";
import { Tree } from "./thread/TreeRow";
import { ProviderTile } from "./thread/Tile";
import { ShimmerText } from "./thread/Shimmer";
import { modelLabel } from "./thread/steps";
import { useNow } from "./thread/use-now";

const t = (key: string, params?: Record<string, string | number>) => translate("pt-BR", key, params);

/** The conversation's folder and how to open one of its files (file references in replies). */
export type ReplyFiles = { readonly cwd: string; readonly open: (path: string) => void; readonly image?: (link: string) => Promise<string> };

/** Steps that count in "23 passos": not the finished-task markers nor background shells. */
const countsAsStep = (item: ActivityItem) => !item.finishedTask && !item.shell;

/**
 * An agent reply as the desktop shows it: its text and its tool work in the order they
 * happened (`timelineParts`), the work as a tree and the subagents as one team card. Once the
 * turn ends, everything before the final answer folds into one "Trabalhou por …" line.
 */
export const AgentReply = memo(function AgentReply(props: {
  readonly message: Message; readonly agent: string; readonly computerId: string; readonly cwd?: string; readonly files?: ReplyFiles;
  /** The person's prompt this reply answers; the team card's headline falls back to it. */
  readonly prompt?: string;
  /** The conversation's model, for the name over the reply. */
  readonly model?: string | null;
}) {
  const { message } = props;
  const items = message.activity?.items;
  const parts = useMemo(() => timelineParts(message.content, items ?? [], message.steers), [message.content, items, message.steers]);
  const finished = !message.streaming;
  const fold = finished ? foldBoundary(parts) : 0;
  const [open, setOpen] = useState(false);
  const source = useMemo<StepSource>(() => ({ computerId: props.computerId, sessionId: message.sessionId, messageId: message.id }), [props.computerId, message.sessionId, message.id]);
  const team = useMemo(() => teamSize(items ?? []) >= 2, [items]);
  const launchId = useMemo(() => items?.find(isSubagent)?.id, [items]);

  // While the turn runs with no tool running and no text being written, the agent is thinking.
  const lastPart = parts.at(-1);
  const thinking = message.streaming && lastPart?.kind !== "text" && !(items ?? []).some((item) => item.state === "running" && !item.shell && !item.finishedTask);
  const trailing = thinking ? <ThinkingRow thought={message.activity?.thought} /> : undefined;

  const model = message.activity?.model ?? props.model;
  const head = <ReplyHead provider={props.agent} model={model} activity={message.activity} streaming={!!message.streaming}
    fold={fold > 0 ? { open, toggle: () => setOpen((value) => !value) } : undefined} />;
  if (!parts.length) return thinking ? <View className="gap-1">{head}<Tree>{trailing}</Tree></View> : null;
  const lastWork = parts.findLastIndex((part) => part.kind === "work");
  const piece = (part: TimelinePart, at: number) => <Part key={partKey(part, at)} part={part} content={message.content} cwd={props.cwd} files={props.files} source={source} review={message.activity?.review}
    team={team} launchId={launchId} agent={props.agent} prompt={props.prompt ?? ""} allItems={items ?? []} trailing={at === lastWork && at === parts.length - 1 ? trailing : undefined} />;
  // What a finished turn folds away hangs off the header, on a rail; its answer stays at the left.
  const shownParts = (from: number, to: number) => {
    if (from === 0 && fold > 0) {
      if (!open) return null;
      return <View style={{ marginLeft: 30 }} className="gap-1">
        <View pointerEvents="none" className="absolute border-b border-l border-border-subtle" style={{ left: -19, top: -7, height: 24, width: 12, borderBottomLeftRadius: 6 }} />
        {parts.slice(0, fold).map((part, at) => piece(part, at))}
      </View>;
    }
    return parts.slice(from, to).map((part, at) => piece(part, from + at));
  };
  return <View className="gap-1">
    {head}
    {shownParts(0, fold)}
    {shownParts(fold, parts.length)}
    {thinking && lastPart?.kind !== "work" ? <Tree>{trailing}</Tree> : null}
  </View>;
});

/** Over a reply, as on the Mac: the provider's tile, the model and how long it worked. With work folded away, it opens it. */
function ReplyHead(props: { readonly provider: string; readonly model?: string | null; readonly activity: Message["activity"]; readonly streaming: boolean; readonly fold?: { readonly open: boolean; readonly toggle: () => void } }) {
  const now = useNow(props.streaming);
  const name = modelLabel(props.model, props.provider);
  const seconds = props.activity ? activityElapsed(props.activity, now) : null;
  const time = seconds == null ? null : formatActivityDuration(seconds);
  const label = props.streaming ? (time ? `${name} trabalhando há ${time}` : `${name} trabalhando`) : time ? `${name} trabalhou por ${time}` : name;
  return <Pressable onPress={props.fold?.toggle} disabled={!props.fold} accessibilityRole={props.fold ? "button" : undefined}
    className="mb-1 min-h-[30px] flex-row items-center gap-2 active:opacity-60">
    <ProviderTile provider={props.provider} chevron={props.fold?.open} />
    {props.streaming
      ? <ShimmerText numberOfLines={1} className="shrink text-[14px] text-foreground-muted">{label}</ShimmerText>
      : <AppText numberOfLines={1} className="shrink text-[14px] text-foreground-muted">{label}</AppText>}
  </Pressable>;
}

function partKey(part: TimelinePart, index: number) {
  return part.kind === "text" ? `t${part.start}` : part.kind === "work" ? `w${part.items[0]?.id ?? index}` : part.kind === "marker" ? `m${part.item.id}` : `s${index}`;
}

const Part = memo(function Part(props: {
  readonly part: TimelinePart; readonly content: string; readonly cwd?: string; readonly files?: ReplyFiles; readonly source: StepSource;
  readonly review: TurnReview | null | undefined;
  readonly team: boolean; readonly launchId?: string; readonly agent: string; readonly prompt: string; readonly allItems: readonly ActivityItem[]; readonly trailing?: ReactNode;
}) {
  const { part } = props;
  if (part.kind === "text") return <Markdown markdown={props.content.slice(part.start, part.end).trim()} files={props.files} />;
  if (part.kind === "steer") return <AppText className="self-end rounded-2xl bg-card px-3 py-1.5 text-sm text-foreground-muted">{part.text}</AppText>;
  if (part.kind === "marker") return <AppText className="py-1 text-sm text-foreground-muted">{markerSentence(part.item, t)}</AppText>;
  // With a team, the subagents leave the tree: the card stands where the first one began.
  const segments: ReactNode[] = [];
  let endsWithCard = false;
  let run: ActivityItem[] = [];
  const flush = (last: boolean) => {
    if (run.length) segments.push(<StepTree key={`s${run[0]!.id}`} items={run} cwd={props.cwd} review={props.review} source={props.source} trailing={last ? props.trailing : undefined} />);
    run = [];
  };
  for (const item of part.items) {
    if (props.team && isSubagent(item)) {
      flush(false);
      endsWithCard = true;
      if (item.id === props.launchId) segments.push(<SubagentTeam key="team" items={props.allItems} messageId={props.source.messageId} provider={props.agent} prompt={props.prompt} reply={props.content} />);
    } else { run.push(item); endsWithCard = false; }
  }
  flush(true);
  // A trailing "Pensando" still needs its own tree when the part ended with the card.
  if (props.trailing && endsWithCard) segments.push(<Tree key="thinking">{props.trailing}</Tree>);
  return <>{segments}</>;
});
