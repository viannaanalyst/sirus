import { useNavigation, useRoute, type NavigationProp, type ParamListBase, type RouteProp } from "@react-navigation/native";
import * as Application from "expo-application";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { ActivityIndicator, Alert, Linking, Pressable, ScrollView, View } from "react-native";
import { useSafeAreaInsets } from "react-native-safe-area-context";
import { create } from "zustand";

import type { AppSettings, ProviderUsage, Session } from "@sirus/client/types";
import { translate } from "@sirus/i18n";
import { orderedUsageWindows, resetDuration, USAGE_PROVIDER_IDS, usageWindowLabel } from "@sirus/lib/provider-usage";
import { providerById } from "@sirus/lib/provider-registry";
import { APPROVAL_CHOICES, isProviderEnabled, STREAMING_CHOICES, WORKSPACE_CHOICES } from "@sirus/lib/settings";
import { isSnoozed, snoozeCountdown } from "@sirus/lib/snooze";

import type { AppSymbolName } from "../components/AppSymbol";
import { SymbolView } from "../components/AppSymbol";
import { AppText } from "../components/AppText";
import { ListChoiceRow } from "../components/ListChoiceRow";
import { ListRow } from "../components/ListRow";
import { ListRowLayout } from "../components/ListRowLayout";
import { ListSection } from "../components/ListSection";
import { ListToggleRow } from "../components/ListToggleRow";
import { ProviderIcon } from "../components/ProviderIcon";
import { SheetGroup, SheetRow, SheetScreen, SheetSection } from "../components/SheetKit";
import { computerName } from "../data/computers";
import { useSirus } from "../data/store";
import { NativeHeaderToolbar, NativeStackScreenOptions } from "../native/StackHeader";
import { CodeAppearanceSection } from "../features/settings/appearance/sections/CodeAppearanceSection";
import { TerminalAppearanceSection } from "../features/settings/appearance/sections/TerminalAppearanceSection";
import { TextAppearanceSection } from "../features/settings/appearance/sections/TextAppearanceSection";
import { ThemeAppearanceSection } from "../features/settings/appearance/sections/ThemeAppearanceSection";
import { createNativeMailSearchToolbarItem } from "../features/layout/native-mail-search-toolbar";
import { checkForAppUpdate, runningUpdateLabel } from "../data/app-updates";
import { ConnectScreen } from "./ConnectScreen";

const t = (key: string, params?: Record<string, string | number>) => translate("pt-BR", key, params);
const icon = (name: string) => name as AppSymbolName;

/** The computer the Mac settings pages change, when there are several. */
const useSettingsComputer = create<{ id: string | null; set: (id: string) => void }>()((set) => ({ id: null, set: (id) => set({ id }) }));

/**
 * One settings page: the sheet's color, a scroll view under the glass header,
 * sections of grouped rows.
 */
function Page(props: { readonly title: string; readonly children: ReactNode }) {
  const insets = useSafeAreaInsets();
  return <View collapsable={false} className="flex-1 bg-screen">
    <NativeStackScreenOptions options={{ title: props.title }} />
    <ScrollView contentInsetAdjustmentBehavior="automatic" showsVerticalScrollIndicator={false} className="flex-1"
      contentContainerClassName="gap-5 px-5 pt-4" contentContainerStyle={{ paddingBottom: Math.max(insets.bottom, 18) + 18 }}>
      {props.children}
    </ScrollView>
  </View>;
}

function Note(props: { readonly children: string }) {
  return <AppText className="-mt-2 px-2 text-sm text-foreground-muted">{props.children}</AppText>;
}

/**
 * Ajustes (ADR-135) in the sheet of the approved design: the computer's status row on top, then
 * grouped rows (icon, value, chevron) that open each settings page over the stack, Reconectar and
 * Desconectar at the end. Every page the old list reached is still one row away.
 */
export function SettingsScreen() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const computers = useSirus((state) => state.computers);
  const connection = useSirus((state) => state.connection);
  const data = useSirus((state) => state.data);
  const prefs = useSirus((state) => state.prefs);
  const snoozed = useMemo(() => Object.values(data).reduce((sum, value) => sum + value.sessions.filter((session) => !session.sideChat && !session.pullAsk && isSnoozed(session)).length, 0), [data]);
  const online = computers.filter((computer) => connection[computer.id] === "open").length;
  const go = (screen: string) => () => navigation.navigate(screen);
  const first = computers[0];
  const status = !computers.length ? "Nenhum computador" : online === computers.length ? "Conectado" : online ? `${online} de ${computers.length} conectados` : connection[first!.id] === "lost" ? "Fora de alcance" : "Conectando…";
  const reconnect = () => { for (const computer of computers) void useSirus.getState().addComputer(computer); };
  const disconnect = () => {
    if (computers.length !== 1 || !first) { navigation.navigate("SettingsComputers"); return; }
    Alert.alert(`Desconectar de ${computerName(first)}?`, "Este iPhone deixa de ver as conversas dele. Para voltar, pareie de novo com um código.", [
      { text: "Cancelar", style: "cancel" },
      { text: "Desconectar", style: "destructive", onPress: () => { void useSirus.getState().forgetComputer(first.id); } },
    ]);
  };
  return <SheetScreen title="Ajustes" onClose={() => navigation.goBack()}>
    <SheetGroup className="mt-2">
      <SheetRow icon="desktopcomputer" title={first ? (computers.length > 1 ? `${computerName(first)} +${computers.length - 1}` : computerName(first)) : "Computador"} note={`${status} · Sirus ${Application.nativeApplicationVersion ?? ""}`.trim()}
        accessory={<View className={`size-2.5 rounded-full ${online ? "bg-adaptive-emerald-600-400" : "bg-warning"}`} />} onPress={go("SettingsComputers")} first />
    </SheetGroup>
    <SheetSection>{t("mobile.connections")}</SheetSection>
    <SheetGroup>
      <SheetRow first icon="desktopcomputer" title="Computadores" value={String(computers.length)} chevron onPress={go("SettingsComputers")} />
      <SheetRow icon="bell" title={t("mobile.alerts")} chevron onPress={go("SettingsAlerts")} />
    </SheetGroup>
    <SheetSection>{t("mobile.interface")}</SheetSection>
    <SheetGroup>
      <SheetRow first icon="paintbrush" title={t("mobile.appearance")} value={t(`mobile.scheme.${prefs.colorScheme}`)} chevron onPress={go("SettingsAppearance")} />
      <SheetRow icon="keyboard" title={t("mobile.keyboard")} chevron onPress={go("SettingsKeyboard")} />
    </SheetGroup>
    <SheetSection>{t("mobile.projectsAndConversations")}</SheetSection>
    <SheetGroup>
      <SheetRow first icon="folder" title={t("mobile.organization")} chevron onPress={go("SettingsOrganization")} />
      <SheetRow icon="moon" title="Conversas adiadas" value={String(snoozed)} chevron onPress={go("SettingsSnoozed")} />
    </SheetGroup>
    <SheetSection>{t("phoneSettings.macSettings")}</SheetSection>
    <SheetGroup>
      <SheetRow first icon="plus.bubble" title={t("phoneSettings.newConversations")} chevron onPress={go("SettingsNewConversations")} />
      <SheetRow icon="arrow.triangle.branch" title={t("phoneSettings.sourceControl")} chevron onPress={go("SettingsSourceControl")} />
      <SheetRow icon="sparkles" title="Comportamento do agente" chevron onPress={go("SettingsAgentBehavior")} />
      <SheetRow icon="wrench.and.screwdriver" title={t("phoneSettings.maintenance")} chevron onPress={go("SettingsMaintenance")} />
    </SheetGroup>
    <AppText className="mt-1.5 px-3 text-xs leading-4 text-foreground-tertiary">{t("mobile.settingsHere")}</AppText>
    <SheetSection>{t("mobile.app")}</SheetSection>
    <SheetGroup>
      <SheetRow first icon="chart.bar" title={t("mobile.usage")} chevron onPress={go("SettingsUsage")} />
      <SheetRow icon="info.circle" title={t("mobile.about")} value={Application.nativeApplicationVersion ?? undefined} chevron onPress={go("SettingsAbout")} />
    </SheetGroup>
    <SheetSection>Espaço de trabalho</SheetSection>
    <SheetGroup>
      <SheetRow first icon="arrow.clockwise" title="Reconectar" disabled={!computers.length} onPress={reconnect} />
    </SheetGroup>
    <SheetGroup className="mt-3">
      <SheetRow first destructive icon="xmark" title="Desconectar do Mac" disabled={!computers.length} onPress={disconnect} />
    </SheetGroup>
  </SheetScreen>;
}

export function SettingsComputersScreen() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const computers = useSirus((state) => state.computers);
  return <Page title="Computadores">
    <NativeHeaderToolbar placement="right">
      <NativeHeaderToolbar.Button accessibilityLabel="Conectar outro computador" icon="plus" onPress={() => navigation.navigate("SettingsAddComputer")} />
    </NativeHeaderToolbar>
    <ListSection>
      {computers.map((computer, index) => <ComputerRow key={computer.id} computerId={computer.id} separated={index > 0} onPress={() => navigation.navigate("SettingsComputer", { computerId: computer.id })} />)}
    </ListSection>
    <Note>Um Mac ou um servidor com o Sirus, alcançado pela sua rede Tailscale. As conversas de todos aparecem juntas no início.</Note>
  </Page>;
}

/** A computer row: status dot, kind, name; address and state below. */
function ComputerRow(props: { readonly computerId: string; readonly separated?: boolean; readonly onPress: () => void }) {
  const computer = useSirus((state) => state.computers.find((item) => item.id === props.computerId));
  const state = useSirus((store) => store.connection[props.computerId]);
  if (!computer) return null;
  const label = state === "open" ? "Conectado" : state === "lost" ? "Fora de alcance" : "Conectando…";
  return <Pressable onPress={props.onPress} className={`bg-grouped-card active:opacity-70 ${props.separated ? "border-t border-border-subtle" : ""}`}>
    <View className="flex-row items-center gap-3 px-4 py-3.5">
      <View className="min-w-0 flex-1 gap-0.5">
        <View className="flex-row items-center gap-2">
          <View className={`size-2 rounded-full ${state === "open" ? "bg-adaptive-emerald-600-400" : state === "lost" ? "bg-danger-foreground" : "bg-warning"}`} />
          <SymbolView name={(computer.kind === "server" ? "server.rack" : "laptopcomputer") as AppSymbolName} size={13} tintColorClassName="accent-foreground-muted" />
          <AppText numberOfLines={1} className="shrink font-app-bold text-base">{computerName(computer)}</AppText>
        </View>
        <AppText numberOfLines={1} className="text-sm text-foreground-muted">{computer.baseUrl}</AppText>
        <AppText className="text-sm text-foreground-muted">{label}</AppText>
      </View>
      <SymbolView name="chevron.right" size={14} weight="semibold" tintColorClassName="accent-chevron" />
    </View>
  </Pressable>;
}

/** One computer: its state, address and conversations, and forgetting it. */
export function SettingsComputerScreen() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const { computerId } = useRoute<RouteProp<{ SettingsComputer: { computerId: string } }, "SettingsComputer">>().params;
  const computer = useSirus((state) => state.computers.find((item) => item.id === computerId));
  const state = useSirus((store) => store.connection[computerId]);
  const data = useSirus((store) => store.data[computerId]);
  if (!computer) return <Page title="Computador"><Note>Este computador foi esquecido.</Note></Page>;
  const forget = () => Alert.alert(`Esquecer ${computerName(computer)}?`, "Este iPhone deixa de ver as conversas dele. Para voltar, pareie de novo com um código.", [
    { text: "Cancelar", style: "cancel" },
    { text: "Esquecer", style: "destructive", onPress: () => { void useSirus.getState().forgetComputer(computerId); navigation.goBack(); } },
  ]);
  return <Page title={computerName(computer)}>
    <ListSection>
      <ListRowLayout icon={icon("circle.fill")} label="Estado" detail={state === "open" ? "Conectado" : state === "lost" ? "Fora de alcance" : "Conectando…"} />
      <ListRowLayout icon={icon(computer.kind === "server" ? "server.rack" : "laptopcomputer")} label="Tipo" detail={computer.kind === "server" ? "Servidor" : "Mac"} />
      <ListRowLayout icon={icon("link")} label="Endereço" description={computer.baseUrl} />
      <ListRowLayout icon={icon("bubble.left.and.bubble.right")} label="Conversas" detail={data ? String(data.sessions.filter((session) => !session.sideChat && !session.pullAsk).length) : "—"} />
      <ListRowLayout icon={icon("folder")} label="Projetos" detail={data ? String(data.projects.length) : "—"} />
    </ListSection>
    <ListSection>
      <Pressable onPress={forget} className="bg-grouped-card px-4 py-3.5 active:opacity-70">
        <AppText className="text-base text-danger-foreground">Esquecer este computador</AppText>
      </Pressable>
    </ListSection>
  </Page>;
}

export function SettingsAddComputerScreen() {
  const navigation = useNavigation();
  return <View className="flex-1 bg-sheet">
    <NativeStackScreenOptions options={{ title: "" }} />
    <ConnectScreen title="Conectar outro computador" onDone={() => navigation.goBack()} />
  </View>;
}

export function SettingsAlertsScreen() {
  return <Page title="Notificações">
    <ListSection title={t("mobile.agentActivity")}>
      <ListToggleRow icon={icon("bell.badge")} label="Notificações do aparelho" value={false} disabled onValueChange={() => {}} />
      <ListToggleRow icon={icon("bolt.circle")} label="Atualizações ao vivo" value={false} disabled onValueChange={() => {}} />
    </ListSection>
    <Note>Notificações e Atividades ao Vivo precisam da conta paga da Apple. Enquanto isso, os alertas chegam pelo Sirus da Tela de Início (aberto pelo Safari).</Note>
  </Page>;
}

/** The appearance page: color scheme and themes with previews, text, terminal and code sizes. */
export function SettingsAppearanceScreen() {
  return <Page title={t("mobile.appearance")}>
    <ThemeAppearanceSection />
    <TextAppearanceSection />
    <TerminalAppearanceSection />
    <CodeAppearanceSection />
  </Page>;
}

export function SettingsKeyboardScreen() {
  const returnKey = useSirus((state) => state.prefs.returnKey);
  return <Page title={t("mobile.keyboard")}>
    <ListSection title={t("mobile.returnKey")}>
      {([["send", "mobile.return.send", "Return envia a mensagem."], ["newline", "mobile.return.newline", "Return adiciona uma linha. Envie pelo botão."]] as const).map(([id, label, help], index) =>
        <ListChoiceRow key={id} label={t(label)} description={help} selected={returnKey === id} separated={index > 0} onPress={() => useSirus.getState().setPrefs({ returnKey: id })} />)}
    </ListSection>
  </Page>;
}

export function SettingsOrganizationScreen() {
  const order = useSirus((state) => state.prefs.threadOrder);
  return <Page title={t("mobile.organization")}>
    <ListSection title="Ordem das conversas">
      {(["updated_at", "created_at"] as const).map((id, index) => <ListChoiceRow key={id} label={t(`phoneHome.arrange.${id}`)} selected={order === id} separated={index > 0}
        onPress={() => useSirus.getState().setPrefs({ threadOrder: id })} />)}
    </ListSection>
    <Note>As fixadas ficam sempre no topo.</Note>
  </Page>;
}

/** Snoozed conversations of every computer, with their way back and delete. */
function ConversationList() {
  const data = useSirus((state) => state.data);
  const computers = useSirus((state) => state.computers);
  const [query, setQuery] = useState("");
  const [computerFilter, setComputerFilter] = useState<string | null>(null);
  const all = useMemo(() => {
    const list: { computerId: string; session: Session; project?: string }[] = [];
    for (const [computerId, value] of Object.entries(data)) {
      const projects = new Map(value.projects.map((project) => [project.id, project.name]));
      for (const session of value.sessions) {
        if (session.sideChat || session.pullAsk) continue;
        if (isSnoozed(session)) list.push({ computerId, session, project: projects.get(session.projectId) });
      }
    }
    return list.sort((a, b) => b.session.lastActivityAt.localeCompare(a.session.lastActivityAt));
  }, [data]);
  const rows = useMemo(() => {
    const needle = query.trim().toLocaleLowerCase();
    return all.filter((row) => (!computerFilter || row.computerId === computerFilter)
      && (!needle || row.session.title.toLocaleLowerCase().includes(needle) || (row.project ?? "").toLocaleLowerCase().includes(needle)));
  }, [all, query, computerFilter]);
  const filterMenu = useMemo(() => ({
    title: "Computador",
    items: [
      { type: "action" as const, title: "Todos os computadores", state: computerFilter === null ? "on" as const : "off" as const, onPress: () => setComputerFilter(null) },
      ...computers.map((computer) => ({ type: "action" as const, title: computerName(computer), state: computerFilter === computer.id ? "on" as const : "off" as const, onPress: () => setComputerFilter(computer.id) })),
    ],
  }), [computers, computerFilter]);
  const several = computers.length > 1;
  const act = (computerId: string, session: Session) => {
    const run = (action: Parameters<ReturnType<typeof useSirus.getState>["rowAction"]>[2]) =>
      void useSirus.getState().rowAction(computerId, session.id, action).catch((reason: unknown) => Alert.alert(session.title, reason instanceof Error ? reason.message : String(reason)));
    Alert.alert(session.title || "Conversa", undefined, [
      { text: t("snooze.return"), onPress: () => run({ type: "snooze", until: null }) },
      { text: t("mobile.delete"), style: "destructive", onPress: () => Alert.alert("Excluir a conversa?", "Não dá para desfazer.", [
        { text: "Cancelar", style: "cancel" },
        { text: "Excluir", style: "destructive", onPress: () => run({ type: "delete", removeWorktree: false }) },
      ]) },
      { text: "Cancelar", style: "cancel" },
    ]);
  };
  return <Page title="Conversas adiadas">
    {/* The list: filter and search in the bottom glass toolbar. */}
    <NativeStackScreenOptions optionsVersion={[filterMenu.items, computerFilter]} options={{
      headerSearchBarOptions: { autoCapitalize: "none", onCancelButtonPress: () => setQuery("") },
      unstable_headerToolbarItems: () => [createNativeMailSearchToolbarItem({
        filterMenu, filterButtonId: "snoozed-filter", filterSystemImageName: computerFilter ? "line.3.horizontal.decrease.circle.fill" : "line.3.horizontal.decrease.circle",
        onSearchTextChange: setQuery, placeholder: "Buscar", searchTextChangeId: "snoozed-search-text", showsSearchDismissButton: true,
      } as never)],
    }} />
    {rows.length ? <ListSection>
      {rows.map((row) => <Pressable key={`${row.computerId}:${row.session.id}`} onPress={() => act(row.computerId, row.session)} className="bg-grouped-card active:opacity-70">
        <View className="flex-row items-center gap-3 px-4 py-3">
          <ProviderIcon provider={row.session.agent} size={16} />
          <View className="min-w-0 flex-1">
            <AppText numberOfLines={1} className="text-base">{row.session.title || "Conversa"}</AppText>
            <AppText numberOfLines={1} className="text-xs text-foreground-muted">
              {[row.project, several ? computerName(computers.find((computer) => computer.id === row.computerId) ?? { hostName: "", kind: "mac" }) : null,
                row.session.snoozedUntil ? snoozeCountdown(row.session.snoozedUntil, new Date(), t) : null].filter(Boolean).join(" · ")}
            </AppText>
          </View>
        </View>
      </Pressable>)}
    </ListSection> : <AppText className="mt-10 text-center text-sm text-foreground-muted">{query || computerFilter ? "Nada encontrado." : "Nenhuma conversa adiada."}</AppText>}
    <View className="h-16" />
  </Page>;
}
export const SettingsSnoozedScreen = ConversationList;

/**
 * A computer's own settings page (ADR-130): the chosen computer (when there are several), then
 * its settings, saved on it through `save_settings`. Nothing changes while it is out of reach.
 */
function MacPage(props: { readonly title: string; readonly children: (settings: AppSettings, save: (patch: Partial<AppSettings>) => void, disabled: boolean, server: boolean) => ReactNode }) {
  const computers = useSirus((state) => state.computers);
  const chosen = useSettingsComputer((state) => state.id);
  const computer = computers.find((item) => item.id === chosen) ?? computers[0];
  const settings = useSirus((state) => computer ? state.data[computer.id]?.settings ?? null : null);
  const online = useSirus((state) => computer ? state.connection[computer.id] === "open" : false);
  const disabled = !online || !settings;
  const save = (patch: Partial<AppSettings>) => {
    if (disabled || !computer) return;
    void useSirus.getState().saveSettings(computer.id, patch).catch((reason: unknown) => Alert.alert(props.title, reason instanceof Error ? reason.message : String(reason)));
  };
  return <Page title={props.title}>
    {computers.length > 1 ? <NativeHeaderToolbar placement="right">
      <NativeHeaderToolbar.Menu accessibilityLabel="Computador" icon="line.3.horizontal.decrease" title="Computador">
        {computers.map((item) => <NativeHeaderToolbar.MenuAction key={item.id} isOn={item.id === computer?.id} onPress={() => useSettingsComputer.getState().set(item.id)}>
          <NativeHeaderToolbar.Label>{computerName(item)}</NativeHeaderToolbar.Label>
        </NativeHeaderToolbar.MenuAction>)}
      </NativeHeaderToolbar.Menu>
    </NativeHeaderToolbar> : null}
    {settings ? props.children(settings, save, disabled, computer?.kind === "server") : <ActivityIndicator className="mt-10" />}
    {online ? null : <Note>{t("phoneSettings.offline")}</Note>}
  </Page>;
}

export function SettingsNewConversationsScreen() {
  return <MacPage title={t("phoneSettings.newConversations")}>{(settings, save, disabled) => <>
    <ListSection title="Espaço de trabalho padrão">
      {WORKSPACE_CHOICES.map((choice, index) => <ListChoiceRow key={choice.mode} label={t(choice.label)} description={t(choice.help)} selected={settings.defaultSessionWorkspace === choice.mode}
        separated={index > 0} disabled={disabled} onPress={() => save({ defaultSessionWorkspace: choice.mode })} />)}
    </ListSection>
    <ListSection title={t("settings.defaultApproval")}>
      {APPROVAL_CHOICES.map((choice, index) => <ListChoiceRow key={choice.mode} label={t(choice.label)} description={t(choice.help)} selected={settings.defaultApproval === choice.mode}
        separated={index > 0} disabled={disabled} onPress={() => save({ defaultApproval: choice.mode })} />)}
    </ListSection>
  </>}</MacPage>;
}

export function SettingsSourceControlScreen() {
  return <MacPage title={t("phoneSettings.sourceControl")}>{(settings, save, disabled) => <>
    <ListSection title={t("phoneSettings.defaultBranch")}>
      <ListToggleRow icon={icon("arrow.down.circle")} label={t("settings.autoPull")} description={t("settings.autoPullHelp")} value={settings.gitAutoPull} disabled={disabled} onValueChange={(gitAutoPull) => save({ gitAutoPull })} />
    </ListSection>
    <ListSection title={t("phoneSettings.worktrees")}>
      <ListToggleRow icon={icon("arrow.triangle.branch")} label={t("settings.fromOrigin")} description={t("settings.fromOriginHelp")} value={settings.worktreesFromOrigin} disabled={disabled} onValueChange={(worktreesFromOrigin) => save({ worktreesFromOrigin })} />
    </ListSection>
  </>}</MacPage>;
}

export function SettingsAgentBehaviorScreen() {
  return <MacPage title="Comportamento do agente">{(settings, save, disabled, server) => <>
    <ListSection title={t("settings.responseStreaming")}>
      {STREAMING_CHOICES.map((choice, index) => <ListChoiceRow key={choice.mode} label={t(choice.label)} description={t(choice.help)} selected={settings.responseStreaming === choice.mode}
        separated={index > 0} disabled={disabled} onPress={() => save({ responseStreaming: choice.mode })} />)}
    </ListSection>
    <Note>{t("settings.responseStreamingHelp")}</Note>
    {server ? null : <ListSection title={t("phoneSettings.browser")}>
      <ListToggleRow icon={icon("globe")} label={t("settings.browserAccess")} description={t("settings.browserAccessHelp")} value={settings.agentBrowserAccess} disabled={disabled} onValueChange={(agentBrowserAccess) => save({ agentBrowserAccess })} />
    </ListSection>}
  </>}</MacPage>;
}

export function SettingsMaintenanceScreen() {
  const navigation = useNavigation<NavigationProp<ParamListBase>>();
  const computers = useSirus((state) => state.computers);
  return <MacPage title={t("phoneSettings.maintenance")}>{(settings, save, disabled) => <>
  <ListSection title="Gerenciar computadores">
    {computers.map((computer) => <ListRow key={computer.id} icon={icon(computer.kind === "server" ? "server.rack" : "desktopcomputer")} label={computerName(computer)}
      detail={computer.kind === "server" ? "Servidor" : "Mac"} onPress={() => navigation.navigate("SettingsComputer", { computerId: computer.id })} />)}
  </ListSection>
  <ListSection title={t("phoneSettings.updates")}>
    <ListToggleRow icon={icon("arrow.clockwise")} label={t("Check for CLI updates")} description={t("Check Codex, Claude and other provider CLIs for newer versions in the background.")} value={settings.enableProviderUpdateChecks} disabled={disabled}
      onValueChange={(enableProviderUpdateChecks) => save({ enableProviderUpdateChecks })} />
    <ListToggleRow icon={icon("arrow.uturn.backward")} label={t("chatBehavior.continueInterrupted")} description={t("chatBehavior.continueInterruptedHelp")} value={settings.continueInterruptedSessions} disabled={disabled}
      onValueChange={(continueInterruptedSessions) => save({ continueInterruptedSessions })} />
  </ListSection>
  </>}</MacPage>;
}

/** Usage and limits of each provider on each computer (`provider_usage`), as the web app's. */
export function SettingsUsageScreen() {
  const computers = useSirus((state) => state.computers);
  const data = useSirus((state) => state.data);
  const agents = useSirus((state) => state.agents);
  const [usage, setUsage] = useState<Record<string, ProviderUsage | "loading" | "error">>({});
  const [now] = useState(() => Date.now());
  const entries = useMemo(() => computers.flatMap((computer) => {
    const settings = data[computer.id]?.settings;
    if (!settings) return [];
    return USAGE_PROVIDER_IDS
      .filter((id) => (agents[computer.id] ?? []).some((agent) => agent.id === id && agent.installed) && isProviderEnabled(settings, id))
      .map((provider) => ({ computer, provider, key: `${computer.id}:${provider}` }));
  }), [agents, computers, data]);
  useEffect(() => { for (const computer of computers) if (!agents[computer.id]) void useSirus.getState().loadAgents(computer.id).catch(() => undefined); }, [agents, computers]);
  const keys = entries.map((entry) => entry.key).join(",");
  useEffect(() => {
    for (const entry of entries) {
      setUsage((current) => current[entry.key] ? current : { ...current, [entry.key]: "loading" });
      useSirus.getState().invoke<ProviderUsage>(entry.computer.id, "provider_usage", { provider: entry.provider, refresh: false })
        .then((value) => setUsage((current) => ({ ...current, [entry.key]: value })), () => setUsage((current) => ({ ...current, [entry.key]: "error" })));
    }
  }, [keys]); // once per set of providers
  const several = computers.length > 1;
  return <Page title={t("mobile.usage")}>
    {!entries.length ? <ActivityIndicator className="mt-10" /> : entries.map((entry) => {
      const value = usage[entry.key];
      const windows = typeof value === "object" ? orderedUsageWindows(value).filter((window) => window.usedPercent !== null) : [];
      return <ListSection key={entry.key} title={`${providerById(entry.provider).name}${several ? ` · ${computerName(entry.computer)}` : ""}`}
        titleIcon={<ProviderIcon provider={entry.provider} size={14} />}>
        <View className="gap-4 bg-grouped-card px-4 py-4">
          {typeof value === "object" && value.account?.plan ? <AppText className="text-sm text-foreground-muted">Plano {value.account.plan}</AppText> : null}
          {windows.length ? windows.map((window) => {
            const used = Math.max(0, Math.min(100, Math.round(window.usedPercent ?? 0)));
            const reset = resetDuration(window.resetsAt, now);
            return <View key={window.id} className="gap-1.5">
              <View className="flex-row justify-between"><AppText className="text-base">{t(usageWindowLabel(window))}</AppText><AppText className="text-base text-foreground-muted">{used}%</AppText></View>
              <View className="h-1.5 overflow-hidden rounded-full bg-subtle-strong">
                <View className={`h-full rounded-full ${used >= 90 ? "bg-danger-foreground" : used >= 70 ? "bg-warning" : "bg-foreground"}`} style={{ width: `${used}%` }} />
              </View>
              {reset ? <AppText className="text-xs text-foreground-muted">{t("mobile.resetsIn", { time: reset })}</AppText> : null}
            </View>;
          }) : <AppText className="text-sm text-foreground-muted">{value === "loading" || value === undefined ? "Carregando…" : typeof value === "object" && value.note ? value.note : t("mobile.usageUnavailable")}</AppText>}
        </View>
      </ListSection>;
    })}
  </Page>;
}

export function SettingsAboutScreen() {
  return <Page title={t("mobile.about")}>
    <ListSection>
      <ListRowLayout icon={icon("app.badge")} label="Sirus para iPhone" detail={`${Application.nativeApplicationVersion ?? "?"} (${Application.nativeBuildVersion ?? "?"})`} />
      <ListRowLayout icon={icon("arrow.down.app")} label="Atualização em uso" detail={runningUpdateLabel()} />
      <ListRow icon={icon("arrow.clockwise")} label="Procurar atualização" onPress={() => void checkForAppUpdate(true).then((result) => {
        if (result === "none") Alert.alert("Atualização", "Você já está na versão mais nova.");
        else if (result === "unavailable") Alert.alert("Atualização", "Não foi possível procurar agora. O computador que publica as atualizações precisa estar conectado.");
      })} />
      <ListRow icon={icon("chevron.left.forwardslash.chevron.right")} label="Código do Sirus" detail="GitHub" onPress={() => void Linking.openURL("https://github.com/viannaanalyst/sirus")} />
    </ListSection>
    <ListSection>
      <ListRow icon={icon("doc.text")} label="Licenças de código aberto" onPress={() => void Linking.openURL("https://github.com/viannaanalyst/sirus/blob/main/THIRD-PARTY-NOTICES.md")} />
    </ListSection>
  </Page>;
}
