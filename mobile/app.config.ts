import type { ExpoConfig } from "expo/config";

/**
 * Sirus's native iPhone app (ADR-135): React Native with
 * Expo, native navigation, Liquid Glass and SF Symbols. Signed with the owner's free Personal
 * Team, so no push, app groups or Sign in with Apple capabilities are requested.
 */

/**
 * Updates come from the owner's own computer (ADR-135 addendum), never from Expo's servers:
 * `SIRUS_UPDATE_URL` is that computer's `https://…ts.net/api/app-update/manifest`, set when
 * building. The app swaps the empty `Authorization` for its device token at launch. Bump
 * `RUNTIME_VERSION` whenever native code changes, so an update never runs on the wrong build.
 */
const UPDATE_URL = process.env.SIRUS_UPDATE_URL;
const RUNTIME_VERSION = "4";

const config: ExpoConfig = {
  name: "Sirus",
  slug: "sirus-mobile",
  platforms: ["ios"],
  scheme: "sirus",
  version: "0.1.0",
  runtimeVersion: RUNTIME_VERSION,
  updates: {
    enabled: Boolean(UPDATE_URL),
    url: UPDATE_URL,
    checkAutomatically: "ON_LOAD",
    fallbackToCacheTimeout: 0,
    requestHeaders: { Authorization: "" },
  },
  orientation: "portrait",
  icon: "./assets/icon.png",
  userInterfaceStyle: "automatic",
  ios: {
    bundleIdentifier: "com.siruscode.phone",
    appleTeamId: "47929H7K29",
    supportsTablet: false,
    infoPlist: {
      NSAppTransportSecurity: { NSAllowsArbitraryLoads: true },
      NSLocalNetworkUsageDescription: "O Sirus conecta aos seus computadores pela sua rede Tailscale.",
      NSMicrophoneUsageDescription: "O Sirus usa o microfone para ditar mensagens aos agentes.",
      NSSpeechRecognitionUsageDescription: "O Sirus transcreve o que você fala para escrever mensagens aos agentes.",
      ITSAppUsesNonExemptEncryption: false,
    },
  },
  plugins: [
    "expo-secure-store",
    "expo-web-browser",
    ["expo-speech-recognition", {
      microphonePermission: "O Sirus usa o microfone para ditar mensagens aos agentes.",
      speechRecognitionPermission: "O Sirus transcreve o que você fala para escrever mensagens aos agentes.",
    }],
    ["expo-image-picker", { photosPermission: "O Sirus anexa fotos às mensagens que você manda aos agentes.", cameraPermission: false, microphonePermission: false }],
    ["expo-splash-screen", { image: "./assets/splash.png", imageWidth: 309, backgroundColor: "#0f0f12" }],
    ["expo-build-properties", { ios: { deploymentTarget: "16.4" } }],
  ],
};

export default config;
