import { NavigationContainer } from "@react-navigation/native";
import { useEffect } from "react";
import { Alert, Linking } from "react-native";
import { GestureHandlerRootView } from "react-native-gesture-handler";
import { KeyboardProvider } from "react-native-keyboard-controller";
import { SafeAreaProvider } from "react-native-safe-area-context";

import "../global.css";
import { useSirus } from "./data/store";
import { LaunchOverlay } from "./components/LaunchOverlay";
import { NotificationBanners } from "./components/NotificationBanners";
import { navigationRef, syncBannerScreen } from "./data/navigation-ref";
import { checkForAppUpdate } from "./data/app-updates";
import { pairComputer, pairingFromLink } from "./data/computers";
import { AppearancePreferencesProvider } from "./features/settings/appearance/AppearancePreferencesProvider";
import { useMobileNavigationTheme } from "./lib/useMobileNavigationTheme";
import { createV5StackNavigator } from "./native/createV5StackNavigator";
import { NATIVE_LIQUID_GLASS_SUPPORTED } from "./native/native-glass";
import { nativeHeaderScrollEdgeEffects, type AppNativeStackNavigationOptions } from "./native/StackHeader";
import { Platform } from "react-native";
import { ConnectScreen } from "./screens/ConnectScreen";
import { SearchScreen } from "./screens/ConversationsScreen";
import { CostScreen } from "./screens/CostScreen";
import { MainTabs } from "./screens/MainTabs";
import { ProjectScreen } from "./screens/ProjectsScreen";
import { PullRequestScreen, PullRequestsScreen } from "./screens/PullRequestsScreen";
import { FileScreen, FilesScreen } from "./screens/FilesScreen";
import { DiffScreen, GitScreen } from "./screens/GitScreen";
import { NewThreadScreen } from "./screens/NewThreadScreen";
import {
  SettingsComputersScreen,
  SettingsComputerScreen,
  SettingsAddComputerScreen,
  SettingsAlertsScreen,
  SettingsAppearanceScreen,
  SettingsKeyboardScreen,
  SettingsOrganizationScreen,
  SettingsSnoozedScreen,
  SettingsNewConversationsScreen,
  SettingsSourceControlScreen,
  SettingsAgentBehaviorScreen,
  SettingsMaintenanceScreen,
  SettingsUsageScreen,
  SettingsAboutScreen,
  SettingsScreen,
} from "./screens/SettingsScreen";
import { TerminalScreen } from "./screens/TerminalScreen";
import { ThreadSettingsScreen } from "./screens/ThreadSettingsScreen";
import { ThreadScreen } from "./screens/ThreadScreen";

const Stack = createV5StackNavigator();

/** The glass header preset: transparent over the screen's scroll view on iOS 26+. */
const GLASS_HEADER: AppNativeStackNavigationOptions = {
  headerBackButtonDisplayMode: "minimal",
  headerBackTitle: "",
  headerLargeTitle: false,
  headerShadowVisible: false,
  headerShown: true,
  headerStyle: NATIVE_LIQUID_GLASS_SUPPORTED ? { backgroundColor: "transparent" } : undefined,
  headerTitleStyle: { fontSize: 18, fontWeight: "800" },
  headerTransparent: NATIVE_LIQUID_GLASS_SUPPORTED,
  scrollEdgeEffects: NATIVE_LIQUID_GLASS_SUPPORTED ? nativeHeaderScrollEdgeEffects(Platform.OS, Platform.Version) : undefined,
  unstable_navigationItemStyle: NATIVE_LIQUID_GLASS_SUPPORTED ? "editor" : undefined,
};

/** Form sheets: full height, grabber visible. */
const FORM_SHEET = { presentation: "formSheet", sheetAllowedDetents: [1], sheetGrabberVisible: true, headerShown: false, gestureEnabled: true } as const;

/** Sirus's iPhone app (ADR-135). */
export default function App() {
  useEffect(() => { void useSirus.getState().start().then(() => checkForAppUpdate()); }, []);
  // `sirus://pair?address=…&code=…` pairs at once (a computer's QR code can carry it).
  useEffect(() => {
    const pair = (url: string | null | undefined) => {
      if (!url?.startsWith("sirus://pair")) return;
      const found = pairingFromLink(url);
      if (!found) return;
      void pairComputer(found.address, found.code, found.access).then((computer) => useSirus.getState().addComputer(computer), (reason: unknown) => Alert.alert("Não foi possível conectar", reason instanceof Error ? reason.message : String(reason)));
    };
    void Linking.getInitialURL().then(pair);
    const subscription = Linking.addEventListener("url", (event) => pair(event.url));
    return () => subscription.remove();
  }, []);
  return <GestureHandlerRootView style={{ flex: 1 }}>
    <KeyboardProvider>
      <SafeAreaProvider>
        <AppearancePreferencesProvider>
          <Navigation />
          <NotificationBanners />
          <LaunchOverlay />
        </AppearancePreferencesProvider>
      </SafeAreaProvider>
    </KeyboardProvider>
  </GestureHandlerRootView>;
}

function Navigation() {
  const theme = useMobileNavigationTheme();
  const ready = useSirus((state) => state.ready);
  const paired = useSirus((state) => state.computers.length > 0);
  if (!ready) return null;
  return <NavigationContainer theme={theme} ref={navigationRef} onReady={syncBannerScreen} onStateChange={syncBannerScreen}>
    <Stack.Navigator screenOptions={GLASS_HEADER as never}>
      {paired ? <>
        {/* The main screen (top bar: Novo, Conversas, Painel; the Projetos drawer); everything else opens over it. */}
        <Stack.Screen name="Home" component={MainTabs} options={{ title: "Sirus", headerShown: false }} />
        <Stack.Screen name="Settings" component={SettingsScreen} options={{ headerShown: false }} />
        <Stack.Screen name="Cost" component={CostScreen} options={{ headerShown: false }} />
        <Stack.Screen name="Search" component={SearchScreen} />
        <Stack.Screen name="Project" component={ProjectScreen} />
        <Stack.Screen name="PullRequests" component={PullRequestsScreen} />
        <Stack.Screen name="PullRequest" component={PullRequestScreen} />
        <Stack.Screen name="Thread" component={ThreadScreen} />
        <Stack.Screen name="NewThread" component={NewThreadScreen} options={FORM_SHEET as never} />
        <Stack.Screen name="Terminal" component={TerminalScreen} />
        <Stack.Screen name="Files" component={FilesScreen} />
        <Stack.Screen name="File" component={FileScreen} />
        <Stack.Screen name="Git" component={GitScreen} options={{ headerShown: false }} />
        <Stack.Screen name="Diff" component={DiffScreen} />
        <Stack.Screen name="ThreadSettings" component={ThreadSettingsScreen} options={FORM_SHEET as never} />
        <Stack.Screen name="SettingsComputers" component={SettingsComputersScreen} />
        <Stack.Screen name="SettingsComputer" component={SettingsComputerScreen} />
        <Stack.Screen name="SettingsAddComputer" component={SettingsAddComputerScreen} />
        <Stack.Screen name="SettingsAlerts" component={SettingsAlertsScreen} />
        <Stack.Screen name="SettingsAppearance" component={SettingsAppearanceScreen} />
        <Stack.Screen name="SettingsKeyboard" component={SettingsKeyboardScreen} />
        <Stack.Screen name="SettingsOrganization" component={SettingsOrganizationScreen} />
        <Stack.Screen name="SettingsSnoozed" component={SettingsSnoozedScreen} />
        <Stack.Screen name="SettingsNewConversations" component={SettingsNewConversationsScreen} />
        <Stack.Screen name="SettingsSourceControl" component={SettingsSourceControlScreen} />
        <Stack.Screen name="SettingsAgentBehavior" component={SettingsAgentBehaviorScreen} />
        <Stack.Screen name="SettingsMaintenance" component={SettingsMaintenanceScreen} />
        <Stack.Screen name="SettingsUsage" component={SettingsUsageScreen} />
        <Stack.Screen name="SettingsAbout" component={SettingsAboutScreen} />
      </> : <Stack.Screen name="Connect" component={ConnectScreen} options={{ headerShown: false }} />}
    </Stack.Navigator>
  </NavigationContainer>;
}
