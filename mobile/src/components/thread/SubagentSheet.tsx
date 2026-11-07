import { useEffect, useMemo, useRef, useState } from "react";
import { Alert, KeyboardAvoidingView, Modal, Platform, Pressable, ScrollView, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { ActivityItem, ActivityKind, SubagentFeed, SubagentFeedEntry } from "@sirus/client/types";
import { formatActivityDuration } from "@sirus/lib/agent-activity";
import { feedKey, isSubagent, memberElapsed, mergeFeedEntries, teamMember } from "@sirus/lib/subagent-team";

import { inkOn, useMacAccent } from "../../data/mac-accent";
import { transcriptKey, useSirus } from "../../data/store";
import { SymbolView } from "../AppSymbol";
import { AppText, AppTextInput } from "../AppText";
import { Markdown } from "../Markdown";
import { ProviderIcon } from "../ProviderIcon";
import { StepRow, type StepSource } from "./StepTree";
import { Tree, TreeRow } from "./TreeRow";
import { modelLabel, STATE_COLOR, t } from "./steps";
import { StatusDot, statusWord, stepsText } from "./SubagentTeam";
import { useNow } from "./use-now";

export interface SubagentTarget { readonly messageId: string; readonly itemId: string }

const KIND_ICON: Record<ActivityKind, string> = { read: "doc.text", edit: "pencil", command: "terminal", tool: "wrench.and.screwdriver", agent: "person.2", skill: "sparkles" };
const POLL_MS = 2500;
/** The id the host knows a subagent by (`agent:<task>` → `<task>`), as the Mac sends it. */
const taskOf = (id: string) => id.replace(/^agent:/, "");

/** A feed entry as the activity row the tree draws. */
function asItem(entry: SubagentFeedEntry): ActivityItem {
  return { id: entry.id, kind: entry.tool ?? "tool", label: entry.label ?? "", state: entry.state ?? "completed", model: null, ...(entry.detail ? { detail: entry.detail } : {}), ...(entry.output ? { output: entry.output } : {}) };
}

type Block = { readonly kind: "rows"; readonly entries: SubagentFeedEntry[] } | { readonly kind: "text"; readonly entry: SubagentFeedEntry };

/** Tool calls and thoughts share one tree; the subagent's own words break it. */
function blocks(entries: readonly SubagentFeedEntry[]): Block[] {
  const out: Block[] = [];
  for (const entry of entries) {
    if (entry.kind === "prompt") continue;
    if (entry.kind === "text") { if (entry.text?.trim()) out.push({ kind: "text", entry }); continue; }
    const last = out.at(-1);
    if (last?.kind === "rows") last.entries.push(entry);
    else out.push({ kind: "rows", entries: [entry] });
  }
  return out;
}

/**
 * A subagent's sheet, as the Mac's live view: its instruction, status, what it did as a tree,
 * Parar, and a message for it. The feed (`subagent_feed`) is read when the sheet opens and
 * again every couple of seconds while the subagent runs; when the host kept none (older
 * runs) the steps the turn recorded are shown instead.
 */
export function SubagentSheet(props: { readonly computerId: string; readonly sessionId: string; readonly target: SubagentTarget | null; readonly provider: string; readonly onClose: () => void }) {
  const { computerId, sessionId, target, onClose } = props;
  const insets = useSafeAreaInsets();
  const accent = useMacAccent(computerId);
  const item = useSirus((state) => {
    if (!target) return undefined;
    const reply = state.transcripts[transcriptKey(computerId, sessionId)]?.find((message) => message.id === target.messageId);
    return reply?.activity?.items.find((candidate) => candidate.id === target.itemId);
  });
  // Keep the last known row so the sheet does not blank while it slides away.
  const kept = useRef<ActivityItem | undefined>(undefined);
  if (item) kept.current = item;
  const shown = item ?? kept.current;
  const member = shown && isSubagent(shown) ? teamMember(shown) : null;
  const running = member?.status === "running";
  const now = useNow(Boolean(running) && target !== null);

  const [feed, setFeed] = useState<{ entries: SubagentFeedEntry[]; source: SubagentFeed["source"] } | null>(null);
  const key = shown ? feedKey(shown) : null;
  const open = target !== null;
  // A settled subagent is read once more and then left alone.
  const settled = useRef(false);
  settled.current = member !== null && !running;
  useEffect(() => {
    if (!open || !key) { setFeed(null); return; }
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout> | undefined;
    const load = async () => {
      try {
        const result = await useSirus.getState().invoke<SubagentFeed>(computerId, "subagent_feed", { sessionId, key });
        if (!cancelled) setFeed((current) => ({ entries: mergeFeedEntries(current?.entries ?? [], result.entries).entries, source: result.source }));
      } catch {
        if (!cancelled) setFeed((current) => current ?? { entries: [], source: "none" });
      }
      if (!cancelled && !settled.current) timer = setTimeout(() => void load(), POLL_MS);
    };
    void load();
    return () => { cancelled = true; if (timer) clearTimeout(timer); };
  }, [open, key, computerId, sessionId]);

  const [text, setText] = useState("");
  const [busy, setBusy] = useState<"send" | "stop" | null>(null);
  useEffect(() => { setText(""); }, [target?.itemId]);
  const fail = (title: string) => (reason: unknown) => Alert.alert(title, reason instanceof Error ? reason.message : String(reason));
  const send = () => {
    if (!shown || !text.trim() || busy) return;
    setBusy("send");
    void useSirus.getState().invoke<void>(computerId, "steer_subagent", { sessionId, taskId: taskOf(shown.id), text: text.trim() })
      .then(() => setText(""))
      .catch(fail("Não foi possível enviar"))
      .finally(() => setBusy(null));
  };
  const stop = () => {
    if (!shown || busy) return;
    setBusy("stop");
    void useSirus.getState().invoke<void>(computerId, "stop_background_task", { sessionId, taskId: taskOf(shown.id) })
      .catch(fail("Não foi possível parar"))
      .finally(() => setBusy(null));
  };

  const entries = feed?.entries;
  const prompt = entries?.find((entry) => entry.kind === "prompt")?.text?.trim() || shown?.detail?.trim() || "";
  const list = useMemo(() => blocks(entries ?? []), [entries]);
  const source: StepSource = useMemo(() => ({ computerId, sessionId, messageId: target?.messageId ?? "" }), [computerId, sessionId, target?.messageId]);
  const seconds = member ? memberElapsed(member, now) : null;
  const subtitle = [member ? modelLabel(member.model, props.provider) : null, seconds != null ? formatActivityDuration(seconds) : null].filter(Boolean).join(" · ");
  const hasFeed = list.length > 0;
  const fallback = !hasFeed && feed !== null ? shown?.steps ?? [] : [];
  const empty = feed !== null && !hasFeed && !fallback.length;

  return <Modal visible={open} animationType="slide" presentationStyle="pageSheet" onRequestClose={onClose}>
    <KeyboardAvoidingView behavior={Platform.OS === "ios" ? "padding" : undefined} className="flex-1 bg-thread-canvas">
      <View className="flex-row items-center gap-3 px-4 pb-2 pt-4">
        <Pressable onPress={onClose} accessibilityLabel="Fechar" hitSlop={8} className="size-9 items-center justify-center rounded-full bg-card active:opacity-70">
          <SymbolView name="xmark" size={14} weight="semibold" tintColorClassName="accent-foreground-muted" />
        </Pressable>
        <View className="min-w-0 flex-1">
          <AppText numberOfLines={1} className="font-app-semibold text-base">{member?.label || t("subagentView.title")}</AppText>
          {subtitle ? <View className="flex-row items-center gap-1.5">
            <ProviderIcon provider={props.provider} size={12} />
            <AppText numberOfLines={1} className="text-xs text-foreground-muted">{subtitle}</AppText>
          </View> : null}
        </View>
      </View>
      {member ? <View className="mx-4 mb-2 mt-1 flex-row items-center gap-[7px]">
        <StatusDot status={member.status} />
        <AppText className="text-[13px]" style={{ color: STATE_COLOR[member.status] }}>{statusWord(member.status)}</AppText>
        {member.tools > 0 ? <AppText className="text-[13px] text-foreground-muted">{`· ${stepsText(member.tools)}`}</AppText> : null}
        <View className="flex-1" />
        {running ? <Pressable onPress={stop} disabled={busy === "stop"} accessibilityRole="button" className="rounded-xl px-[11px] py-[5px] active:opacity-70" style={{ backgroundColor: "#ee8a9622", opacity: busy === "stop" ? 0.5 : 1 }}>
          <AppText className="text-[13px]" style={{ color: "#ee8a96" }}>Parar</AppText>
        </Pressable> : null}
      </View> : null}
      <ScrollView className="flex-1" contentContainerStyle={{ paddingHorizontal: 16, paddingBottom: 24, gap: 12 }} keyboardDismissMode="interactive" keyboardShouldPersistTaps="handled">
        {prompt ? <View className="rounded-[14px] bg-card px-3 py-2.5"><AppText className="text-[13.5px] leading-[19px] text-foreground-muted">{prompt}</AppText></View> : null}
        {feed === null ? <AppText className="py-3 text-sm text-foreground-tertiary">{t("subagentView.loading")}</AppText> : null}
        {list.map((block, index) => block.kind === "text"
          ? <Markdown key={block.entry.id} markdown={block.entry.text!.trim()} />
          : <Tree key={`b${index}`}>
              {block.entries.map((entry) => entry.kind === "thinking"
                ? <TreeRow key={entry.id} icon="brain" title="Pensou" body={entry.text?.trim() ? () => <AppText className="text-[13px] leading-[19px] text-foreground-tertiary">{entry.text}</AppText> : undefined} />
                : <StepRow key={entry.id} item={asItem(entry)} source={source} />)}
            </Tree>)}
        {fallback.length ? <View>
          <AppText className="text-xs text-foreground-tertiary">{t("subagentView.empty")}</AppText>
          <Tree>{fallback.map((step) => <TreeRow key={step.id} icon={KIND_ICON[step.kind]} live={step.state === "running"} failed={step.state === "failed"} title={step.label} />)}</Tree>
        </View> : null}
        {empty ? <AppText className="py-3 text-sm text-foreground-tertiary">{t(running ? "subagentView.waiting" : "subagentView.nothing")}</AppText> : null}
      </ScrollView>
      {running ? <View style={{ paddingHorizontal: 12, paddingBottom: Math.max(insets.bottom, 12), paddingTop: 6 }}>
        <View className="min-h-[52px] flex-row items-end gap-2 rounded-[26px] border border-border-subtle bg-card px-3 py-1.5">
          <AppTextInput value={text} onChangeText={setText} placeholder="Mensagem para este subagente" multiline
            className="max-h-32 min-h-[38px] min-w-0 flex-1 rounded-none border-0 bg-transparent px-1 py-2 text-base" />
          <Pressable onPress={send} disabled={!text.trim() || busy === "send"} accessibilityLabel="Enviar" className="mb-0.5 size-[38px] items-center justify-center rounded-full active:opacity-70"
            style={{ backgroundColor: accent, opacity: text.trim() ? 1 : 0.4 }}>
            <SymbolView name="arrow.up" size={16} weight="bold" tintColor={inkOn(accent)} />
          </Pressable>
        </View>
      </View> : <View style={{ height: Math.max(insets.bottom, 12) }} />}
    </KeyboardAvoidingView>
  </Modal>;
}
