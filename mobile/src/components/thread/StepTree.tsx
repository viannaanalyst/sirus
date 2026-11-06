import { memo, useContext, useEffect, useMemo, useState } from "react";
import { ActivityIndicator, ScrollView, View } from "react-native";

import type { ActivityItem, TurnReview } from "@sirus/client/types";

import { isSubagent } from "@sirus/lib/subagent-team";

import { useSirus } from "../../data/store";
import { AppText } from "../AppText";
import { OpenSubagent } from "./subagent-context";
import { TreeRow, Tree } from "./TreeRow";
import { fileName, shortElapsed, stepLine, stepSeconds, t } from "./steps";
import { useNow } from "./use-now";

/** Where a reply lives, so a row can fetch what transcripts leave out (full output, diffs). */
export interface StepSource { readonly computerId: string; readonly sessionId: string; readonly messageId: string }
type ReviewFile = TurnReview["files"][number];

const GREEN = "#5fce8c";
const RED = "#ee8a96";
const MONO = { fontFamily: "Menlo" } as const;

/** A diff or output block: monospace, scrolls sideways instead of wrapping. */
export function CodeBlock(props: { readonly lines: readonly string[]; readonly tint?: (line: string) => string | undefined; readonly max?: number }) {
  const max = props.max ?? 160;
  const shown = props.lines.slice(0, max);
  return <View className="overflow-hidden rounded-xl bg-card py-2">
    <ScrollView horizontal showsHorizontalScrollIndicator={false} nestedScrollEnabled>
      <View>
        {shown.map((line, index) => {
          const color = props.tint?.(line);
          return <View key={index} className="px-2.5" style={color ? { backgroundColor: `${color}1c` } : undefined}>
            <AppText style={[MONO, color ? { color } : null]} className="text-xs leading-[18px] text-foreground-muted">{line || " "}</AppText>
          </View>;
        })}
      </View>
    </ScrollView>
    {props.lines.length > max ? <AppText className="px-2.5 pt-1 text-xs text-foreground-tertiary">{`… mais ${props.lines.length - max} linhas`}</AppText> : null}
  </View>;
}

const diffTint = (line: string) => line.startsWith("+") && !line.startsWith("+++") ? GREEN : line.startsWith("-") && !line.startsWith("---") ? RED : undefined;

/** The file a step edited, matched in the turn's review by path. */
function reviewFor(files: readonly ReviewFile[] | undefined, detail: string | undefined): ReviewFile | undefined {
  const path = detail?.trim();
  if (!path || !files) return undefined;
  return files.find((file) => file.path === path || path.endsWith(`/${file.path}`) || file.path.endsWith(`/${fileName(path)}`) && fileName(file.path) === fileName(path));
}

/** An edit's diff; a turn review that left the text out is fetched when the row opens. */
function EditBody(props: { readonly file: ReviewFile | undefined; readonly path: string; readonly source: StepSource }) {
  const { file, source } = props;
  const [diff, setDiff] = useState<string | null>(file?.diff ?? null);
  const [loading, setLoading] = useState(Boolean(file && file.diff == null && file.diffOmitted));
  useEffect(() => {
    if (!file || file.diff != null || !file.diffOmitted) return;
    let cancelled = false;
    void useSirus.getState().invoke<{ type: "turnReview"; review: TurnReview | null }>(source.computerId, "transcript_action", { action: { type: "turnReview", sessionId: source.sessionId, messageId: source.messageId } })
      .then((result) => { if (!cancelled) setDiff(result.review?.files.find((item) => item.path === file.path)?.diff ?? null); })
      .catch(() => undefined)
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [file, source.computerId, source.sessionId, source.messageId]);
  if (loading) return <ActivityIndicator size="small" className="self-start py-2" />;
  if (!diff) return <AppText numberOfLines={2} className="text-xs text-foreground-tertiary">{props.path}</AppText>;
  return <View className="gap-1">
    <AppText numberOfLines={1} className="text-xs text-foreground-tertiary">{props.path}</AppText>
    <CodeBlock lines={diff.split("\n")} tint={diffTint} />
  </View>;
}

/** A command's output; when it is stored apart the full text is fetched on open (`toolOutput`). */
function CommandBody(props: { readonly item: ActivityItem; readonly source: StepSource }) {
  const { item, source } = props;
  const [text, setText] = useState(item.output ?? "");
  const [loading, setLoading] = useState(false);
  useEffect(() => {
    if (!item.outputBytes || item.outputBytes <= (item.output?.length ?? 0)) return;
    let cancelled = false;
    setLoading(true);
    void useSirus.getState().invoke<{ type: "toolOutput"; text: string }>(source.computerId, "transcript_action", { action: { type: "toolOutput", sessionId: source.sessionId, messageId: source.messageId, stepId: item.id } })
      .then((result) => { if (!cancelled && typeof result.text === "string") setText(result.text); })
      .catch(() => undefined)
      .finally(() => { if (!cancelled) setLoading(false); });
    return () => { cancelled = true; };
  }, [item.id, item.output, item.outputBytes, source.computerId, source.sessionId, source.messageId]);
  const lines = useMemo(() => [`$ ${item.detail ?? ""}`.trimEnd(), ...(text ? text.replace(/\n+$/, "").split("\n") : [])], [item.detail, text]);
  return <View className="gap-1">
    <CodeBlock lines={lines} />
    {loading ? <ActivityIndicator size="small" className="self-start" /> : null}
  </View>;
}

/** Elapsed seconds of a step; ticks only while it runs. */
function useElapsed(item: ActivityItem): string | null {
  const running = item.state === "running";
  const now = useNow(running);
  return shortElapsed(stepSeconds(item, now));
}

/** "Executou" while it runs reads "Executando"; verbs with no such form stay. */
const liveVerb = (verb: string) => verb === "Leu" ? "Lendo" : verb.replace(/ou$/, "ando");

const Title = (props: { readonly verb: string; readonly target: string | null; readonly extra?: React.ReactNode }) => <>
  {props.verb ? `${props.verb} ` : ""}{props.target ? <AppText className="text-[13.5px] font-app-medium text-foreground">{props.target}</AppText> : null}{props.extra}
</>;

const Delta = (props: { readonly file: ReviewFile }) => <>
  {"  "}<AppText className="text-xs" style={{ color: GREEN }}>{`+${props.file.additions}`}</AppText>{" "}<AppText className="text-xs" style={{ color: RED }}>{`−${props.file.deletions}`}</AppText>
</>;

/** One step as a row: the Mac's verb and target, time, +/− for edits, details on tap. */
export const StepRow = memo(function StepRow(props: { readonly item: ActivityItem; readonly cwd?: string; readonly file?: ReviewFile; readonly source: StepSource }) {
  const { item, file, source } = props;
  const line = stepLine(item, props.cwd);
  const live = item.state === "running";
  const time = useElapsed(item);
  const detail = item.detail?.trim() ?? "";
  const openSubagent = useContext(OpenSubagent);
  // A lone subagent (no team card) is a row that opens its sheet.
  if (isSubagent(item)) return <TreeRow icon="person.2" live={live} failed={item.state === "failed"} time={time} onPress={() => openSubagent(source.messageId, item.id)}>{item.label.trim() || t("subagentView.title")}</TreeRow>;
  const body = line.category === "edit" && detail ? () => <EditBody file={file} path={detail} source={source} />
    : line.category === "command" && (detail || item.output) ? () => <CommandBody item={item} source={source} />
    : item.output?.trim() ? () => <CodeBlock lines={item.output!.replace(/\n+$/, "").split("\n")} />
    : undefined;
  return <TreeRow icon={line.icon} live={live} failed={item.state === "failed"} time={time} body={body}>
    <Title verb={live ? liveVerb(line.verb) : line.verb} target={line.target} extra={file && line.category === "edit" ? <Delta file={file} /> : null} />
  </TreeRow>;
});

/** Back-to-back file reads fold into one row ("Leu 8 arquivos"); a tap lists them. */
const ReadsRow = memo(function ReadsRow(props: { readonly items: ActivityItem[] }) {
  const names = useMemo(() => [...new Set(props.items.map((item) => fileName(item.detail ?? "")))], [props.items]);
  const seconds = props.items.reduce((sum, item) => sum + (stepSeconds(item, 0) ?? 0), 0);
  return <TreeRow icon="doc.text" time={shortElapsed(seconds)} body={() => <View className="gap-0.5">
    {names.slice(0, 40).map((name) => <AppText key={name} numberOfLines={1} style={MONO} className="text-xs leading-[19px] text-foreground-muted">{name}</AppText>)}
    {names.length > 40 ? <AppText className="text-xs text-foreground-tertiary">{`+ ${names.length - 40} arquivos`}</AppText> : null}
  </View>}>
    <Title verb="Leu" target={`${names.length} arquivos`} />
  </TreeRow>;
});

type Entry = { readonly kind: "step"; readonly item: ActivityItem } | { readonly kind: "reads"; readonly items: ActivityItem[] };

/** Settled reads that come one after another become one entry; everything else stays a row. */
function entries(items: readonly ActivityItem[]): Entry[] {
  const out: Entry[] = [];
  let reads: ActivityItem[] = [];
  const flush = () => {
    if (reads.length > 1) out.push({ kind: "reads", items: reads });
    else if (reads[0]) out.push({ kind: "step", item: reads[0] });
    reads = [];
  };
  for (const item of items) {
    if (stepLine(item).category === "read" && item.state !== "running" && item.state !== "failed" && item.detail) reads.push(item);
    else { flush(); out.push({ kind: "step", item }); }
  }
  flush();
  return out;
}

/** The activity tree of a run of steps. */
export const StepTree = memo(function StepTree(props: {
  readonly items: readonly ActivityItem[];
  readonly cwd?: string;
  readonly review?: TurnReview | null;
  readonly source: StepSource;
  /** Rendered after the steps, still on the guide: the "Pensando" line. */
  readonly trailing?: React.ReactNode;
}) {
  const list = useMemo(() => entries(props.items), [props.items]);
  return <Tree>
    {list.map((entry) => entry.kind === "reads"
      ? <ReadsRow key={`r${entry.items[0]!.id}`} items={entry.items} />
      : <StepRow key={entry.item.id} item={entry.item} cwd={props.cwd} file={entry.item.kind === "edit" ? reviewFor(props.review?.files, entry.item.detail) : undefined} source={props.source} />)}
    {props.trailing}
  </Tree>;
});

/** The tail of a running turn when no tool is running: the agent is thinking. */
export function ThinkingRow(props: { readonly thought?: string | null }) {
  return <TreeRow icon="brain" live body={props.thought ? () => <AppText className="text-[13px] leading-[19px] text-foreground-tertiary">{props.thought}</AppText> : undefined}>
    Pensando
  </TreeRow>;
}
