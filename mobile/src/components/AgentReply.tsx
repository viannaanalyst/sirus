import { memo, useMemo, useState, type ReactNode } from "react";
import { Pressable, View } from "react-native";

import type { ActivityItem, Message, TurnReview } from "@sirus/client/types";
import { translate } from "@sirus/i18n";
import { activityElapsed, formatActivityDuration } from "@sirus/lib/agent-activity";
import { isSubagent, teamSize } from "@sirus/lib/subagent-team";
import { foldBoundary, markerSentence, timelineParts, type TimelinePart } from "@sirus/lib/turn-timeline";

import { SymbolView } from "./AppSymbol";
import { AppText } from "./AppText";
import { Markdown } from "./Markdown";
import { StepTree, ThinkingRow, type StepSource } from "./thread/StepTree";
import { SubagentTeam } from "./thread/SubagentTeam";
import { Tree } from "./thread/TreeRow";

const t = (key: string, params?: Record<string, string | number>) => translate("pt-BR", key, params);

/** The conversation's folder and how to open one of its files (file references in replies). */
export type ReplyFiles = { readonly cwd: string; readonly open: (path: string) => void };

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
}) {
  const { message } = props;
  const items = message.activity?.items;
  const parts = useMemo(() => timelineParts(message.content, items ?? [], message.steers), [message.content, items, message.steers]);
  const finished = !message.streaming;
  const fold = finished ? foldBoundary(parts) : 0;
  const [open, setOpen] = useState(false);
  const shown = fold > 0 && !open ? parts.slice(fold) : parts;
  const source = useMemo<StepSource>(() => ({ computerId: props.computerId, sessionId: message.sessionId, messageId: message.id }), [props.computerId, message.sessionId, message.id]);
  const team = useMemo(() => teamSize(items ?? []) >= 2, [items]);
  const launchId = useMemo(() => items?.find(isSubagent)?.id, [items]);

  // While the turn runs with no tool running and no text being written, the agent is thinking.
  const lastPart = parts.at(-1);
  const thinking = message.streaming && lastPart?.kind !== "text" && !(items ?? []).some((item) => item.state === "running" && !item.shell && !item.finishedTask);
  const trailing = thinking ? <ThinkingRow thought={message.activity?.thought} /> : undefined;

  if (!parts.length) return thinking ? <Tree>{trailing}</Tree> : null;
  const duration = message.activity ? formatActivityDuration(activityElapsed(message.activity, Date.now())) : null;
  const steps = (items ?? []).filter(countsAsStep).length;
  const subagents = teamSize(items ?? []);
  const summary = [steps ? `${steps} ${steps === 1 ? "passo" : "passos"}` : null, subagents ? `${subagents} ${subagents === 1 ? "subagente" : "subagentes"}` : null].filter(Boolean).join(" · ");
  const lastWork = parts.findLastIndex((part) => part.kind === "work");
  return <View className="gap-1">
    {fold > 0
      ? <Pressable onPress={() => setOpen((value) => !value)} accessibilityRole="button" className="mt-2 flex-row items-center gap-[7px] self-start py-1 active:opacity-60">
          <SymbolView name="checkmark" size={13} weight="semibold" tintColorClassName="accent-foreground-muted" />
          <AppText numberOfLines={1} className="shrink text-[13.5px] text-foreground-muted">{duration ? t("timeline.workedFor", { duration }) : t("timeline.worked")}</AppText>
          {summary ? <AppText numberOfLines={1} className="shrink text-[13.5px] text-foreground-tertiary">{`· ${summary}`}</AppText> : null}
          <SymbolView name={open ? "chevron.down" : "chevron.right"} size={11} weight="semibold" tintColorClassName="accent-foreground-tertiary" />
        </Pressable>
      : null}
    {shown.map((part) => {
      const at = parts.indexOf(part);
      return <Part key={partKey(part, at)} part={part} content={message.content} cwd={props.cwd} files={props.files} source={source} review={message.activity?.review}
        team={team} launchId={launchId} agent={props.agent} prompt={props.prompt ?? ""} allItems={items ?? []} trailing={at === lastWork && at === parts.length - 1 ? trailing : undefined} />;
    })}
    {thinking && lastPart?.kind !== "work" ? <Tree>{trailing}</Tree> : null}
  </View>;
});

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
