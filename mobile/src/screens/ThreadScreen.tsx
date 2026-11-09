import { useNavigation, useRoute, type NavigationProp, type ParamListBase, type RouteProp } from "@react-navigation/native";
import * as Haptics from "expo-haptics";
import { SymbolView } from "../components/AppSymbol";
import { ICON } from "../lib/icon-sizes";
import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import * as Clipboard from "expo-clipboard";
import { ActivityIndicator, Alert, FlatList, Keyboard, Pressable, View } from "react-native";
import Animated, { FadeIn, FadeOut, useAnimatedStyle, useSharedValue, withTiming } from "react-native-reanimated";
import { KeyboardStickyView, useKeyboardState } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { Message, MessageAttachment, PromptAttachment, TurnActivity } from "@sirus/client/types";
import { afterSend, executionWithModes, promptExtras, setDebugging, setGoal, setPlanning, setTeam } from "@sirus/lib/composer-modes";

import { ListeningRobot } from "../components/ListeningRobot";
import { ComposerSend } from "../components/ComposerSend";
import { ProviderIcon } from "../components/ProviderIcon";
import { ControlPillMenu } from "../components/ControlPillMenu";
import { GlassSurface } from "../components/GlassSurface";
import { AgentReply, type ReplyFiles } from "../components/AgentReply";
import { AgentRequest, PromptQueue } from "../components/AgentRequest";
import { UserText } from "../components/UserText";
import { OpenSubagent } from "../components/thread/subagent-context";
import { SubagentSheet, type SubagentTarget } from "../components/thread/SubagentSheet";
import { WORKING_PILL_HEIGHT, WorkingPill } from "../components/thread/WorkingPill";
import { createWorkAnchors, scrollTargetFor, WorkAnchorsContext, type WorkAnchor } from "../components/thread/work-anchors";
import { AddSheet } from "../components/composer/AddSheet";
import { ComposerField } from "../components/composer/ComposerField";
import { ComposerStrip, type StripAttachment } from "../components/composer/ComposerStrip";
import { SuggestionsPanel } from "../components/composer/SuggestionsPanel";
import { useComposer } from "../components/composer/useComposer";
import { SentAttachments } from "../components/SentAttachments";
import { useConversationModel } from "../data/conversation-model";
import { attachmentOwner, pasteClipboardImage, pickAttachments, releaseAttachments, uploadAttachments } from "../data/attachments";
import { useMacAccent } from "../data/mac-accent";
import { isScratch, projectLabel } from "../data/scratch";
import { ACTIVE_STATUS, transcriptKey, useSirus, type QueuedPrompt, type RowAction } from "../data/store";
import { NativeHeaderToolbar, NativeStackScreenOptions } from "../native/StackHeader";

type ThreadParams = { Thread: { computerId: string; sessionId: string } };

const NO_QUEUE: QueuedPrompt[] = [];

/** Room the list keeps above the composer for the "Trabalhando" pill: its height plus a breath. */
const PILL_SPACE = WORKING_PILL_HEIGHT + 4;
/** The list's gap between rows. */
const ROW_GAP = 18;

/**
 * The end of the list grows by the pill's room while the pill shows, animated so the text above
 * does not jump when the pill comes or goes. Its negative margin cancels the list's row gap
 * while it is closed.
 */
function PillSpacer(props: { readonly on: boolean }) {
  const room = useSharedValue(props.on ? PILL_SPACE : 0);
  useEffect(() => { room.value = withTiming(props.on ? PILL_SPACE : 0, { duration: 220 }); }, [props.on, room]);
  const style = useAnimatedStyle(() => ({ height: room.value, marginTop: -ROW_GAP * (1 - room.value / PILL_SPACE) }));
  return <Animated.View pointerEvents="none" style={style} />;
}

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
  const anchors = useMemo(createWorkAnchors, []);
  const scroll = useRef({ offset: 0, viewport: 0, content: 0 });
  // A message just sent rises to the top of the list and stays there while the reply grows under
  // it, as on the Mac: `pinRoom` is the empty space at the end that lets it get that high.
  const sentAfter = useRef<string | null | undefined>(undefined);
  const [pinId, setPinId] = useState<string | null>(null);
  const [pinRoom, setPinRoom] = useState(0);
  const pinRef = useRef<React.ComponentRef<typeof View>>(null);
  const pinTarget = useRef<number | null>(null);
  // Until it gets there: the list may still be too short (the keyboard is closing), so it tries again.
  const pinSettled = useRef(true);
  const raise = useCallback(() => {
    if (pinSettled.current || pinTarget.current === null) return;
    list.current?.scrollToOffset({ offset: pinTarget.current, animated: true });
  }, []);
  useEffect(() => {
    const hidden = Keyboard.addListener("keyboardDidHide", raise);
    return () => hidden.remove();
  }, [raise]);
  const topPad = safeArea.top + 52;

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
  const lastUser = useMemo(() => shown.findLast((message) => message.role === "user")?.id ?? null, [shown]);
  useEffect(() => {
    if (sentAfter.current === undefined || lastUser === sentAfter.current || !lastUser) return;
    sentAfter.current = undefined;
    pinTarget.current = null;
    atEnd.current = false;
    dragging.current = false;
    setPinId(lastUser);
    // More than a screen, so nothing stops the rise; it shrinks to what is needed afterwards.
    setPinRoom(scroll.current.viewport * 2);
    pinSettled.current = false;
    // Once the room and the new row are laid out, the row's place is known.
    const timer = setTimeout(() => {
      const native = list.current?.getNativeScrollRef() as { measureInWindow?: (callback: (x: number, y: number) => void) => void } | null;
      native?.measureInWindow?.((_x, listTop) => pinRef.current?.measureInWindow((_nx: number, top: number) => {
        pinTarget.current = Math.max(0, top - listTop + scroll.current.offset - topPad);
        raise();
      }));
    }, 90);
    return () => clearTimeout(timer);
  }, [lastUser, topPad]);
  const working = session ? ACTIVE_STATUS.has(session.status) : false;
  const cwd = session?.worktree?.path ?? project?.path;
  // File references in replies open in the conversation's files.
  const openFile = useCallback((path: string) => navigation.navigate("File", { computerId, sessionId, path }), [navigation, computerId, sessionId]);
  // A picture named by path lives on the computer: its `reply_image` answers a data URI.
  const loadImage = useCallback((path: string) => useSirus.getState().invoke<string>(computerId, "reply_image", { sessionId, path }), [computerId, sessionId]);
  const files = useMemo<ReplyFiles | undefined>(() => cwd ? { cwd, open: openFile, image: loadImage } : undefined, [cwd, openFile, loadImage]);
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
  const pillOn = working && !!latestActivity;

  /**
   * The pill's tap: brings the current work into view. Subagents running: their card, opened.
   * Otherwise the lowest live step (the end of the running tree); with neither on screen, the end.
   */
  const jumpToWork = useCallback(async () => {
    const native = list.current?.getNativeScrollRef() as { measureInWindow?: (callback: (x: number, y: number, width: number, height: number) => void) => void } | null;
    const place = (anchor: WorkAnchor) => new Promise<{ y: number; height: number } | null>((resolve) => {
      const node = anchor.ref.current;
      if (!node || !native?.measureInWindow) return resolve(null);
      native.measureInWindow((_x, listTop) => node.measureInWindow((_nx, top, _w, height) => resolve({ y: top - listTop + scroll.current.offset, height })));
    });
    const lowest = async (pool: readonly WorkAnchor[]) => {
      let best: { anchor: WorkAnchor; y: number; height: number } | null = null;
      for (const anchor of pool) {
        const at = await place(anchor);
        if (at && (!best || at.y > best.y)) best = { anchor, ...at };
      }
      return best;
    };
    const all = anchors.all();
    const teams = all.filter((anchor) => anchor.kind === "team");
    let target = await lowest(teams.length ? teams : all);
    if (target && teams.length) {
      // Open the card, give the layout a moment, and measure it open.
      target.anchor.expand?.();
      await new Promise((resolve) => setTimeout(resolve, 120));
      target = (await lowest([target.anchor])) ?? target;
    }
    dragging.current = true;
    if (!target) { atEnd.current = true; toEnd(true); return; }
    const { viewport, content } = scroll.current;
    const offset = scrollTargetFor({ y: target.y, height: target.height, viewport, topPad: safeArea.top + 52, bottomReserve: composerHeight + lift + PILL_SPACE + 12, contentHeight: content });
    atEnd.current = offset >= content - viewport - 160;
    list.current?.scrollToOffset({ offset, animated: true });
  }, [anchors, safeArea.top, composerHeight, lift]);

  return <WorkAnchorsContext.Provider value={anchors}><OpenSubagent.Provider value={openSubagent}><View className="flex-1 bg-thread-canvas">
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
      extraData={pinId}
      keyExtractor={(message) => message.id}
      renderItem={({ item, index }) => <View ref={item.id === pinId ? pinRef : undefined} collapsable={false}><MessageRow message={item} computerId={computerId} agent={session?.agent ?? "claude"} model={session?.model} cwd={cwd} files={files} prompt={shown[index - 1]?.role === "user" ? shown[index - 1]!.content : ""} /></View>}
      // The agent waiting on an approval or an answer, as the desktop's cards under the reply.
      ListFooterComponent={<View style={{ marginTop: requests.length ? 0 : -ROW_GAP }}>
        {requests.length ? <View className="gap-3">
          {requests.map((request) => <AgentRequest key={request.requestId} computerId={computerId} sessionId={sessionId} agent={session?.agent ?? "claude"} request={request} />)}
        </View> : null}
        <PillSpacer on={pillOn} />
        <View pointerEvents="none" style={{ height: pinRoom }} />
      </View>}
      contentContainerStyle={{ paddingHorizontal: 18, paddingTop: safeArea.top + 44 + 8, paddingBottom: composerHeight + lift + 12, gap: ROW_GAP }}
      onLayout={({ nativeEvent }) => { scroll.current.viewport = nativeEvent.layout.height; }}
      onContentSizeChange={(_width, height) => {
        // The reply fills the room as it grows; the row stays where it rose to.
        if (pinTarget.current !== null) {
          const room = Math.max(0, pinTarget.current + scroll.current.viewport - (height - pinRoom));
          if (pinSettled.current && Math.abs(room - pinRoom) > 1) setPinRoom(room);
          raise();
        }
        scroll.current.content = height; if (atEnd.current) toEnd();
      }}
      // Follows new content while the newest message is in view; a drag up stops it.
      onScrollBeginDrag={() => { dragging.current = true; pinSettled.current = true; }}
      onScroll={({ nativeEvent }) => {
        if (!pinSettled.current && pinTarget.current !== null && Math.abs(nativeEvent.contentOffset.y - pinTarget.current) < 2) pinSettled.current = true;
        scroll.current = { offset: nativeEvent.contentOffset.y, viewport: nativeEvent.layoutMeasurement.height, content: nativeEvent.contentSize.height };
        if (dragging.current && nativeEvent.contentOffset.y < nativeEvent.layoutMeasurement.height * 1.5) loadOlder();
        const near = nativeEvent.contentOffset.y + nativeEvent.layoutMeasurement.height >= nativeEvent.contentSize.height - 160;
        if (dragging.current) atEnd.current = near;
        const far = nativeEvent.contentOffset.y + nativeEvent.layoutMeasurement.height < nativeEvent.contentSize.height - 400;
        if (far !== away) setAway(far);
      }}
      scrollEventThrottle={64}
    />}
    <Composer computerId={computerId} sessionId={sessionId} working={working} onSending={() => { sentAfter.current = lastUser; }} onSendFailed={() => { sentAfter.current = undefined; }} onHeight={setComposerHeight} agent={session?.agent ?? "claude"} queued={queued}
      away={away} onJump={() => { atEnd.current = true; dragging.current = false; setAway(false); toEnd(true); }}
      pill={pillOn ? <WorkingPill activity={latestActivity} onPress={() => void jumpToWork()} /> : null} />
    <SubagentSheet computerId={computerId} sessionId={sessionId} target={subagent} provider={session?.agent ?? "claude"} onClose={closeSubagent} />
  </View></OpenSubagent.Provider></WorkAnchorsContext.Provider>;
}

const MessageRow = memo(function MessageRow(props: { readonly message: Message; readonly computerId: string; readonly agent: string; readonly model?: string | null; readonly cwd?: string; readonly files?: ReplyFiles; readonly prompt: string }) {
  if (props.message.role === "user") return <UserBubble text={props.message.content} attachments={props.message.attachments} computerId={props.computerId} />;
  return <AgentReply message={props.message} computerId={props.computerId} agent={props.agent} model={props.model} cwd={props.cwd} files={props.files} prompt={props.prompt} />;
});

/** A sent message in the Mac's accent at 24%: a pill on one line, a card past it. */
function UserBubble(props: { readonly text: string; readonly attachments?: readonly MessageAttachment[]; readonly computerId: string }) {
  const accent = useMacAccent(props.computerId);
  const [lines, setLines] = useState(1);
  const bubble = props.text.trim() ? <View style={{ alignSelf: "flex-end", maxWidth: "88%", paddingHorizontal: 14, paddingVertical: 9, borderRadius: lines > 1 ? 14 : 21, borderCurve: "continuous", backgroundColor: `${accent}3d` }}>
    <UserText text={props.text} onLines={setLines} />
  </View> : null;
  if (!props.attachments?.length) return bubble;
  // The files go over the bubble, as on the Mac (`sent_chips`).
  return <View className="items-end gap-1.5">
    <SentAttachments attachments={props.attachments} computerId={props.computerId} />
    {bubble}
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
  /** A message is leaving: the list raises it once it shows. */
  readonly onSending: () => void;
  readonly onSendFailed: () => void;
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
  const [listening, setListening] = useState(false);
  const project = useSirus((state) => {
    const data = state.data[props.computerId];
    const projectId = data?.sessions.find((item) => item.id === props.sessionId)?.projectId;
    return data?.projects.find((item) => item.id === projectId);
  });
  const canSend = (text.trim().length > 0 || attachments.length > 0) && !busy;
  const { execution, provider, model, session } = useConversationModel(props.computerId, props.sessionId);
  const accent = useMacAccent(props.computerId);
  const skillOwner = useMemo(() => ({ projectId: null, sessionId: props.sessionId }), [props.sessionId]);
  const composer = useComposer({ computerId: props.computerId, owner: skillOwner, provider, model, text, setText, savedGoal: session?.goal });
  const submit = async () => {
    if (!canSend) return;
    setBusy("sending");
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    // It leaves the composer at once, and comes back if the send fails.
    const sent = { text, attachments };
    setText("");
    setAttachments([]);
    Keyboard.dismiss();
    props.onSending();
    try {
      // While the agent works it joins the queue and goes once the reply ends.
      await useSirus.getState().submitPrompt(props.computerId, props.sessionId, sent.text, sent.attachments.map((item) => item.id),
        executionWithModes(execution, composer.modes, provider, model), promptExtras(composer.modes, provider, model));
      composer.setModes(afterSend);
    } catch (reason) {
      setText(sent.text);
      setAttachments(sent.attachments);
      props.onSendFailed();
      Alert.alert("Não foi possível enviar", reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(null);
    }
  };
  const attach = async (kind: "photos" | "files" | "paste") => {
    try {
      const files = kind === "paste" ? await pasteClipboardImage() : await pickAttachments(kind);
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
  const strip = useMemo<StripAttachment[]>(() => attachments.map((item) => ({
    key: item.id, name: item.name, onRemove: () => remove(item.id),
    image: !!item.mimeType?.startsWith("image/") || !!item.previewUrl?.startsWith("data:image/"),
    uri: item.previewUrl?.startsWith("data:image/") ? item.previewUrl : undefined,
  })), [attachments]); // eslint-disable-line react-hooks/exhaustive-deps
  const plus = <Pressable onPress={composer.openSheet} accessibilityRole="button" accessibilityLabel="Adicionar" className="h-[42px] w-[38px] items-center justify-center active:opacity-60">
    <SymbolView name="plus" size={ICON.composerPlus} tintColorClassName="accent-icon" />
  </Pressable>;
  const send = <ComposerSend text={text} onTextChange={setText} onSend={() => void submit()} sending={busy === "sending"} canSend={canSend} accent={accent}
    onStop={props.working && !attachments.length ? stop : undefined} disabled={busy === "attaching"} onListening={setListening} />;
  const field = <ComposerField ref={composer.field} value={text} onChangeText={composer.onChangeText} onSelection={composer.setSelection} caret={composer.caret}
    onFocus={composer.onFocus} onBlur={composer.onBlur} placeholder="Pergunte ao agente…" submit={returnSends ? () => void submit() : undefined}
    className="max-h-40 min-h-[42px] rounded-none border-0 bg-transparent px-1 py-2.5 text-base" />;
  const listing = composer.suggestions.visible;
  const removeMode = (id: "planning" | "debugging" | "team" | "goal") => composer.changeModes((current) => id === "planning" ? setPlanning(current, false) : id === "debugging" ? setDebugging(current, false) : id === "team" ? setTeam(current, false) : setGoal(current, ""));
  return <KeyboardStickyView offset={{ closed: 0, opened: insets.bottom - 8 }} style={{ position: "absolute", left: 0, right: 0, bottom: 0 }}>
    {/* The jump arrow rides above the pill when both show, so they never overlap. */}
    {props.away && !listing ? <Pressable onPress={props.onJump} accessibilityLabel="Ir para a última mensagem" style={{ position: "absolute", top: props.pill ? -(46 + 46) : -52, right: 16 }}>
      <GlassSurface style={{ borderRadius: 20 }} className="size-10 items-center justify-center">
        <SymbolView name="arrow.down" size={18} weight="semibold" tintColorClassName="accent-foreground" />
      </GlassSurface>
    </Pressable> : null}
    {props.pill && !listing && !listening ? <Animated.View entering={FadeIn.duration(200)} exiting={FadeOut.duration(160)} pointerEvents="box-none" style={{ position: "absolute", top: -46, left: 0, right: 0, alignItems: "center" }}>{props.pill}</Animated.View> : null}
    <View style={{ paddingHorizontal: 12 }}><SuggestionsPanel composer={composer} /></View>
    <View style={{ paddingHorizontal: 12, paddingBottom: Math.max(insets.bottom, 12) }} onLayout={(event) => props.onHeight(Math.round(event.nativeEvent.layout.height))}>
      {listening ? <ListeningRobot name={project?.name ?? "Sirus"} appearance={project?.appearance} /> : null}
      {props.queued.length ? <PromptQueue computerId={props.computerId} sessionId={props.sessionId} agent={props.agent} items={props.queued} /> : null}
      <GlassSurface style={{ borderRadius: 29 }} className="min-h-[58px] justify-center p-[7px]">
        <ComposerStrip attachments={strip} modes={composer.modes} onRemoveMode={removeMode}>
          {busy === "attaching" ? <View className="h-8 justify-center px-2"><ActivityIndicator size="small" /></View> : null}
        </ComposerStrip>
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
    <AddSheet composer={composer} provider={provider} model={model} busy={busy !== null} onPhotos={() => void attach("photos")} onFiles={() => void attach("files")} onPaste={() => void attach("paste")} />
  </KeyboardStickyView>;
}

/** The conversation's provider in the composer, icon and a small chevron only; it opens the conversation's settings sheet (model and provider). */
function ModelButton(props: { readonly computerId: string; readonly sessionId: string }) {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const { provider, name } = useConversationModel(props.computerId, props.sessionId);
  return <Pressable onPress={() => navigation.navigate("ThreadSettings", { computerId: props.computerId, sessionId: props.sessionId })} accessibilityLabel={`Modelo: ${name}`}
    className="h-[34px] flex-row items-center gap-[3px] rounded-[17px] bg-card px-2.5 active:bg-row-hover">
    <ProviderIcon provider={provider} size={ICON.capsuleProvider} />
    <SymbolView name="chevron.down" size={ICON.capsuleChevron} weight="semibold" tintColorClassName="accent-foreground-muted" />
  </Pressable>;
}
