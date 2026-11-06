import { createNavigationContainerRef, type ParamListBase } from "@react-navigation/native";

import { setBannerScreen } from "./banners";

/** The root navigator, for what lives outside it (the banners). */
export const navigationRef = createNavigationContainerRef<ParamListBase>();

/** Tells the banners which conversation is on screen, so it never gets a banner of its own. */
export function syncBannerScreen() {
  const route = navigationRef.isReady() ? navigationRef.getCurrentRoute() : undefined;
  const params = route?.params as { computerId?: string; sessionId?: string } | undefined;
  setBannerScreen(route?.name === "Thread" && params?.computerId && params.sessionId ? { computerId: params.computerId, sessionId: params.sessionId } : null);
}
