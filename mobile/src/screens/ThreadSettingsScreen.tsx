import { useNavigation, useRoute, type NavigationProp, type ParamListBase, type RouteProp } from "@react-navigation/native";
import { createContext, use, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { Alert, Pressable, ScrollView, View } from "react-native";
import { KeyboardStickyView } from "react-native-keyboard-controller";
import { useSafeAreaInsets } from "react-native-safe-area-context";

import type { AgentProviderId, ApprovalMode, ExecutionOptions } from "@sirus/client/types";
import { translate } from "@sirus/i18n";
import { composerApproval } from "@sirus/lib/composer-approval";
import { modelExecutionControls } from "@sirus/lib/execution-options";
import { approvalOptions, chooseExecution, isDefaultChoice, threadSettingsOptions, threadSettingsSections, threadSettingsSummary } from "@sirus/lib/phone-chat";
import { PROVIDERS, providerById } from "@sirus/lib/provider-registry";
import { isProviderEnabled, modelKey } from "@sirus/lib/settings";

import type { AppSymbolName } from "../components/AppSymbol";
import { SymbolView } from "../components/AppSymbol";
import { AppText, AppTextInput } from "../components/AppText";
import { ListChoiceRow } from "../components/ListChoiceRow";
import { ListRow } from "../components/ListRow";
import { ListSection } from "../components/ListSection";
import { ProviderIcon } from "../components/ProviderIcon";
import { SheetGroup, SheetRow, SheetScreen } from "../components/SheetKit";
import { RoundButton } from "../components/TopBar";
import { useMacAccent } from "../data/mac-accent";
import { useSirus } from "../data/store";
import { createV5SheetStackNavigator } from "../native/createV5StackNavigator";
import { NativeHeaderToolbar, NativeStackScreenOptions } from "../native/StackHeader";

const t = (key: string, params?: Record<string, string | number>) => translate("pt-BR", key, params);
const APPROVAL_LABEL: Record<ApprovalMode, string> = { ask: "composer.askApproval", auto: "composer.autoReview", full: "composer.fullAccess" };
const APPROVAL_SYMBOL: Record<ApprovalMode, string> = { ask: "hand.raised", auto: "person.badge.shield.checkmark", full: "bolt" };

interface Draft { provider: AgentProviderId; model: string | null; preference: ExecutionOptions; save: boolean; approval: ApprovalMode | null }
type ThreadSettingsParams = { ThreadSettings: { computerId: string; sessionId: string } };

/**
 * The conversation's settings sheet, after the phone web app's (ADR-129, "summary first"): the
 * pending model and its options on one page, the model list and each option's choices on pages
 * that push in. Choices wait in a draft; OK applies them as the desktop composer does (the model
 * through `set_session_model`, reasoning and Fast saved per model, access per provider in this
 * conversation); Cancel drops them.
 */
const DraftContext = createContext<{
  computerId: string; sessionId: string; draft: Draft; setDraft: (update: (draft: Draft) => Draft) => void;
  providers: AgentProviderId[]; locked: boolean; apply: () => void; close: () => void;
} | null>(null);
const useDraft = () => use(DraftContext)!;

const Sheet = createV5SheetStackNavigator();

export function ThreadSettingsScreen() {
  const { computerId, sessionId } = useRoute<RouteProp<ThreadSettingsParams, "ThreadSettings">>().params;
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const session = useSirus((state) => state.data[computerId]?.sessions.find((item) => item.id === sessionId));
  const settings = useSirus((state) => state.data[computerId]?.settings ?? null);
  const agents = useSirus((state) => state.agents[computerId]);
  const provider = session?.agent ?? settings?.defaultAgent ?? "claude";
  const model = session?.model ?? null;
  const [draft, setDraftState] = useState<Draft>(() => ({ provider, model, preference: settings?.modelExecution[modelKey(provider, model ?? "")] ?? {}, save: false, approval: null }));
  const providers = useMemo(() => settings ? PROVIDERS.filter((definition) => (agents ?? []).some((item) => item.id === definition.id && item.installed) && isProviderEnabled(settings, definition.id)).map((definition) => definition.id) : [], [agents, settings]);
  const locked = !!session && ["starting", "running", "waiting"].includes(session.status);
  useEffect(() => { if (!agents) void useSirus.getState().loadAgents(computerId).catch(() => undefined); }, [agents, computerId]);

  const apply = () => {
    const store = useSirus.getState();
    const fail = (reason: unknown) => Alert.alert(t("phoneChat.settings"), reason instanceof Error ? reason.message : String(reason));
    if (draft.provider !== provider || draft.model !== model) void store.setSessionModel(computerId, sessionId, draft.provider, draft.model).catch(fail);
    if (draft.save && draft.model && settings) {
      const key = modelKey(draft.provider, draft.model);
      void store.saveSettings(computerId, { modelExecution: { ...settings.modelExecution, [key]: { ...settings.modelExecution[key], ...draft.preference } } }).catch(fail);
    }
    if (draft.approval) store.setApproval(computerId, sessionId, draft.provider, draft.approval);
    navigation.goBack();
  };
  const value = { computerId, sessionId, draft, setDraft: (update: (current: Draft) => Draft) => setDraftState(update), providers, locked, apply, close: () => navigation.goBack() };
  return <DraftContext value={value}>
    <Sheet.Navigator screenOptions={{ headerTransparent: true, headerShadowVisible: false, headerLargeTitle: false, contentStyle: { backgroundColor: "transparent" } } as never}>
      <Sheet.Screen name="ThreadSettingsModel" component={ModelPage} options={{ headerShown: false }} />
      <Sheet.Screen name="ThreadSettingsMain" component={MainPage} />
      <Sheet.Screen name="ThreadSettingsEffort" component={EffortPage} />
      <Sheet.Screen name="ThreadSettingsTier" component={TierPage} />
      <Sheet.Screen name="ThreadSettingsAccess" component={AccessPage} />
    </Sheet.Navigator>
  </DraftContext>;
}

function Page(props: { readonly title: string; readonly children: ReactNode; readonly main?: boolean; readonly footer?: ReactNode }) {
  const insets = useSafeAreaInsets();
  const { apply, close } = useDraft();
  return <View collapsable={false} className="flex-1 bg-sheet">
    <NativeStackScreenOptions options={{ title: props.title, headerBackVisible: !props.main }} />
    {props.main ? <>
      <NativeHeaderToolbar placement="left"><NativeHeaderToolbar.Button accessibilityLabel={t("common.cancel")} label="Cancelar" onPress={close} /></NativeHeaderToolbar>
      <NativeHeaderToolbar placement="right"><NativeHeaderToolbar.Button accessibilityLabel={t("phoneChat.done")} label={t("phoneChat.done")} onPress={apply} /></NativeHeaderToolbar>
    </> : null}
    <ScrollView contentInsetAdjustmentBehavior="automatic" showsVerticalScrollIndicator={false} className="flex-1" keyboardShouldPersistTaps="handled"
      contentContainerClassName="gap-5 px-5 pt-4" contentContainerStyle={{ paddingBottom: Math.max(insets.bottom, 18) + 18 }}>
      {props.children}
    </ScrollView>
    {props.footer}
  </View>;
}

function useDraftState() {
  const context = useDraft();
  const settings = useSirus((state) => state.data[context.computerId]?.settings ?? null);
  const session = useSirus((state) => state.data[context.computerId]?.sessions.find((item) => item.id === context.sessionId));
  const catalogs = useSirus((state) => state.models);
  const byProvider = useMemo(() => Object.fromEntries(context.providers.map((id) => [id, catalogs[`${context.computerId}:${id}`]])), [catalogs, context.computerId, context.providers]);
  const chosen = useSirus((state) => state.approvals[`${context.computerId}:${context.sessionId}`]);
  const models = byProvider[context.draft.provider]?.models ?? [];
  const controls = modelExecutionControls(context.draft.provider, context.draft.model, models, context.draft.preference);
  const definition = providerById(context.draft.provider);
  const approval = context.draft.approval ?? composerApproval(context.draft.provider, session, chosen, false, settings?.defaultApproval).effective;
  return { ...context, settings, session, byProvider, models, controls, definition, approval };
}

function ModelRows(props: { readonly providers: AgentProviderId[]; readonly query: string; readonly stay?: boolean; readonly grouped?: boolean }) {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const { settings, byProvider, draft, setDraft, locked } = useDraftState();
  if (!settings) return null;
  const sections = threadSettingsSections(byProvider, settings, props.providers, draft.provider, draft.model, props.query);
  const pick = (provider: AgentProviderId, id: string) => {
    if (locked) return;
    setDraft((current) => ({ provider, model: id, preference: settings.modelExecution[modelKey(provider, id)] ?? {}, save: false, approval: provider === current.provider ? current.approval : null }));
    if (!props.stay) navigation.goBack();
  };
  return <>{sections.map(({ provider, rows }) => <ListSection key={provider} title={props.grouped ? providerById(provider).name : t("phoneChat.providerModels", { provider: providerById(provider).name })}
    titleIcon={props.grouped ? <ProviderIcon provider={provider} size={14} /> : undefined}>
    {rows.length ? rows.map((row, index) => <ListChoiceRow key={modelKey(row.provider, row.model.id)} separated={index > 0} disabled={locked}
      label={row.displayName.replace(/^Claude\s+/i, "")} description={isDefaultChoice(settings, row) ? t("phoneChat.default") : undefined}
      selected={row.provider === draft.provider && row.variantIds.includes(draft.model ?? "")} onPress={() => pick(row.provider, row.model.id)} />)
      : <AppText className="bg-grouped-card px-4 py-3 text-sm text-foreground-muted">{t(byProvider[provider] ? "models.noProviderModels" : "models.loading")}</AppText>}
  </ListSection>)}</>;
}

function MainPage() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const state = useDraftState();
  const { draft, settings, models, controls, definition, approval, locked, computerId, session } = state;
  useEffect(() => { if (!state.byProvider[draft.provider]) void useSirus.getState().loadModels(computerId, draft.provider).catch(() => undefined); }, [computerId, draft.provider, state.byProvider]);
  const summary = settings ? threadSettingsSummary(settings, models, draft.provider, draft.model, session ? session.agent : null) : { name: null, note: null };
  const shown = threadSettingsOptions(controls, definition.approvalModes);
  const go = (screen: string) => () => navigation.navigate(screen);
  return <Page title={t("phoneChat.settings")} main>
    <View className="flex-row items-center gap-3 rounded-[24px] bg-grouped-card p-4">
      <View className="size-11 items-center justify-center rounded-2xl bg-sheet"><ProviderIcon provider={draft.provider} size={24} /></View>
      <View className="min-w-0 flex-1">
        <AppText numberOfLines={1} className="font-app-medium text-lg">{summary.name ?? t("models.select")}</AppText>
        <AppText numberOfLines={1} className="text-sm text-foreground-muted">{definition.name}{summary.note ? ` · ${t(summary.note === "switch" ? "phoneChat.switchesAgent" : "phoneChat.default")}` : ""}</AppText>
      </View>
      <Pressable onPress={() => navigation.goBack()} disabled={locked} className="h-9 justify-center rounded-full bg-sheet px-4 active:opacity-60 disabled:opacity-40">
        <AppText className="font-app-medium text-sm">{t("phoneChat.change")}</AppText>
      </Pressable>
    </View>
    {locked ? <AppText className="-mt-2 px-2 text-sm text-foreground-muted">{t("phoneChat.modelLocked")}</AppText> : null}
    <ModelRows providers={[draft.provider]} query="" stay />
    {shown.reasoning || shown.tier || shown.access ? <ListSection title={t("phoneChat.options")}>
      {shown.reasoning ? <ListRow icon={"lightbulb" as AppSymbolName} label={t("phoneChat.reasoning")} detail={controls.effort ? t(`effort.${controls.effort}`) : t("composer.effortAuto")}
        disabled={locked || !draft.model || controls.selectableLevels.length < 2} onPress={go("ThreadSettingsEffort")} /> : null}
      {shown.tier ? <ListRow icon={"bolt" as AppSymbolName} label={t("phoneChat.serviceTier")} detail={t(controls.fast ? "phoneChat.fast" : "phoneChat.standard")} disabled={locked} onPress={go("ThreadSettingsTier")} /> : null}
      {shown.access ? <ListRow icon={APPROVAL_SYMBOL[approval] as AppSymbolName} label={t("phoneChat.access")}
        detail={t(approval === "ask" && definition.approvalPolicy === "vendor" ? "composer.vendorApproval" : APPROVAL_LABEL[approval])} onPress={go("ThreadSettingsAccess")} /> : null}
    </ListSection> : null}
  </Page>;
}

/** A provider's models as a group that expands: the chosen model checked in the Mac's accent, the count on closed ones. */
function ProviderGroup(props: { readonly provider: AgentProviderId; readonly open: boolean; readonly onToggle: () => void; readonly accent: string; readonly searching: boolean }) {
  const { settings, byProvider, draft, setDraft, locked } = useDraftState();
  const query = useContext(QueryContext);
  if (!settings) return null;
  // The unfiltered count is what the closed header shows; the filtered rows are what an open one lists.
  const all = threadSettingsSections(byProvider, settings, [props.provider], draft.provider, draft.model, "")[0]?.rows ?? [];
  const rows = props.searching ? threadSettingsSections(byProvider, settings, [props.provider], draft.provider, draft.model, query)[0]?.rows ?? [] : all;
  const name = providerById(props.provider).name;
  const pick = (provider: AgentProviderId, id: string) => {
    if (locked) return;
    setDraft((current) => ({ provider, model: id, preference: settings.modelExecution[modelKey(provider, id)] ?? {}, save: false, approval: provider === current.provider ? current.approval : null }));
  };
  return <SheetGroup className="mt-3">
    <SheetRow first bold title={name} lead={<ProviderIcon provider={props.provider} size={20} />} onPress={props.onToggle}
      accessory={<View className="flex-row items-center gap-2">
        {!props.open && byProvider[props.provider] ? <AppText className="text-[13px] text-foreground-tertiary">{all.length}</AppText> : null}
        <SymbolView name={props.open ? "chevron.down" : "chevron.right"} size={13} weight="semibold" tintColorClassName="accent-foreground-muted" />
      </View>} />
    {props.open ? (rows.length ? rows.map((row) => {
      const selected = row.provider === draft.provider && row.variantIds.includes(draft.model ?? "");
      return <SheetRow key={modelKey(row.provider, row.model.id)} title={row.displayName.replace(/^Claude\s+/i, "")} note={isDefaultChoice(settings, row) ? t("phoneChat.default") : undefined}
        disabled={locked} onPress={() => pick(row.provider, row.model.id)}
        accessory={selected ? <SymbolView name="checkmark" size={17} weight="semibold" tintColor={props.accent} /> : undefined} />;
    }) : <View className="border-t border-border-subtle py-3"><AppText className="text-sm text-foreground-muted">{t(byProvider[props.provider] ? "models.noProviderModels" : "models.loading")}</AppText></View>) : null}
  </SheetGroup>;
}

const QueryContext = createContext("");

/**
 * Modelo, the sheet's first page (design "Modelo"): the chosen model on top with its access and
 * effort and an Options row (the old summary page: reasoning, speed, access), provider groups
 * that expand, and a fixed field at the bottom filtering models live. The round check applies
 * the draft as the old Done did; the close button drops it.
 */
function ModelPage() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const state = useDraftState();
  const { providers, computerId, byProvider, draft, settings, models, controls, definition, approval, session, apply, close } = state;
  const accent = useMacAccent(computerId);
  const insets = useSafeAreaInsets();
  const [query, setQuery] = useState("");
  const [expanded, setExpanded] = useState<Partial<Record<AgentProviderId, boolean>>>({});
  useEffect(() => { for (const id of providers) if (!byProvider[id]) void useSirus.getState().loadModels(computerId, id).catch(() => undefined); }, [byProvider, computerId, providers]);
  const summary = settings ? threadSettingsSummary(settings, models, draft.provider, draft.model, session ? session.agent : null) : { name: null, note: null };
  const shown = threadSettingsOptions(controls, definition.approvalModes);
  const note = [shown.access ? t(approval === "ask" && definition.approvalPolicy === "vendor" ? "composer.vendorApproval" : APPROVAL_LABEL[approval]) : null, shown.reasoning ? `esforço ${(controls.effort ? t(`effort.${controls.effort}`) : t("composer.effortAuto")).toLowerCase()}` : null].filter(Boolean).join(" · ");
  const searching = query.trim().length > 0;
  return <QueryContext value={query}>
    <SheetScreen embedded title={t("phoneChat.model")} onClose={close} closeLabel={t("common.cancel")} bottomPadding={64}
      right={<RoundButton icon="checkmark" label={t("phoneChat.done")} onPress={apply} />}
      footer={<KeyboardStickyView offset={{ closed: 0, opened: Math.max(insets.bottom, 12) - 8 }} style={{ position: "absolute", left: 0, right: 0, bottom: 0 }}>
        <View className="mx-4 flex-row items-center gap-2 rounded-full border border-border-subtle bg-grouped-card px-4" style={{ marginBottom: Math.max(insets.bottom, 12), height: 46 }}>
          <SymbolView name="magnifyingglass" size={15} tintColorClassName="accent-foreground-muted" />
          <AppTextInput value={query} onChangeText={setQuery} placeholder={t("phoneChat.findModel")} autoCapitalize="none" autoCorrect={false} returnKeyType="search"
            className="min-h-0 flex-1 rounded-none border-0 bg-transparent px-0 py-0 text-base" />
          {query ? <Pressable onPress={() => setQuery("")} hitSlop={8}><SymbolView name="xmark.circle.fill" size={16} tintColorClassName="accent-foreground-muted" /></Pressable> : null}
        </View>
      </KeyboardStickyView>}>
      <SheetGroup className="mt-2">
        <SheetRow first bold lead={<ProviderIcon provider={draft.provider} size={20} />} title={summary.name ?? t("models.select")} note={note || definition.name} noteColor={accent}
          value={t("phoneChat.options")} chevron onPress={() => navigation.navigate("ThreadSettingsMain")} />
      </SheetGroup>
      {state.locked ? <AppText className="mt-2 px-3 text-sm text-foreground-muted">{t("phoneChat.modelLocked")}</AppText> : null}
      {providers.map((id) => <ProviderGroup key={id} provider={id} accent={accent} searching={searching}
        open={searching || (expanded[id] ?? id === draft.provider)} onToggle={() => setExpanded((current) => ({ ...current, [id]: !(current[id] ?? id === draft.provider) }))} />)}
    </SheetScreen>
  </QueryContext>;
}

function EffortPage() {
  const navigation = useNavigation();
  const { controls, setDraft, byProvider } = useDraftState();
  const pick = (next: ExecutionOptions) => {
    setDraft((current) => {
      const result = chooseExecution(current.provider, current.model, byProvider[current.provider]?.models ?? [], current.preference, next);
      return { ...current, model: result.model, preference: result.preference, save: result.save || (current.save && result.model === current.model) };
    });
    navigation.goBack();
  };
  return <Page title={t("phoneChat.reasoning")}>
    <ListSection>
      {controls.levels.map((level, index) => <ListChoiceRow key={level} label={t(`effort.${level}`)} selected={controls.effort === level} separated={index > 0}
        disabled={!controls.selectableLevels.includes(level)} onPress={() => pick({ effort: level })} />)}
    </ListSection>
    <AppText className="-mt-2 px-2 text-sm text-foreground-muted">{t("phoneChat.reasoningHint")}</AppText>
  </Page>;
}

function TierPage() {
  const navigation = useNavigation();
  const { controls, setDraft, byProvider } = useDraftState();
  const pick = (fast: boolean) => {
    setDraft((current) => {
      const result = chooseExecution(current.provider, current.model, byProvider[current.provider]?.models ?? [], current.preference, { fast });
      return { ...current, model: result.model, preference: result.preference, save: result.save || (current.save && result.model === current.model) };
    });
    navigation.goBack();
  };
  return <Page title={t("phoneChat.serviceTier")}>
    <ListSection>
      <ListChoiceRow label={t("phoneChat.standard")} selected={!controls.fast} onPress={() => pick(false)} />
      <ListChoiceRow label={t("phoneChat.fast")} selected={controls.fast} separated onPress={() => pick(true)} />
    </ListSection>
    <AppText className="-mt-2 px-2 text-sm text-foreground-muted">{t("phoneChat.fastHint")}</AppText>
  </Page>;
}

function AccessPage() {
  const navigation = useNavigation();
  const { definition, draft, approval, setDraft } = useDraftState();
  return <Page title={t("phoneChat.access")}>
    <ListSection>
      {approvalOptions(definition.approvalModes).map(({ mode, supported }, index) => {
        const description = !supported ? "composer.approvalUnsupported" : mode === "ask" ? "composer.askApprovalDescription" : mode === "auto" && draft.provider === "opencode" ? "composer.autoEditsDescription" : mode === "auto" ? "composer.autoReviewDescription" : "composer.fullAccessDescription";
        return <ListChoiceRow key={mode} label={t(APPROVAL_LABEL[mode])} description={t(description)} selected={supported && approval === mode} separated={index > 0} disabled={!supported}
          onPress={() => { setDraft((current) => ({ ...current, approval: mode })); navigation.goBack(); }} />;
      })}
    </ListSection>
    <AppText className="-mt-2 px-2 text-sm text-foreground-muted">{t("phoneChat.accessHint", { provider: definition.name })}</AppText>
  </Page>;
}
