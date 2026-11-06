import { useNavigation, useRoute, type NavigationProp, type ParamListBase, type RouteProp } from "@react-navigation/native";
import * as Haptics from "expo-haptics";
import { useCallback, useEffect, useMemo, useState } from "react";
import { ActivityIndicator, Alert, Pressable, ScrollView, View } from "react-native";

import type { GitCommitResult, GitPushResult, GitWorkspaceEntry, GitWorkspaceResponse, GitWorkspaceSnapshot } from "@sirus/client/types";

import { SymbolView } from "../components/AppSymbol";
import { AppText } from "../components/AppText";
import { SheetGroup, SheetScreen, SheetSection } from "../components/SheetKit";
import { inkOn, useMacAccent } from "../data/mac-accent";
import { useSirus } from "../data/store";
import { useAppearancePreferences } from "../features/settings/appearance/AppearancePreferencesProvider";
import { NativeStackScreenOptions } from "../native/StackHeader";

type GitParams = {
  Git: { computerId: string; sessionId: string };
  Diff: { computerId: string; sessionId: string; path: string; staged: boolean; indexToken: string };
};

const KIND: Record<GitWorkspaceEntry["kind"], { letter: string; className: string }> = {
  added: { letter: "A", className: "text-adaptive-emerald-600-400" },
  untracked: { letter: "U", className: "text-adaptive-emerald-600-400" },
  modified: { letter: "M", className: "text-warning" },
  renamed: { letter: "R", className: "text-warning" },
  deleted: { letter: "D", className: "text-danger-foreground" },
};

const errorText = (reason: unknown) => reason instanceof Error ? reason.message : String(reason);

/**
 * The conversation's Git on its computer, as the desktop's Changes pane: staged and unstaged
 * files (tap the circle to move one, the row to open its diff inline), then commit, commit and push, pull. A commit
 * with nothing staged takes every change.
 */
export function GitScreen() {
  const { computerId, sessionId } = useRoute<RouteProp<GitParams, "Git">>().params;
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const [snapshot, setSnapshot] = useState<GitWorkspaceSnapshot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const accent = useMacAccent(computerId);
  const invoke = useCallback(<T,>(command: string, args: Record<string, unknown>) => useSirus.getState().invoke<T>(computerId, command, args), [computerId]);
  const action = useCallback((value: Record<string, unknown>) => invoke<GitWorkspaceResponse>("git_workspace_action", { action: { sessionId, ...value } }), [invoke, sessionId]);

  const refresh = useCallback(async () => {
    try {
      const result = await action({ type: "snapshot" });
      if (result.type === "snapshot") { setSnapshot(result.snapshot); setError(null); }
    } catch (reason) { setError(errorText(reason)); }
  }, [action]);
  useEffect(() => { void refresh(); }, [refresh]);

  const run = async (label: string, task: () => Promise<void>) => {
    setBusy(label);
    try { await task(); void Haptics.notificationAsync(Haptics.NotificationFeedbackType.Success); }
    catch (reason) { Alert.alert(label, errorText(reason)); }
    finally { setBusy(null); await refresh(); }
  };

  const toggle = (entry: GitWorkspaceEntry, staged: boolean) => {
    if (!snapshot || !entry.actionable) return;
    void Haptics.selectionAsync();
    void run(staged ? "Tirar do commit" : "Incluir no commit", async () => {
      await action({ type: staged ? "unstage" : "stage", paths: [entry.path], expectedIndex: snapshot.indexToken });
    });
  };

  const commit = (thenPush: boolean) => {
    if (!snapshot) return;
    Alert.prompt(thenPush ? "Commit e Push" : "Commit", snapshot.staged.length ? `${snapshot.staged.length} arquivo(s) preparados.` : "Nada preparado: o commit leva todas as alterações.", [
      { text: "Cancelar", style: "cancel" },
      { text: "Commit", onPress: (message?: string) => {
        if (!message?.trim()) return;
        void run(thenPush ? "Commit e Push" : "Commit", async () => {
          let expected = snapshot.indexToken;
          if (!snapshot.staged.length) {
            const paths = snapshot.unstaged.filter((entry) => entry.actionable).map((entry) => entry.path);
            await action({ type: "stage", paths, expectedIndex: expected });
            const fresh = await action({ type: "snapshot" });
            if (fresh.type === "snapshot") expected = fresh.snapshot.indexToken;
          }
          const result = await invoke<GitCommitResult>("git_commit", { sessionId, message: message.trim(), expectedIndex: expected });
          if (thenPush) {
            const pushed = await invoke<GitPushResult>("git_push", { sessionId });
            Alert.alert("Commit e push feitos", `${result.hash.slice(0, 7)} ${result.summary}\nEnviado para ${pushed.branch}.`);
          } else {
            Alert.alert("Commit feito", `${result.hash.slice(0, 7)} ${result.summary}`, [
              { text: "OK" },
              { text: "Push", onPress: () => push() },
            ]);
          }
        });
      } },
    ], "plain-text");
  };
  const push = () => void run("Push", async () => {
    const result = await invoke<GitPushResult>("git_push", { sessionId });
    Alert.alert("Push feito", `Enviado para ${result.branch}.`);
  });
  const pull = () => void run("Pull", async () => {
    const result = await action({ type: "pull", confirm: true });
    if (result.type === "pulled") Alert.alert("Pull feito", result.summary || result.branch);
  });

  // The file open inline (its diff loads on tap and is dropped when the index changes).
  const [open, setOpen] = useState<string | null>(null);
  const [diffs, setDiffs] = useState<Record<string, string | { error: string }>>({});
  // Diffs load for the first files as soon as the snapshot arrives: they give the +/- counts and open at once.
  useEffect(() => {
    if (!snapshot) return;
    let live = true;
    setDiffs({});
    const files = [...snapshot.staged.map((entry) => [entry, true] as const), ...snapshot.unstaged.map((entry) => [entry, false] as const)].slice(0, 40);
    for (const [entry, staged] of files) {
      action({ type: "diff", path: entry.path, staged, expectedIndex: snapshot.indexToken }).then(
        (result) => { if (live) setDiffs((current) => ({ ...current, [`${staged ? "s" : "u"}:${entry.path}`]: result.type === "diff" ? result.diff : "" })); },
        (reason: unknown) => { if (live) setDiffs((current) => ({ ...current, [`${staged ? "s" : "u"}:${entry.path}`]: { error: errorText(reason) } })); },
      );
    }
    return () => { live = false; };
  }, [snapshot, action]);
  const openFile = (entry: GitWorkspaceEntry, staged: boolean) => {
    void Haptics.selectionAsync();
    const key = `${staged ? "s" : "u"}:${entry.path}`;
    setOpen(open === key ? null : key);
  };

  const sections = useMemo(() => snapshot ? [
    ...(snapshot.staged.length ? [{ title: "Preparados para o commit", staged: true, data: snapshot.staged }] : []),
    ...(snapshot.unstaged.length ? [{ title: "Alterações", staged: false, data: snapshot.unstaged }] : []),
  ] : [], [snapshot]);
  const branch = snapshot?.identity.branch ?? (snapshot?.identity.detached ? "HEAD solto" : "");
  const sync = [snapshot?.ahead ? `${snapshot.ahead}↑` : "", snapshot?.behind ? `${snapshot.behind}↓` : ""].filter(Boolean).join(" ");
  const nothing = !snapshot || (!snapshot.staged.length && !snapshot.unstaged.length);

  return <SheetScreen title="Mudanças" subtitle={[branch, sync].filter(Boolean).join(" · ") || undefined} onClose={() => navigation.goBack()}>
    <View className="my-2.5 flex-row gap-2">
      <SheetButton label="Commit" disabled={nothing || !!busy} onPress={() => commit(false)} />
      <SheetButton label="Commit e Push" primary accent={accent} disabled={nothing || !!busy} onPress={() => commit(true)} />
    </View>
    {/* Pull, Push and refresh (the old header menu) have no place in the design's two buttons: they stay here, small. */}
    <View className="mb-1 flex-row items-center justify-center gap-5">
      {([["Pull", "arrow.down.circle", pull], ["Push", "arrow.up.circle", push], ["Atualizar", "arrow.clockwise", () => void refresh()]] as const).map(([label, icon, onPress]) =>
        <Pressable key={label} onPress={onPress} disabled={!!busy} accessibilityRole="button" accessibilityLabel={label} className="flex-row items-center gap-1.5 py-1.5 active:opacity-60 disabled:opacity-40">
          <SymbolView name={icon} size={14} tintColorClassName="accent-foreground-muted" />
          <AppText className="text-[13px] text-foreground-muted">{label}</AppText>
        </Pressable>)}
    </View>
    {busy ? <View className="flex-row items-center justify-center gap-2 py-2"><ActivityIndicator size="small" /><AppText className="text-sm text-foreground-muted">{busy}…</AppText></View> : null}
    {!snapshot && !error ? <ActivityIndicator className="mt-16" />
      : !sections.length ? <AppText className="mt-12 px-8 text-center text-sm text-foreground-muted">{error ?? (snapshot?.identity.isRepo === false ? "Esta pasta não é um repositório Git." : "Nenhuma alteração.")}</AppText>
      : sections.map((section) => <View key={section.title}>
        <SheetSection>{`${section.title} ${section.data.length}`}</SheetSection>
        <SheetGroup>
          {section.data.map((entry, index) => {
            const key = `${section.staged ? "s" : "u"}:${entry.path}`;
            const diff = diffs[key];
            const stat = entry.conflicted ? { letter: "!", className: "text-danger-foreground" } : KIND[entry.kind];
            return <View key={`${entry.path}:${index}`} className={index ? "border-t border-border-subtle" : ""}>
              <Pressable onPress={() => openFile(entry, section.staged)} className="flex-row items-center gap-[11px] py-3 active:opacity-60">
                <Pressable onPress={() => toggle(entry, section.staged)} hitSlop={10} disabled={!entry.actionable || !!busy} accessibilityLabel={section.staged ? "Tirar do commit" : "Incluir no commit"}>
                  <SymbolView name={section.staged ? "checkmark.circle.fill" : "circle"} size={20} tintColor={section.staged ? accent : undefined} tintColorClassName={section.staged ? undefined : "accent-foreground-tertiary"} />
                </Pressable>
                <View className="min-w-0 flex-1">
                  <AppText numberOfLines={1} className="text-[15px]">{entry.path.split("/").pop()}</AppText>
                  {entry.path.includes("/") ? <AppText numberOfLines={1} className="text-[12.5px] text-foreground-muted">{entry.path}</AppText> : null}
                </View>
                <DiffStat diff={diff} />
                <AppText className={`w-3.5 text-center font-app-medium text-xs ${stat.className}`}>{stat.letter}</AppText>
              </Pressable>
              {open === key ? <InlineDiff diff={diff} onFull={() => navigation.navigate("Diff", { computerId, sessionId, path: entry.path, staged: section.staged, indexToken: snapshot!.indexToken })} /> : null}
            </View>;
          })}
        </SheetGroup>
      </View>)}
  </SheetScreen>;
}

function SheetButton(props: { readonly label: string; readonly onPress: () => void; readonly primary?: boolean; readonly accent?: string; readonly disabled?: boolean }) {
  return <Pressable onPress={props.onPress} disabled={props.disabled} accessibilityRole="button" className={`flex-1 items-center rounded-[13px] py-2.5 active:opacity-70 ${props.primary ? "" : "bg-subtle-strong"} ${props.disabled ? "opacity-40" : ""}`}
    style={props.primary ? { backgroundColor: props.accent } : undefined}>
    <AppText numberOfLines={1} className="font-app-medium text-sm" style={props.primary ? { color: inkOn(props.accent ?? "") } : undefined}>{props.label}</AppText>
  </Pressable>;
}

/** The +/- of a file, counted from its diff once loaded. */
function DiffStat(props: { readonly diff: string | { error: string } | undefined }) {
  const counts = useMemo(() => {
    if (typeof props.diff !== "string") return null;
    const lines = props.diff.split("\n");
    return { added: lines.filter((line) => line.startsWith("+") && !line.startsWith("+++")).length, removed: lines.filter((line) => line.startsWith("-") && !line.startsWith("---")).length };
  }, [props.diff]);
  if (!counts || (!counts.added && !counts.removed)) return null;
  return <View className="flex-row gap-1.5">
    <AppText className="text-[13px] text-adaptive-emerald-600-400">+{counts.added}</AppText>
    <AppText className="text-[13px] text-danger-foreground">−{counts.removed}</AppText>
  </View>;
}

/** The design's `.df` block: the file's diff, added and removed lines tinted, scrolling sideways. */
function InlineDiff(props: { readonly diff: string | { error: string } | undefined; readonly onFull: () => void }) {
  const codeSize = Math.min(useAppearancePreferences().appearance.codeFontSize, 12);
  const text = typeof props.diff === "string" ? props.diff : "";
  const lines = useMemo(() => text.split("\n").filter((line) => !/^(diff --git|index |--- |\+\+\+ )/.test(line)), [text]);
  return <View className="mb-3 overflow-hidden rounded-xl bg-screen py-2">
    {props.diff === undefined ? <ActivityIndicator className="py-3" />
      : typeof props.diff !== "string" ? <AppText className="px-3 py-2 text-sm text-foreground-muted">{props.diff.error}</AppText>
      : !lines.some(Boolean) ? <AppText className="px-3 py-2 text-sm text-foreground-muted">Sem diferenças de texto.</AppText>
      : <ScrollView horizontal showsHorizontalScrollIndicator={false} style={{ maxHeight: 340 }}>
        <ScrollView nestedScrollEnabled showsVerticalScrollIndicator={false}>
          <View>
            {lines.map((line, index) => {
              const kind = line.startsWith("@@") ? "hunk" : line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : "ctx";
              return <AppText key={index} selectable
                className={kind === "add" ? "bg-adaptive-emerald-500-a12-a16 text-adaptive-emerald-600-400" : kind === "del" ? "bg-adaptive-rose-500-a12-a16 text-danger-foreground" : kind === "hunk" ? "text-foreground-muted" : "text-foreground-secondary"}
                style={{ fontFamily: "Menlo", fontSize: codeSize, lineHeight: Math.round(codeSize * 1.5), paddingHorizontal: 10, paddingTop: kind === "hunk" && index ? 8 : 0 }}>{line || " "}</AppText>;
            })}
          </View>
        </ScrollView>
      </ScrollView>}
    <Pressable onPress={props.onFull} accessibilityRole="button" className="items-end px-3 pt-2 active:opacity-60"><AppText className="text-[12.5px] text-foreground-muted">Abrir em tela cheia</AppText></Pressable>
  </View>;
}

/** One file's diff, lines colored as added, removed or hunk headers. */
export function DiffScreen() {
  const codeSize = useAppearancePreferences().appearance.codeFontSize;
  const { computerId, sessionId, path, staged, indexToken } = useRoute<RouteProp<GitParams, "Diff">>().params;
  const [diff, setDiff] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    useSirus.getState().invoke<GitWorkspaceResponse>(computerId, "git_workspace_action", { action: { type: "diff", sessionId, path, staged, expectedIndex: indexToken } })
      .then((result) => setDiff(result.type === "diff" ? result.diff : ""), (reason: unknown) => setError(errorText(reason)));
  }, [computerId, sessionId, path, staged, indexToken]);
  const lines = useMemo(() => (diff ?? "").split("\n").filter((line) => !/^(diff --git|index |--- |\+\+\+ )/.test(line)), [diff]);
  return <View className="flex-1 bg-thread-canvas">
    <NativeStackScreenOptions options={{ title: path.split("/").pop() ?? path, unstable_headerSubtitle: path }} />
    {diff === null && !error ? <ActivityIndicator className="mt-24" />
      : error || !lines.some(Boolean) ? <AppText className="mt-16 px-8 text-center text-sm text-foreground-muted">{error ?? "Sem diferenças de texto."}</AppText>
      : <ScrollView className="flex-1" contentInsetAdjustmentBehavior="automatic" contentContainerStyle={{ paddingVertical: 12, paddingBottom: 60 }}>
          <ScrollView horizontal showsHorizontalScrollIndicator={false}>
            <View>
              {lines.map((line, index) => {
                const kind = line.startsWith("@@") ? "hunk" : line.startsWith("+") ? "add" : line.startsWith("-") ? "del" : "ctx";
                return <AppText key={index} selectable
                  className={kind === "add" ? "bg-adaptive-emerald-500-a12-a16 text-foreground" : kind === "del" ? "bg-adaptive-rose-500-a12-a16 text-foreground" : kind === "hunk" ? "text-foreground-muted" : "text-foreground-secondary"}
                  style={{ fontFamily: "Menlo", fontSize: codeSize, lineHeight: Math.round(codeSize * 1.5), paddingHorizontal: 14, paddingTop: kind === "hunk" && index ? 10 : 0 }}>{line || " "}</AppText>;
              })}
            </View>
          </ScrollView>
        </ScrollView>}
  </View>;
}
