import { StackActions, useNavigation, useRoute, type NavigationProp, type ParamListBase } from "@react-navigation/native";
import * as Haptics from "expo-haptics";
import { createContext, use, useEffect, useMemo, useState, type ReactNode } from "react";
import { ActivityIndicator, Alert, Image, Pressable, ScrollView, View } from "react-native";
import { KeyboardStickyView } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { AgentProviderId, BranchInfo, Project } from "@sirus/client/types";
import { defaultApprovalFor } from "@sirus/lib/composer-approval";
import { latestModelChoices, providerModelChoices } from "@sirus/lib/model-registry";
import { PROVIDERS, providerById } from "@sirus/lib/provider-registry";
import { isProviderEnabled } from "@sirus/lib/settings";

import type { AppSymbolName } from "../components/AppSymbol";
import { SymbolView } from "../components/AppSymbol";
import { AppText, AppTextInput } from "../components/AppText";
import { ComposerSend } from "../components/ComposerSend";
import { ControlPillMenu } from "../components/ControlPillMenu";
import { GlassSurface } from "../components/GlassSurface";
import { ListSection } from "../components/ListSection";
import { ProjectRobot } from "../components/ProjectRobot";
import { ProviderIcon } from "../components/ProviderIcon";
import { pickAttachments } from "../data/attachments";
import { isScratch, SCRATCH_FOLDER } from "../data/scratch";
import { computerName, type Computer } from "../data/computers";
import { useSirus } from "../data/store";
import { createV5SheetStackNavigator } from "../native/createV5StackNavigator";
import { NativeHeaderToolbar, NativeStackScreenOptions } from "../native/StackHeader";
import { ProjectChip } from "../components/ProjectChip";
import { useMainTabs } from "./main-tabs-context";

const DEFAULT_MODEL = "m:";
const Sheet = createV5SheetStackNavigator();

interface Draft {
  projectKey: string | null;
  provider: AgentProviderId | null;
  model: string | null;
  isolated: boolean | null;
  prompt: string;
  files: { name: string; data: string }[];
}
export interface ProjectRow { readonly key: string; readonly computer: Computer; readonly project: Project }

const DraftContext = createContext<{ draft: Draft; setDraft: (update: (draft: Draft) => Draft) => void; close: () => void; started: (computerId: string, sessionId: string) => void } | null>(null);
const useDraft = () => use(DraftContext)!;

/** "Sem projeto" (`data/scratch.ts`) on a computer; its conversations never use a worktree. */
const scratchKey = (computerId: string) => `none:${computerId}`;

async function scratchProject(computerId: string): Promise<Project> {
  const existing = useSirus.getState().data[computerId]?.projects.find(isScratch);
  if (existing) return existing;
  const project = await useSirus.getState().invoke<Project>(computerId, "create_project", { name: SCRATCH_FOLDER, parent: "~" });
  useSirus.setState((state) => {
    const data = state.data[computerId];
    return data ? { data: { ...state.data, [computerId]: { ...data, projects: [...data.projects, project] } } } : {};
  });
  return project;
}

/** Every computer's projects, the most recently opened first. */
export function useProjects(): ProjectRow[] {
  const data = useSirus((state) => state.data);
  const computers = useSirus((state) => state.computers);
  return useMemo(() => computers.flatMap((computer) => (data[computer.id]?.projects ?? []).filter((project) => !isScratch(project)).map((project) => ({ key: `${computer.id}:${project.id}`, computer, project })))
    .sort((a, b) => b.project.lastOpenedAt.localeCompare(a.project.lastOpenedAt)), [computers, data]);
}

/**
 * A new conversation: "O que vamos construir em <projeto>?" with
 * the project opening "Escolher projeto", the computer under it; the workspace and branch above
 * the composer ("Branch" lists the project's branches); the composer with attachments, agent
 * and model, dictation and send. It starts on the computer that has the project.
 */
export function NewThreadScreen() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  // A project's page opens it with its project chosen (`computerId:projectId`).
  const preset = (useRoute().params as { projectKey?: string } | undefined)?.projectKey ?? null;
  const [draft, setDraftState] = useState<Draft>({ projectKey: preset, provider: null, model: null, isolated: null, prompt: "", files: [] });
  const value = {
    draft,
    setDraft: (update: (current: Draft) => Draft) => setDraftState(update),
    close: () => navigation.goBack(),
    started: (computerId: string, sessionId: string) => {
      navigation.goBack();
      navigation.dispatch(StackActions.push("Thread", { computerId, sessionId }));
    },
  };
  return <DraftContext value={value}>
    {/* the sheet opens on "Escolher projeto"; a project leads to the composer. */}
    <Sheet.Navigator initialRouteName={preset ? "NewThreadMain" : "NewThreadPick"} screenOptions={{ headerTransparent: true, headerShadowVisible: false, headerLargeTitle: false, contentStyle: { backgroundColor: "transparent" } } as never}>
      <Sheet.Screen name="NewThreadPick" component={ProjectPage} initialParams={{ first: true }} />
      <Sheet.Screen name="NewThreadMain" component={MainPage} />
      <Sheet.Screen name="NewThreadProject" component={ProjectPage} />
      <Sheet.Screen name="NewThreadBranch" component={BranchPage} />
    </Sheet.Navigator>
  </DraftContext>;
}

/**
 * "Novo" in the top bar's pill: the same new-conversation page (project, worktree or folder,
 * composer, agent and model) living on the main screen. The project comes from the drawer;
 * without one it starts in the most recently opened. A started conversation opens over it.
 */
export function NewThreadTab() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const tabs = useMainTabs();
  const project = tabs?.project ?? null;
  const [draft, setDraftState] = useState<Draft>({ projectKey: project, provider: null, model: null, isolated: null, prompt: "", files: [] });
  useEffect(() => { setDraftState((current) => current.projectKey === project ? current : { ...current, projectKey: project }); }, [project]);
  const value = {
    draft,
    setDraft: (update: (current: Draft) => Draft) => setDraftState(update),
    close: () => undefined,
    started: (computerId: string, sessionId: string) => {
      setDraftState((current) => ({ ...current, prompt: "", files: [] }));
      navigation.navigate("Thread", { computerId, sessionId });
    },
  };
  return <DraftContext value={value}><MainPage embedded /></DraftContext>;
}

/** The project's own icon (`project_auto_icon`), else a folder. */
export function ProjectIcon(props: { readonly computerId: string; readonly projectId: string; readonly size?: number }) {
  const [uri, setUri] = useState<string | null>(null);
  useEffect(() => {
    let live = true;
    useSirus.getState().invoke<string | null>(props.computerId, "project_auto_icon", { projectId: props.projectId })
      .then((value) => { if (live && value && /^(data:image\/|https:)/.test(value)) setUri(value); }, () => undefined);
    return () => { live = false; };
  }, [props.computerId, props.projectId]);
  const size = props.size ?? 20;
  return uri ? <Image source={{ uri }} style={{ width: size, height: size, borderRadius: 4 }} />
    : <SymbolView name="folder.fill" size={size - 2} tintColorClassName="accent-foreground-muted" />;
}

function MainPage(props: { readonly embedded?: boolean }) {
  const embedded = props.embedded === true;
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const tabs = useMainTabs();
  const insets = useSafeAreaInsets();
  const { draft, setDraft, close, started } = useDraft();
  const projects = useProjects();
  const data = useSirus((state) => state.data);
  const agents = useSirus((state) => state.agents);
  const catalogs = useSirus((state) => state.models);
  const computers = useSirus((state) => state.computers);
  const scratch = computers.find((computer) => scratchKey(computer.id) === draft.projectKey);
  const chosen = scratch ? undefined : projects.find((item) => item.key === draft.projectKey) ?? projects[0];
  const target = scratch ?? chosen?.computer;
  const computerId = target?.id;
  const settings = computerId ? data[computerId]?.settings ?? null : null;

  useEffect(() => {
    if (computerId && !agents[computerId]) void useSirus.getState().loadAgents(computerId).catch(() => undefined);
  }, [computerId, agents]);

  const installed = useMemo(() => !settings || !computerId ? [] : PROVIDERS
    .filter((definition) => (agents[computerId] ?? []).some((agent) => agent.id === definition.id && agent.installed) && isProviderEnabled(settings, definition.id))
    .map((definition) => definition.id), [agents, computerId, settings]);
  const wanted = draft.provider ?? settings?.defaultAgent ?? "claude";
  const provider = installed.includes(wanted) ? wanted : installed[0] ?? wanted;

  useEffect(() => {
    if (computerId && !catalogs[`${computerId}:${provider}`]) void useSirus.getState().loadModels(computerId, provider).catch(() => undefined);
  }, [computerId, provider, catalogs]);

  const models = useMemo(() => {
    const list = computerId ? catalogs[`${computerId}:${provider}`] : undefined;
    if (!list || !settings) return [];
    return latestModelChoices(providerModelChoices({ [provider]: list }, settings, provider, null, ""), null, "")
      .map((row) => ({ id: row.variantIds[0] ?? row.model.id, name: row.displayName }));
  }, [catalogs, computerId, provider, settings]);
  const model = draft.provider === provider && draft.model && models.some((item) => item.id === draft.model) ? draft.model : null;
  const modelName = models.find((item) => item.id === model)?.name.replace(/^Claude\s+/i, "") ?? "Padrão";

  const worktree = !scratch && (draft.isolated ?? settings?.defaultSessionWorkspace !== "checkout");
  const returnSends = useSirus((state) => state.prefs.returnKey === "send");
  const [busy, setBusy] = useState(false);
  const [branch, setBranch] = useState<string | null>(null);
  useEffect(() => {
    setBranch(null);
    if (!chosen) return;
    useSirus.getState().invoke<BranchInfo[]>(chosen.computer.id, "list_branches", { projectId: chosen.project.id })
      .then((list) => setBranch(list.find((item) => item.current)?.name ?? null), () => undefined);
  }, [chosen?.key]); // once per project

  const canStart = !!target && (!!draft.prompt.trim() || draft.files.length > 0) && !busy && installed.length > 0;
  const start = async () => {
    if (!canStart || !computerId) return;
    setBusy(true);
    void Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Light);
    try {
      const approval = defaultApprovalFor(providerById(provider).approvalModes, settings?.defaultApproval);
      const projectId = chosen ? chosen.project.id : (await scratchProject(computerId)).id;
      const session = await useSirus.getState().startSession(computerId, { projectId, agent: provider, model, isolatedWorktree: worktree, prompt: draft.prompt, approval, files: draft.files });
      started(computerId, session.id);
    } catch (reason) {
      setBusy(false);
      Alert.alert("Não foi possível começar", reason instanceof Error ? reason.message : String(reason));
    }
  };
  const attach = async (kind: "photos" | "files") => {
    try {
      const files = await pickAttachments(kind);
      if (files.length) setDraft((current) => ({ ...current, files: [...current.files, ...files].slice(0, 8) }));
    } catch (reason) {
      Alert.alert("Anexos", reason instanceof Error ? reason.message : String(reason));
    }
  };
  const agentActions = [
    { id: "agents", title: "Agente", displayInline: true, subactions: installed.map((id) => ({ id: `p:${id}`, title: providerById(id).name, state: id === provider ? "on" as const : "off" as const })) },
    { id: "models", title: "Modelo", displayInline: true, subactions: [
      { id: DEFAULT_MODEL, title: "Padrão do CLI", state: model === null ? "on" as const : "off" as const },
      ...models.map((item) => ({ id: `m:${item.id}`, title: item.name, state: item.id === model ? "on" as const : "off" as const })),
    ] },
  ];
  const fromOrigin = settings?.worktreesFromOrigin ?? false;
  const branchLabel = branch ? worktree ? fromOrigin ? `A partir de origin/${branch}` : `A partir de ${branch}` : branch : "…";

  const openProjects = () => { if (embedded) tabs?.setDrawerOpen(true); else navigation.navigate("NewThreadProject"); };
  return <View className={embedded ? "flex-1 bg-screen" : "flex-1 bg-sheet"}>
    {embedded ? null : <>
      <NativeStackScreenOptions options={{ title: "", headerBackVisible: false }} />
      <NativeHeaderToolbar placement="left">
        <NativeHeaderToolbar.Button accessibilityLabel="Cancelar" label="Cancelar" onPress={close} />
      </NativeHeaderToolbar>
    </>}
    <View style={{ flex: 1 }}>
      <ScrollView style={{ flex: 1 }} contentInsetAdjustmentBehavior={embedded ? "never" : "automatic"} keyboardDismissMode="interactive" keyboardShouldPersistTaps="handled"
        contentContainerStyle={{ flexGrow: 1, alignItems: "center", paddingHorizontal: embedded ? 16 : 24, paddingTop: embedded ? tabs?.topInset ?? 60 : 4 }} showsVerticalScrollIndicator={false}>
        {embedded && target ? <View className="w-full items-center">
          <ProjectChip projectKey={tabs?.project ?? null} />
          <View className="flex-1 items-center justify-center gap-1.5 pb-24">
            <ProjectRobot project={chosen?.project} name={chosen ? undefined : "Sem projeto"} size={76} />
            <AppText className="mt-3 text-center font-app-medium text-2xl tracking-tight">Em que vamos trabalhar?</AppText>
            <Pressable onPress={openProjects} accessibilityRole="button" accessibilityLabel="Escolher projeto" className="active:opacity-60">
              <AppText numberOfLines={1} className="text-center text-sm text-foreground-muted">{chosen ? chosen.project.name : "Sem projeto"}{computers.length > 1 ? ` · ${computerName(target)}` : ""}</AppText>
            </Pressable>
          </View>
        </View> : target ? <View className="w-full items-center gap-1.5">
          <AppText className="text-center font-app-medium text-2xl tracking-tight">{chosen ? "O que vamos construir" : "O que vamos fazer?"}</AppText>
          {chosen ? <View className="max-w-full flex-row items-center justify-center">
            <AppText className="font-app-medium text-2xl tracking-tight">em </AppText>
            <Pressable onPress={openProjects} accessibilityLabel="Escolher projeto" className="min-w-0 max-w-[240px] border-b border-foreground-muted active:opacity-60">
              <AppText numberOfLines={1} className="font-app-medium text-2xl tracking-tight">{chosen.project.name}</AppText>
            </Pressable>
            <AppText className="font-app-medium text-2xl tracking-tight">?</AppText>
          </View> : <Pressable onPress={openProjects} className="border-b border-foreground-muted active:opacity-60">
            <AppText className="text-base text-foreground-muted">Sem projeto</AppText>
          </Pressable>}
          <View className="mt-3 flex-row items-center gap-1.5">
            <SymbolView name={(target.kind === "server" ? "server.rack" : "laptopcomputer") as AppSymbolName} size={13} tintColorClassName="accent-foreground-muted" />
            <AppText className="text-sm text-foreground-muted">no {computerName(target)}</AppText>
          </View>
        </View> : <AppText className="mt-10 text-center text-base text-foreground-muted">Adicione um projeto no Sirus do computador para começar uma conversa.</AppText>}
      </ScrollView>
      {/* The composer rides the keyboard as the conversation's does (the sheet reaches the screen's bottom). */}
      <KeyboardStickyView offset={{ closed: 0, opened: Math.max(insets.bottom, 12) - 8 }}>
      <View style={{ paddingHorizontal: 12, paddingBottom: Math.max(insets.bottom, 12) }}>
        {chosen ? <View className="mb-2 flex-row items-center gap-4 px-2">
          <Pressable onPress={() => setDraft((current) => ({ ...current, isolated: !worktree }))} className="flex-row items-center gap-1.5 active:opacity-60" accessibilityRole="button">
            <SymbolView name={(worktree ? "folder.badge.plus" : "folder") as AppSymbolName} size={14} tintColorClassName="accent-foreground-muted" />
            <AppText className="text-sm text-foreground-muted">{worktree ? "Novo worktree" : "Pasta do projeto"}</AppText>
          </Pressable>
          <Pressable onPress={worktree ? undefined : () => embedded ? navigation.navigate("NewThread", { projectKey: chosen?.key }) : navigation.navigate("NewThreadBranch")} disabled={worktree} className="min-w-0 flex-row items-center gap-1.5 active:opacity-60" accessibilityRole="button">
            <SymbolView name="arrow.triangle.branch" size={13} tintColorClassName="accent-foreground-muted" />
            <AppText numberOfLines={1} className="shrink text-sm text-foreground-muted">{branchLabel}</AppText>
            {worktree ? null : <SymbolView name="chevron.right" size={10} weight="semibold" tintColorClassName="accent-foreground-muted" />}
          </Pressable>
        </View> : null}
        <GlassSurface style={{ borderRadius: 26 }} className="gap-1 px-1.5 pb-1.5 pt-1.5">
          {draft.files.length ? <ScrollView horizontal showsHorizontalScrollIndicator={false} keyboardShouldPersistTaps="always" contentContainerClassName="gap-1.5 px-1.5 pt-1">
            {draft.files.map((file, index) => <View key={`${file.name}:${index}`} className="h-8 max-w-[180px] flex-row items-center gap-1.5 rounded-full bg-card pl-2.5 pr-1">
              <SymbolView name="doc" size={12} tintColorClassName="accent-foreground-muted" />
              <AppText numberOfLines={1} className="shrink text-xs">{file.name}</AppText>
              <Pressable onPress={() => setDraft((current) => ({ ...current, files: current.files.filter((_, at) => at !== index) }))} hitSlop={8} className="size-6 items-center justify-center rounded-full">
                <SymbolView name="xmark" size={9} weight="semibold" tintColorClassName="accent-foreground-muted" />
              </Pressable>
            </View>)}
          </ScrollView> : null}
          <AppTextInput value={draft.prompt} onChangeText={(prompt) => setDraft((current) => ({ ...current, prompt }))} placeholder={embedded ? "Peça algo ao Sirus…" : "Pergunte qualquer coisa…"} multiline autoFocus={!embedded}
            {...(returnSends ? { submitBehavior: "submit" as const, returnKeyType: "send" as const, onSubmitEditing: () => void start() } : {})}
            className="max-h-40 min-h-[64px] rounded-none border-0 bg-transparent px-2 py-2 text-base" />
          <View className="flex-row items-center gap-1">
            <ControlPillMenu actions={[{ id: "photos", title: "Fotos", image: "photo" }, { id: "files", title: "Escolher arquivos", image: "folder" }]}
              onPressAction={({ nativeEvent }) => void attach(nativeEvent.event as "photos" | "files")} accessibilityLabel="Anexar">
              <View className="size-[38px] items-center justify-center"><SymbolView name="plus" size={20} tintColorClassName="accent-icon" /></View>
            </ControlPillMenu>
            <View className="flex-1" />
            <ControlPillMenu actions={agentActions} onPressAction={({ nativeEvent }) => {
              const id = nativeEvent.event;
              if (id.startsWith("p:")) setDraft((current) => ({ ...current, provider: id.slice(2) as AgentProviderId, model: null }));
              else setDraft((current) => ({ ...current, provider, model: id === DEFAULT_MODEL ? null : id.slice(2) }));
            }} accessibilityLabel="Agente e modelo">
              <View className="h-[38px] max-w-[180px] flex-row items-center gap-1.5 rounded-full px-2.5">
                <ProviderIcon provider={provider} size={15} />
                <AppText numberOfLines={1} className="shrink font-app-medium text-sm">{modelName}</AppText>
                <SymbolView name="chevron.down" size={10} weight="semibold" tintColorClassName="accent-foreground-muted" />
              </View>
            </ControlPillMenu>
            <View className="pr-1">
              <ComposerSend text={draft.prompt} onTextChange={(prompt) => setDraft((current) => ({ ...current, prompt }))} onSend={() => void start()} sending={busy}
                disabled={!target || !installed.length} canSend={canStart} />
            </View>
          </View>
        </GlassSurface>
      </View>
      </KeyboardStickyView>
    </View>
  </View>;
}

function SearchPage(props: { readonly title: string; readonly placeholder: string; readonly query: string; readonly onQuery: (value: string) => void; readonly headerRight?: ReactNode; readonly children: ReactNode }) {
  const insets = useSafeAreaInsets();
  return <View className="flex-1 bg-sheet">
    <NativeStackScreenOptions options={{ title: props.title }} />
    {props.headerRight}
    <ScrollView contentInsetAdjustmentBehavior="automatic" keyboardShouldPersistTaps="handled" className="flex-1" contentContainerClassName="gap-4 px-5 pt-4" contentContainerStyle={{ paddingBottom: 96 }}>
      {props.children}
    </ScrollView>
    <View className="absolute inset-x-5 flex-row items-center gap-2 rounded-full bg-grouped-card px-4" style={{ bottom: Math.max(insets.bottom, 12), height: 48 }}>
      <SymbolView name="magnifyingglass" size={16} tintColorClassName="accent-foreground-muted" />
      <AppTextInput value={props.query} onChangeText={props.onQuery} placeholder={props.placeholder} autoCapitalize="none" autoCorrect={false} returnKeyType="search"
        className="min-h-0 flex-1 rounded-none border-0 bg-transparent px-0 py-0 text-base" />
      {props.query ? <Pressable onPress={() => props.onQuery("")} hitSlop={8}><SymbolView name="xmark.circle.fill" size={16} tintColorClassName="accent-foreground-muted" /></Pressable> : null}
    </View>
  </View>;
}

/** "Escolher projeto": each project with its icon and path, a search at the bottom. */
function ProjectPage() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const first = (useRoute().params as { first?: boolean } | undefined)?.first === true;
  const { draft, setDraft } = useDraft();
  const projects = useProjects();
  const computers = useSirus((state) => state.computers);
  const [query, setQuery] = useState("");
  const needle = query.trim().toLocaleLowerCase();
  const shown = projects.filter((row) => !needle || row.project.name.toLocaleLowerCase().includes(needle) || row.project.path.toLocaleLowerCase().includes(needle));
  const selected = draft.projectKey ?? projects[0]?.key;
  const choose = (key: string) => {
    setDraft((current) => ({ ...current, projectKey: key }));
    if (first) navigation.dispatch(StackActions.replace("NewThreadMain")); else navigation.goBack();
  };
  return <SearchPage title="Escolher projeto" placeholder="Buscar projetos" query={query} onQuery={setQuery}
    headerRight={<NativeHeaderToolbar placement="right">
      <NativeHeaderToolbar.Menu accessibilityLabel="Adicionar projeto" icon="plus" title="Adicionar projeto">
        <NativeHeaderToolbar.MenuAction icon="arrow.down.circle" subtitle="dono/repositório ou endereço" onPress={() => addProject("clone", projects, choose)}>
          <NativeHeaderToolbar.Label>Clonar do GitHub</NativeHeaderToolbar.Label>
        </NativeHeaderToolbar.MenuAction>
        <NativeHeaderToolbar.MenuAction icon="folder.badge.plus" subtitle="Uma pasta que já está no computador" onPress={() => addProject("folder", projects, choose)}>
          <NativeHeaderToolbar.Label>Adicionar uma pasta</NativeHeaderToolbar.Label>
        </NativeHeaderToolbar.MenuAction>
      </NativeHeaderToolbar.Menu>
    </NativeHeaderToolbar>}>
    {needle ? null : <ListSection>
      {computers.map((computer, index) => <Pressable key={computer.id} onPress={() => choose(scratchKey(computer.id))}
        className={`bg-grouped-card active:opacity-70 ${index ? "border-t border-border-subtle" : ""}`}>
        <View className="flex-row items-center gap-3 px-4 py-3">
          <View className="size-7 items-center justify-center"><SymbolView name={"bubble.left" as AppSymbolName} size={17} tintColorClassName="accent-foreground-muted" /></View>
          <View className="min-w-0 flex-1">
            <AppText numberOfLines={1} className="font-app-bold text-base">Sem projeto</AppText>
            <AppText numberOfLines={1} className="text-sm text-foreground-muted">{computers.length > 1 ? `No ${computerName(computer)} · ` : ""}Começar uma conversa sem projeto</AppText>
          </View>
          {selected === scratchKey(computer.id) ? <SymbolView name="checkmark" size={14} weight="semibold" tintColorClassName="accent-foreground" /> : <SymbolView name="chevron.right" size={12} tintColorClassName="accent-chevron" />}
        </View>
      </Pressable>)}
    </ListSection>}
    {shown.length ? <ListSection>
      {shown.map((row, index) => <Pressable key={row.key} onPress={() => choose(row.key)}
        className={`bg-grouped-card active:opacity-70 ${index ? "border-t border-border-subtle" : ""}`}>
        <View className="flex-row items-center gap-3 px-4 py-3">
          <View className="size-7 items-center justify-center"><ProjectIcon computerId={row.computer.id} projectId={row.project.id} /></View>
          <View className="min-w-0 flex-1">
            <AppText numberOfLines={1} className="font-app-bold text-base">{row.project.name}</AppText>
            <AppText numberOfLines={1} ellipsizeMode="middle" className="text-sm text-foreground-muted">{computers.length > 1 ? `${computerName(row.computer)} · ` : ""}{row.project.path}</AppText>
          </View>
          {row.key === selected ? <SymbolView name="checkmark" size={14} weight="semibold" tintColorClassName="accent-foreground" /> : <SymbolView name="chevron.right" size={12} tintColorClassName="accent-chevron" />}
        </View>
      </Pressable>)}
    </ListSection> : <AppText className="mt-10 text-center text-sm text-foreground-muted">Nenhum projeto encontrado.</AppText>}
  </SearchPage>;
}

/**
 * Adds a project on the computer with the most recent one, as the web app's Add project: a
 * GitHub clone or an existing folder, next to the other projects (their parent folder).
 */
export function addProject(kind: "clone" | "folder", projects: ProjectRow[], choose: (key: string) => void) {
  const recent = projects[0];
  const computer = recent?.computer ?? useSirus.getState().computers[0];
  if (!computer) return;
  const parent = recent ? recent.project.path.replace(/\/[^/]+\/?$/, "") : "";
  const done = (project: Project) => {
    useSirus.setState((state) => {
      const data = state.data[computer.id];
      return data ? { data: { ...state.data, [computer.id]: { ...data, projects: [project, ...data.projects.filter((item) => item.id !== project.id)] } } } : {};
    });
    choose(`${computer.id}:${project.id}`);
  };
  const fail = (reason: unknown) => Alert.alert("Adicionar projeto", reason instanceof Error ? reason.message : String(reason));
  if (kind === "clone") {
    Alert.prompt("Clonar do GitHub", `Em ${parent || "sua pasta de projetos"}.`, [
      { text: "Cancelar", style: "cancel" },
      { text: "Clonar", onPress: (source?: string) => {
        if (!source?.trim()) return;
        useSirus.getState().invoke<Project>(computer.id, "clone_project", { source: source.trim(), parent, folder: null }).then(done, fail);
      } },
    ], "plain-text", "", "url");
  } else {
    Alert.prompt("Adicionar uma pasta", "O caminho da pasta no computador.", [
      { text: "Cancelar", style: "cancel" },
      { text: "Adicionar", onPress: (path?: string) => {
        if (!path?.trim()) return;
        useSirus.getState().invoke<Project>(computer.id, "add_project", { path: path.trim() }).then(done, fail);
      } },
    ], "plain-text", parent ? `${parent}/` : "", "url");
  }
}

/** "Branch": the project's branches, the current one checked; picking one checks it out. */
function BranchPage() {
  const navigation = useNavigation();
  const { draft } = useDraft();
  const projects = useProjects();
  const chosen = projects.find((item) => item.key === draft.projectKey) ?? projects[0];
  const [branches, setBranches] = useState<BranchInfo[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  useEffect(() => {
    if (!chosen) return;
    useSirus.getState().invoke<BranchInfo[]>(chosen.computer.id, "list_branches", { projectId: chosen.project.id }).then(setBranches, () => setBranches([]));
  }, [chosen?.key]); // once per project
  const needle = query.trim().toLocaleLowerCase();
  const shown = (branches ?? []).filter((item) => !needle || item.name.toLocaleLowerCase().includes(needle));
  const pick = async (item: BranchInfo) => {
    if (!chosen || item.current) { navigation.goBack(); return; }
    setBusy(item.name);
    try {
      await useSirus.getState().invoke<void>(chosen.computer.id, "checkout_branch", { projectId: chosen.project.id, branch: item.name, confirm: true });
      navigation.goBack();
    } catch (reason) {
      Alert.alert("Branch", reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(null);
    }
  };
  return <SearchPage title="Branch" placeholder="Buscar branch" query={query} onQuery={setQuery}>
    {branches === null ? <ActivityIndicator className="mt-10" /> : shown.length ? <ListSection>
      {shown.map((item, index) => <Pressable key={item.name} onPress={() => void pick(item)} disabled={!!busy}
        className={`bg-grouped-card active:opacity-70 ${index ? "border-t border-border-subtle" : ""}`}>
        <View className="flex-row items-center gap-3 px-4 py-3">
          <SymbolView name="arrow.triangle.branch" size={14} tintColorClassName="accent-foreground-muted" />
          <View className="min-w-0 flex-1">
            <AppText numberOfLines={1} className="text-base">{item.name}</AppText>
            {item.current ? <AppText className="text-xs text-foreground-muted">Atual</AppText> : null}
          </View>
          {busy === item.name ? <ActivityIndicator size="small" /> : item.current ? <SymbolView name="checkmark" size={14} weight="semibold" tintColorClassName="accent-foreground" /> : null}
        </View>
      </Pressable>)}
    </ListSection> : <AppText className="mt-10 text-center text-sm text-foreground-muted">Nenhuma branch encontrada.</AppText>}
  </SearchPage>;
}
