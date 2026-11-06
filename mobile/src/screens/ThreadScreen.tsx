import { useNavigation, useRoute, type NavigationProp, type ParamListBase, type RouteProp } from "@react-navigation/native";
import * as Haptics from "expo-haptics";
import { SymbolView } from "../components/AppSymbol";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as Clipboard from "expo-clipboard";
import { ActivityIndicator, Alert, FlatList, Pressable, ScrollView, View } from "react-native";
import { KeyboardStickyView, useKeyboardState } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { Message, PromptAttachment, TurnActivity } from "@sirus/client/types";

import { AppText, AppTextInput } from "../components/AppText";
import { ComposerSend } from "../components/ComposerSend";
import { ProviderIcon } from "../components/ProviderIcon";
import { ControlPillMenu } from "../components/ControlPillMenu";
import { GlassSurface } from "../components/GlassSurface";
import { AgentReply, type ReplyFiles } from "../components/AgentReply";
import { AgentRequest, PromptQueue } from "../components/AgentRequest";
import { UserText } from "../components/UserText";
import { OpenSubagent } from "../components/thread/subagent-context";
import { SubagentSheet, type SubagentTarget } from "../components/thread/SubagentSheet";
import { WorkingPill } from "../components/thread/WorkingPill";
import { useConversationModel } from "../data/conversation-model";
import { attachmentOwner, pickAttachments, releaseAttachments, uploadAttachments } from "../data/attachments";
import { useMacAccent } from "../data/mac-accent";
import { isScratch, projectLabel } from "../data/scratch";
import { ACTIVE_STATUS, transcriptKey, useSirus, type QueuedPrompt, type RowAction } from "../data/store";
import { NativeHeaderToolbar, NativeStackScreenOptions } from "../native/StackHeader";

type ThreadParams = { Thread: { computerId: string; sessionId: string } };

const NO_QUEUE: QueuedPrompt[] = [];

/**
 * A conversation: the native glass header (back, title and project, then the
 * terminal, files and Git controls with iOS menus), the transcript, and the glass composer that
 * rides the keyboard.
 */
export function ThreadScreen() {
  const { computerId, sessionId } = useRoute<RouteProp<ThreadParams, "Thread">>().params;
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const session = useSirus((state) => state.data[computerId]?.sessions.find((item) => item.id === sessionId));
  const project = useSirus((state) => state.data[computerId]?.projects.find((item) => item.id === session?.projectId));
  const messages = useSirus((state) => state.transcripts[transcriptKey(computerId, sessionId)]);
  const queued = useSirus((state) => state.queue[transcriptKey(computerId, sessionId)] ?? NO_QUEUE);
  // Older messages load a page at a time as the top comes near (the first page is the newest).
  const olderLeft = useSirus((state) => (state.transcriptStart[transcriptKey(computerId, sessionId)] ?? 0) > 0);
  const [loadingOlder, setLoadingOlder] = useState(false);
  const loadOlder = () => {
    if (!olderLeft || loadingOlder) return;
    setLoadingOlder(true);
    void useSirus.getState().loadOlder(computerId, sessionId).catch(() => false).finally(() => setLoadingOlder(false));
  };
  const list = useRef<FlatList<Message>>(null);
  const safeArea = useSafeAreaInsets();
  // The native scroll view's own end: FlatList's `scrollToEnd` uses cell sizes measured before
  // the native markdown grew.
  const toEnd = (animated = false) => (list.current?.getNativeScrollRef() as { scrollToEnd?: (options: { animated: boolean }) => void } | null)?.scrollToEnd?.({ animated });
  // Away from the end, a round arrow brings the newest message back (as the desktop's).
  const [away, setAway] = useState(false);
  // The list ends above the composer and, while it is open, above the keyboard too.
  const [composerHeight, setComposerHeight] = useState(96);
  const keyboardHeight = useKeyboardState((state) => state.isVisible ? state.height : 0);
  const lift = keyboardHeight ? Math.max(keyboardHeight - safeArea.bottom + 8, 0) : 0;
  const atEnd = useRef(true);
  const dragging = useRef(false);

  useEffect(() => {
    void useSirus.getState().loadTranscript(computerId, sessionId).catch(() => undefined);
  }, [computerId, sessionId]);
  // Opening the keyboard at the newest message keeps it in view above the composer.
  useEffect(() => {
    if (!lift || !atEnd.current) return;
    const timer = setTimeout(() => toEnd(true), 30);
    return () => clearTimeout(timer);
  }, [lift]);

  const shown = useMemo(() => (messages ?? []).filter((message) => message.role !== "system" && (message.content.trim() || message.streaming || message.activity?.items.length)), [messages]);
  // Native markdown measures after the first layout, so the opening lands on the end again
  // a few times while it settles, unless the person already scrolled.
  const loaded = messages !== undefined;
  useEffect(() => {
    if (!loaded) return;
    const timers = [60, 250, 600, 1200, 2000].map((delay) => setTimeout(() => { if (!dragging.current) toEnd(); }, delay));
    return () => timers.forEach(clearTimeout);
  }, [loaded, sessionId]);
  const working = session ? ACTIVE_STATUS.has(session.status) : false;
  const cwd = session?.worktree?.path ?? project?.path;
  // File references in replies open in the conversation's files.
  const openFile = useCallback((path: string) => navigation.navigate("File", { computerId, sessionId, path }), [navigation, computerId, sessionId]);
  const files = useMemo<ReplyFiles | undefined>(() => cwd ? { cwd, open: openFile } : undefined, [cwd, openFile]);
  const requests = session?.pendingRequests ?? [];
  const computerName = useSirus((state) => state.computers.find((item) => item.id === computerId)?.hostName);
  // "project · computer" under the title, as the approved layout.
  const subtitle = [projectLabel(project), computerName].filter(Boolean).join(" · ");
  const pinned = useSirus((state) => state.data[computerId]?.settings?.pinnedSessionIds?.includes(sessionId) ?? false);
  const run = (action: RowAction, title: string) => void useSirus.getState().rowAction(computerId, sessionId, action).catch((reason: unknown) => Alert.alert(title, reason instanceof Error ? reason.message : String(reason)));
  const rename = () => Alert.prompt("Renomear", undefined, [
    { text: "Cancelar", style: "cancel" },
    { text: "Salvar", onPress: (value?: string) => { if (value?.trim() && value.trim() !== session?.title) run({ type: "rename", title: value }, "Renomear"); } },
  ], "plain-text", session?.title);
  // Subagent sheet: opened from a team card or a lone subagent row.
  const [subagent, setSubagent] = useState<SubagentTarget | null>(null);
  const openSubagent = useCallback((messageId: string, itemId: string) => setSubagent({ messageId, itemId }), []);
  const closeSubagent = useCallback(() => setSubagent(null), []);
  // The newest reply's turn, for the working pill (its time and subagents).
  const latestActivity = useMemo<TurnActivity | undefined>(() => {
    for (let index = (messages?.length ?? 0) - 1; index >= 0; index -= 1) if (messages![index]!.role === "agent" && messages![index]!.activity) return messages![index]!.activity ?? undefined;
    return undefined;
  }, [messages]);

  return <OpenSubagent.Provider value={openSubagent}><View className="flex-1 bg-thread-canvas">
    <NativeStackScreenOptions options={{ title: session?.title || "Conversa", unstable_headerSubtitle: subtitle }} optionsVersion={[session?.title, subtitle, pinned]} />
    <NativeHeaderToolbar placement="right">
      {/* iOS lays the right placement out in reverse: JSX runs right to left, so the screen reads terminal, files, changes, menu. */}
      <NativeHeaderToolbar.Menu accessibilityLabel="Mais" icon="ellipsis">
        <NativeHeaderToolbar.MenuAction icon="arrow.triangle.branch" disabled onPress={() => {}} subtitle={project?.name ?? ""}>
          <NativeHeaderToolbar.Label>{session?.worktree?.branch ?? "—"}</NativeHeaderToolbar.Label>
        </NativeHeaderToolbar.MenuAction>
        <NativeHeaderToolbar.MenuAction icon="slider.horizontal.3" onPress={() => navigation.navigate("ThreadSettings", { computerId, sessionId })}>
          <NativeHeaderToolbar.Label>Ajustes da conversa</NativeHeaderToolbar.Label>
        </NativeHeaderToolbar.MenuAction>
        <NativeHeaderToolbar.MenuAction icon="doc.on.doc" onPress={() => void Clipboard.setStringAsync(sessionId)}>
          <NativeHeaderToolbar.Label>Copiar ID</NativeHeaderToolbar.Label>
        </NativeHeaderToolbar.MenuAction>
        <NativeHeaderToolbar.MenuAction icon={pinned ? "pin.slash" : "pin"} onPress={() => run({ type: pinned ? "unpin" : "pin" }, pinned ? "Desafixar" : "Fixar")}>
          <NativeHeaderToolbar.Label>{pinned ? "Desafixar" : "Fixar"}</NativeHeaderToolbar.Label>
        </NativeHeaderToolbar.MenuAction>
        <NativeHeaderToolbar.MenuAction icon="square.and.pencil" onPress={rename}>
          <NativeHeaderToolbar.Label>Renomear</NativeHeaderToolbar.Label>
        </NativeHeaderToolbar.MenuAction>
      </NativeHeaderToolbar.Menu>
      <NativeHeaderToolbar.Button accessibilityLabel="Alterações" icon="point.topleft.down.curvedto.point.bottomright.up" onPress={() => navigation.navigate("Git", { computerId, sessionId })} />
      <NativeHeaderToolbar.Button accessibilityLabel="Arquivos" icon="folder" onPress={() => navigation.navigate("Files", { computerId, sessionId, title: project?.name })} />
      <NativeHeaderToolbar.Button accessibilityLabel="Terminal" icon="terminal" onPress={() => navigation.navigate("Terminal", { computerId, sessionId })} />
    </NativeHeaderToolbar>
    {messages === undefined ? <ActivityIndicator className="mt-24" /> : <FlatList
      ref={list}
      className="flex-1"
      // Insets of its own: `scrollToEnd` leaves out the ones iOS adds by itself.
      contentInsetAdjustmentBehavior="never"
      // scrolling the conversation puts the keyboard away, and the composer folds back.
      keyboardDismissMode="on-drag"
      // A page added above keeps what is on screen in place.
      maintainVisibleContentPosition={{ minIndexForVisible: 0 }}
      ListHeaderComponent={loadingOlder ? <ActivityIndicator className="py-2" /> : undefined}
      data={shown}
      keyExtractor={(message) => message.id}
      renderItem={({ item, index }) => <MessageRow message={item} computerId={computerId} agent={session?.agent ?? "claude"} cwd={cwd} files={files} prompt={shown[index - 1]?.role === "user" ? shown[index - 1]!.content : ""} />}
      // The agent waiting on an approval or an answer, as the desktop's cards under the reply.
      ListFooterComponent={requests.length ? <View className="gap-3">
        {requests.map((request) => <AgentRequest key={request.requestId} computerId={computerId} sessionId={sessionId} agent={session?.agent ?? "claude"} request={request} />)}
      </View> : undefined}
      contentContainerStyle={{ paddingHorizontal: 18, paddingTop: safeArea.top + 44 + 8, paddingBottom: composerHeight + lift + 12, gap: 18 }}
      // Follows new content while the newest message is in view; a drag up stops it.
      onContentSizeChange={() => { if (atEnd.current) toEnd(); }}
      onScrollBeginDrag={() => { dragging.current = true; }}
      onScroll={({ nativeEvent }) => {
        if (dragging.current && nativeEvent.contentOffset.y < nativeEvent.layoutMeasurement.height * 1.5) loadOlder();
        const near = nativeEvent.contentOffset.y + nativeEvent.layoutMeasurement.height >= nativeEvent.contentSize.height - 160;
        if (dragging.current) atEnd.current = near;
        const far = nativeEvent.contentOffset.y + nativeEvent.layoutMeasurement.height < nativeEvent.contentSize.height - 400;
        if (far !== away) setAway(far);
      }}
      scrollEventThrottle={64}
    />}
    <Composer computerId={computerId} sessionId={sessionId} working={working} onHeight={setComposerHeight} agent={session?.agent ?? "claude"} queued={queued}
      away={away} onJump={() => { atEnd.current = true; dragging.current = false; setAway(false); toEnd(true); }}
      pill={working ? <WorkingPill activity={latestActivity} /> : null} />
    <SubagentSheet computerId={computerId} sessionId={sessionId} target={subagent} provider={session?.agent ?? "claude"} onClose={closeSubagent} />
  </View></OpenSubagent.Provider>;
}

const MessageRow = memo(function MessageRow(props: { readonly message: Message; readonly computerId: string; readonly agent: string; readonly cwd?: string; readonly files?: ReplyFiles; readonly prompt: string }) {
  if (props.message.role === "user") return <UserBubble text={props.message.content} computerId={props.computerId} />;
  return <AgentReply message={props.message} computerId={props.computerId} agent={props.agent} cwd={props.cwd} files={props.files} prompt={props.prompt} />;
});

/** A sent message in the Mac's accent at 24%: a pill on one line, a card past it (as Monocode's). */
function UserBubble(props: { readonly text: string; readonly computerId: string }) {
  const accent = useMacAccent(props.computerId);
  const [lines, setLines] = useState(1);
  return <View style={{ alignSelf: "flex-end", maxWidth: "88%", paddingHorizontal: 14, paddingVertical: 9, borderRadius: lines > 1 ? 14 : 21, borderCurve: "continuous", backgroundColor: `${accent}3d` }}>
    <UserText text={props.text} onLines={setLines} />
  </View>;
}

/** The glass composer: attachments, the field, dictation, and send or stop. */
function Composer(props: {
  readonly computerId: string; readonly sessionId: string; readonly working: boolean;
  readonly agent: string;
  /** Messages waiting for the reply to end; they show above the composer. */
  readonly queued: QueuedPrompt[];
  /** The composer's height, for the list to keep its last message above it. */
  readonly onHeight: (height: number) => void;
  /** Away from the newest message: the round arrow rides just above the composer. */
  readonly away: boolean; readonly onJump: () => void;
  /** The working pill: floats above the composer (and its queue) while the agent works. */
  readonly pill: React.ReactNode;
}) {
  const insets = useSafeAreaInsets();
  const owner = attachmentOwner(props.sessionId);
  const [text, setText] = useState("");
  const [attachments, setAttachments] = useState<PromptAttachment[]>([]);
  const returnSends = useSirus((state) => state.prefs.returnKey === "send");
  const [busy, setBusy] = useState<"sending" | "attaching" | null>(null);
  const canSend = (text.trim().length > 0 || attachments.length > 0) && !busy;
  const { execution } = useConversationModel(props.computerId, props.sessionId);
  const accent = useMacAccent(props.computerId);
  const submit = async () => {
    if (!canSend) return;
    setBusy("sending");
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    try {
      // While the agent works it joins the queue and goes once the reply ends.
      await useSirus.getState().submitPrompt(props.computerId, props.sessionId, text, attachments.map((item) => item.id), execution);
      setText("");
      setAttachments([]);
    } catch (reason) {
      Alert.alert("Não foi possível enviar", reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(null);
    }
  };
  const attach = async (kind: "photos" | "files") => {
    try {
      const files = await pickAttachments(kind);
      if (!files.length) return;
      setBusy("attaching");
      const added = await uploadAttachments(props.computerId, owner, files.slice(0, Math.max(0, 8 - attachments.length)));
      setAttachments((current) => [...current, ...added]);
    } catch (reason) {
      Alert.alert("Anexos", reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(null);
    }
  };
  const remove = (id: string) => {
    setAttachments((current) => current.filter((item) => item.id !== id));
    void releaseAttachments(props.computerId, owner, [id]);
  };
  const stop = () => void useSirus.getState().stopAgent(props.computerId, props.sessionId).catch(() => undefined);
  const plus = <ControlPillMenu actions={[{ id: "photos", title: "Fotos", image: "photo" }, { id: "files", title: "Escolher arquivos", image: "folder" }]}
    onPressAction={({ nativeEvent }) => void attach(nativeEvent.event as "photos" | "files")} accessibilityLabel="Anexar">
    <View className="h-[42px] w-[38px] items-center justify-center"><SymbolView name="plus" size={21} tintColorClassName="accent-icon" /></View>
  </ControlPillMenu>;
  const send = <ComposerSend text={text} onTextChange={setText} onSend={() => void submit()} sending={busy === "sending"} canSend={canSend} accent={accent}
    onStop={props.working && !attachments.length ? stop : undefined} disabled={busy === "attaching"} />;
  const field = <AppTextInput value={text} onChangeText={setText} placeholder="Pergunte ao agente…" multiline
    {...(returnSends ? { submitBehavior: "submit" as const, returnKeyType: "send" as const, onSubmitEditing: () => void submit() } : {})}
    className="max-h-40 min-h-[42px] rounded-none border-0 bg-transparent px-1 py-2.5 text-base" />;
  return <KeyboardStickyView offset={{ closed: 0, opened: insets.bottom - 8 }} style={{ position: "absolute", left: 0, right: 0, bottom: 0 }}>
    {props.away ? <Pressable onPress={props.onJump} accessibilityLabel="Ir para a última mensagem" style={{ position: "absolute", top: -52, right: 16 }}>
      <GlassSurface style={{ borderRadius: 20 }} className="size-10 items-center justify-center">
        <SymbolView name="arrow.down" size={16} weight="semibold" tintColorClassName="accent-foreground" />
      </GlassSurface>
    </Pressable> : null}
    {props.pill ? <View pointerEvents="none" style={{ position: "absolute", top: -46, left: 0, right: 0, alignItems: "center" }}>{props.pill}</View> : null}
    <View style={{ paddingHorizontal: 12, paddingBottom: Math.max(insets.bottom, 12) }} onLayout={(event) => props.onHeight(Math.round(event.nativeEvent.layout.height))}>
      {props.queued.length ? <PromptQueue computerId={props.computerId} sessionId={props.sessionId} agent={props.agent} items={props.queued} /> : null}
      <GlassSurface style={{ borderRadius: 29 }} className="min-h-[58px] justify-center p-[7px]">
        {attachments.length || busy === "attaching" ? <ScrollView horizontal showsHorizontalScrollIndicator={false} keyboardShouldPersistTaps="always" contentContainerClassName="gap-1.5 px-1.5 pb-1 pt-1">
          {attachments.map((item) => <View key={item.id} className="h-8 max-w-[180px] flex-row items-center gap-1.5 rounded-full bg-card pl-2.5 pr-1">
            <SymbolView name={item.mimeType?.startsWith("image/") ? "photo" : "doc"} size={12} tintColorClassName="accent-foreground-muted" />
            <AppText numberOfLines={1} className="shrink text-xs">{item.name}</AppText>
            <Pressable onPress={() => remove(item.id)} hitSlop={8} accessibilityLabel={`Remover ${item.name}`} className="size-6 items-center justify-center rounded-full active:bg-row-hover">
              <SymbolView name="xmark" size={9} weight="semibold" tintColorClassName="accent-foreground-muted" />
            </Pressable>
          </View>)}
          {busy === "attaching" ? <View className="h-8 justify-center px-2"><ActivityIndicator size="small" /></View> : null}
        </ScrollView> : null}
        {/* One row: + , the field, the model, the microphone and send or stop. The controls
            stay at the bottom while the field grows. */}
        <View className="flex-row items-end gap-0.5">
          {plus}
          <View className="min-w-0 flex-1">{field}</View>
          <View className="h-[42px] justify-center"><ModelButton computerId={props.computerId} sessionId={props.sessionId} /></View>
          {send}
        </View>
      </GlassSurface>
    </View>
  </KeyboardStickyView>;
}

/** The conversation's provider in the composer, icon and a small chevron only; it opens the conversation's settings sheet (model and provider). */
function ModelButton(props: { readonly computerId: string; readonly sessionId: string }) {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const { provider, name } = useConversationModel(props.computerId, props.sessionId);
  return <Pressable onPress={() => navigation.navigate("ThreadSettings", { computerId: props.computerId, sessionId: props.sessionId })} accessibilityLabel={`Modelo: ${name}`}
    className="h-[34px] flex-row items-center gap-[3px] rounded-[17px] bg-card px-2.5 active:bg-row-hover">
    <ProviderIcon provider={provider} size={18} />
    <SymbolView name="chevron.down" size={10} weight="semibold" tintColorClassName="accent-foreground-muted" />
  </Pressable>;
}
