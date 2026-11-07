import { memo, useContext, useEffect, useMemo, useState } from "react";
import { ActivityIndicator, Pressable, ScrollView, View } from "react-native";

import type { ActivityItem, TurnReview } from "@sirus/client/types";

import { isSubagent } from "@sirus/lib/subagent-team";

import { useSirus } from "../../data/store";
import { SymbolView } from "../AppSymbol";
import { AppText } from "../AppText";
import { ShimmerText } from "./Shimmer";
import { OpenSubagent } from "./subagent-context";
import { PathChip, TargetChip, TreeRow } from "./TreeRow";
import { commandLabel, commandPaths, fileName, groupTitle, stepCaption, stepLine, t } from "./steps";

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

/** "Executou" while it runs reads "Executando"; verbs with no such form stay. */
const liveVerb = (verb: string) => verb === "Leu" ? "Lendo" : verb.replace(/ou$/, "ando");

const Delta = (props: { readonly file: ReviewFile }) => <AppText className="text-xs">
  <AppText className="text-xs" style={{ color: GREEN }}>{`+${props.file.additions}`}</AppText>{" "}<AppText className="text-xs" style={{ color: RED }}>{`-${props.file.deletions}`}</AppText>
</AppText>;

/** One step as a row: a command reads as what it was for, with its program and files as chips. */
export const StepRow = memo(function StepRow(props: { readonly item: ActivityItem; readonly cwd?: string; readonly file?: ReviewFile; readonly source: StepSource }) {
  const { item, file, source } = props;
  const line = stepLine(item, props.cwd);
  const live = item.state === "running";
  const failed = item.state === "failed";
  const detail = item.detail?.trim() ?? "";
  const openSubagent = useContext(OpenSubagent);
  // A lone subagent (no team card) is a row that opens its sheet.
  if (isSubagent(item)) return <TreeRow icon="person.2" live={live} failed={failed} title={item.detail?.trim() || "Subagente"} onPress={() => openSubagent(source.messageId, item.id)} />;
  if (line.category === "command") {
    const paths = commandPaths(detail);
    const program = commandLabel(detail);
    return <TreeRow live={live} failed={failed} title={stepCaption(item) || undefined} body={detail || item.output ? () => <CommandBody item={item} source={source} /> : undefined}
      chips={<>
        {program ? <TargetChip>{program}</TargetChip> : null}
        {paths.map((path) => <PathChip key={path.name} name={path.name} folder={path.folder} />)}
      </>} />;
  }
  const body = line.category === "edit" && detail ? () => <EditBody file={file} path={detail} source={source} />
    : item.output?.trim() ? () => <CodeBlock lines={item.output!.replace(/\n+$/, "").split("\n")} />
    : undefined;
  return <TreeRow icon={line.icon} live={live} failed={failed} title={live ? liveVerb(line.verb) : line.verb} body={body} chips={<>
    {line.target ? <TargetChip>{line.target}</TargetChip> : null}
    {file && line.category === "edit" ? <Delta file={file} /> : null}
  </>} />;
});

/** Back-to-back file reads fold into one row ("Leu 8 arquivos"); a tap lists them. */
const ReadsRow = memo(function ReadsRow(props: { readonly items: ActivityItem[] }) {
  const names = useMemo(() => [...new Set(props.items.map((item) => fileName(item.detail ?? "")))], [props.items]);
  return <TreeRow icon="doc.text" title="Leu" chips={<TargetChip>{`${names.length} arquivos`}</TargetChip>} body={() => <View className="gap-0.5">
    {names.slice(0, 40).map((name) => <AppText key={name} numberOfLines={1} style={MONO} className="text-xs leading-[19px] text-foreground-muted">{name}</AppText>)}
    {names.length > 40 ? <AppText className="text-xs text-foreground-tertiary">{`+ ${names.length - 40} arquivos`}</AppText> : null}
  </View>} />;
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

/**
 * A run of steps as on the Mac: a tile with a chevron and what the run did, and its rows on a rail.
 * It stays open while it works and folds when it ends, unless the person opened or closed it.
 */
export const StepTree = memo(function StepTree(props: {
  readonly items: readonly ActivityItem[];
  readonly cwd?: string;
  readonly review?: TurnReview | null;
  readonly source: StepSource;
  /** Rendered after the steps, on the rail: the "Pensando" line. */
  readonly trailing?: React.ReactNode;
}) {
  const list = useMemo(() => entries(props.items), [props.items]);
  const live = props.items.some((item) => item.state === "running") || !!props.trailing;
  const [choice, setChoice] = useState<boolean | null>(null);
  const open = choice ?? live;
  const title = groupTitle(props.items, live);
  return <View className="mt-1">
    <Pressable onPress={() => setChoice(!open)} accessibilityRole="button" accessibilityLabel={title} className="min-h-[34px] flex-row items-center gap-[9px] active:opacity-60">
      <View className={`size-[25px] items-center justify-center rounded-[8px] border bg-card ${live ? "border-[#7fb4ff66]" : "border-border-subtle"}`}>
        <SymbolView name={open ? "chevron.down" : "chevron.right"} size={12} weight="semibold" tintColorClassName="accent-foreground-muted" />
      </View>
      {live
        ? <ShimmerText numberOfLines={1} className="min-w-0 flex-1 text-[13.5px] text-foreground-muted">{title}</ShimmerText>
        : <AppText numberOfLines={1} className="min-w-0 flex-1 text-[13.5px] text-foreground-muted">{title}</AppText>}
    </Pressable>
    {open ? <View>
      {list.map((entry) => entry.kind === "reads"
        ? <ReadsRow key={`r${entry.items[0]!.id}`} items={entry.items} />
        : <StepRow key={entry.item.id} item={entry.item} cwd={props.cwd} file={entry.item.kind === "edit" ? reviewFor(props.review?.files, entry.item.detail) : undefined} source={props.source} />)}
      {props.trailing}
    </View> : null}
  </View>;
});

/** The tail of a running turn when no tool is running: the agent is thinking. */
export function ThinkingRow(props: { readonly thought?: string | null }) {
  return <TreeRow icon="brain" live title="Pensando" body={props.thought ? () => <AppText className="text-[13px] leading-[19px] text-foreground-muted">{props.thought}</AppText> : undefined} />;
}
