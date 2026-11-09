# ADR-135: The iPhone app in React Native

**Status:** Accepted (2026-10-14). Replaced the Capacitor shell of ADR-134, removed on 2026-10-15.

## Context

The Capacitor shell (ADR-134) runs the phone web UI in a web view. Native touches were added over it (Liquid Glass overlays, menus from glass buttons, push transitions), but it still reads as a web page: the controls, inputs, icons and window animations differ from iOS. The owner asked for a truly native app: React Native with UIKit navigation, SF Symbols, Liquid Glass and native menus.

## Decision

- **A React Native app in `mobile/`**, an Expo 58 project (React Native 0.88, React 19.3), bundle id `com.siruscode.phone`, signed with the owner's Personal Team like ADR-134. `mobile/ios` is generated (`expo prebuild`) and not committed.
- **A native layer adapted from MIT-licensed code** (notices in `THIRD-PARTY-NOTICES.md`):
  - its forks of `react-native-screens` and `react-native-enriched-markdown` (`mobile/deps`);
  - the V5 native stack with the glass header, header toolbar items and the Mail-style bottom search toolbar;
  - its components (`AppSymbol`, `GlassSurface`, `ControlPillMenu`, list rows) and theme files (uniwind).
  - Patches are applied by `scripts/apply-patches.mjs` on `postinstall`.
- **The shared client, not a second one.** Metro maps `@sirus/*` (and `@/*` inside it) to `../src`, so the app uses:
  - `RemoteTransport` and pairing from `src/client`;
  - `applyAgentOutput`, `timelineParts` and the step sentences from `src/lib`;
  - the Portuguese strings from `src/i18n`.
  React Native's `fetch` and WebSocket send no `Origin`, so the servers accept the app without changes.
- **Every computer at once** (`mobile/src/data/store.ts`). Paired computers and their tokens live in the iPhone keychain (`expo-secure-store`). Each one keeps a socket. Their conversations merge into one list, newest first, as on the phone web UI (ADR-132).
- **Screens:**
  - **Connect:** address and code; the `sirus://pair?address=&code=` link also pairs.
  - **Home:** every conversation; computer filter, search and compose in the bottom toolbar.
  - **Conversation:**
    - header with terminal, files and Git;
    - replies in native markdown;
    - steps grouped into sentences and folded into "Trabalhou por …" once the turn ends;
    - glass composer.
  - **New conversation sheet:** "O que vamos construir em <projeto>?", agent and model menu, worktree or project folder.
  - Files, Git (changes, diff, commit, push, pull), the native terminal, settings, attachments and dictation are in; the Capacitor shell is gone.
- **The JavaScript ships in the app**, and updates come from the owner's computer (see the addendum).

## Consequences

- The app looks and moves like iOS: real navigation stack and swipe back, glass, SF Symbols, native menus, keyboard handling and text selection.
- Two phone UIs exist until the native one covers the web one. The web one stays as the home-screen app for push alerts (ADR-082), which the free Apple account cannot give a native app.
- Debug and Release builds use React Native, Hermes and Expo prebuilt binaries in their own flavor. Switching flavors can leave one in the wrong flavor; reinstalling the pods restores them.

## Addendum (2026-10-15): updates and the 7-day signature

- **Updates from the owner's computer.** The app embeds `expo-updates` with no Expo service: `SIRUS_UPDATE_URL` (set when building, in the untracked `mobile/.env.local`) is one computer's `https://…ts.net/api/app-update/manifest`, and `RUNTIME_VERSION` in `app.config.ts` names the native build. `npm run publish:update` exports the JavaScript and assets into `<data dir>/app-updates/ios/<runtime>/` with a protocol-v1 `manifest.json`, swapped in whole. `remote.rs` serves the manifest (204 when the app already runs it) and the files; both need a device token, which the app puts in the `Authorization` header declared empty in the build (`setUpdateRequestHeadersOverride`, so Expo's anti-bricking checks stay on). The app checks at launch and from Settings → About and offers a restart when an update arrived. Native changes bump `RUNTIME_VERSION` and need a new install.
- **The signature renews from the Mac.** SideStore was tried first; on 2026-10-14/15 Apple began refusing its on-device sign-in (error -80045, also in other sideloading tools), while signing on the Mac kept working. So `mobile/scripts/renew-iphone.sh`, run every 3 hours by the LaunchAgent `com.siruscode.renew-iphone`, rebuilds the Release app (fresh Personal Team profile) and reinstalls it with `devicectl` once the last install is three days old, whenever the iPhone is reachable on the same Wi-Fi or the cable. Logs: `~/Library/Logs/sirus-iphone-renew.log`.

## Addendum (2026-10-16): status, queue, requests and pages

- **Home rows** show a status with a running clock: Trabalhando, Aguardando (the reply ended while background subagents or shell commands still run), Aprovação, Pergunta and Falhou. A delegated session or team worker waiting on the person lights its parent's row too.
- **The conversation** shows the agent's approvals and questions as the desktop's request cards (`respond_agent_request`). A message sent while the agent works joins a queue above the composer, kept while the app runs, with "Instruir agora" (`steer_turn`) for Claude and Codex; it goes when the reply ends.
- **Links:** web addresses in sent messages are blue; `/commands` and `@files` are highlighted; inline code naming a file of the conversation's folder is a blue link that opens the file.
- **Pages:** a conversation opens with its newest 40 light messages and loads older ones as the list nears the top (ADR-048 addendum: 3.4 MB → 192 KB, still after 4.6 s → 1.6 s).
- **Native modules** added since runtime 1 (`@expo/ui` for the native slider, enriched markdown 1.2.0-sirus.13 with the code-block wrap button) made it runtime 2.

## Addendum (2026-10-16): tabs, projects and pull requests

- **Native tabs.** The main screen is iOS's own tab bar (`Tabs` from react-native-screens, the iOS 26 tab bar) with Início, Conversas, Projetos and Ajustes, and the detached search button that opens a search over every conversation. A conversation, a project's page, a pull request or a settings page opens over the tabs in the main stack, so the tab bar stays on the lists only. Each tab mounts the first time it is chosen and stays mounted; only the chosen tab writes the shared header (`NativeWorkspaceTabActivityContext`).
- **Início** shows four cards (waiting on you, working, ready today, projects) that open Conversas filtered or Projetos, the day in one sentence with the last 12 hours' activity, the approvals and questions answerable in place, and the latest conversations. The Início tab carries a badge with what waits on the person.
- **Conversas** is the former home list with a native segmented control (Todas, Atenção, Trabalhando, Prontas) and the computer and project filters in the header menu. "Prontas" means finished in the last 24 hours.
- **Projetos** lists each computer's projects with the branch, uncommitted lines (`project_diff_stats`), agents working, approvals and open pull requests. A project's page has the counters, the project's tools (changes and commits, pull requests, files, terminal, through its latest conversation in the project's own checkout) and its conversations; the compose button starts a conversation already in that project.
- **Pull requests** come from the computer's GitHub inbox (`pull_request_action`, ADR-125): open ones grouped into ready to merge, review requested from you, and waiting; merged and closed in their own segments. A pull request's page has a grouped summary (branch, changes, conflicts, reviews, checks), its files and checks, the description, and "Fazer merge" pinned in glass at the bottom with the method in a menu.
- **Ajustes** moved from a sheet to the fourth tab, with the Sirus card on top and each row's symbol white on a coloured square, as iOS Settings draws them.

## Addendum (2026-10-17): runtime 4

- **Runtime 4** (`RUNTIME_VERSION` "4"): the phone needs a new native install; JavaScript updates for runtime 3 builds stop at the previous one.
- **Segmented controls** (Conversas, Pull requests) are a native `UISegmentedControl` in the theme's colours (`SirusNativeControls`, in the `sirus-terminal` module's pod); a build without that view falls back to the untinted `@expo/ui` control.
- **Enriched markdown 1.2.0-sirus.15** (T3's 1.2.0-t3.15 repacked) and `expo-glass-effect` 58.0.4, which made our patch for it unnecessary.
- **Pull request search** in the header with GitHub-style filters (`label:`, `-label:`, `author:` with `@me`, `draft:`, `review:`, `status:`; links and `owner/repo#n` match too), parsed on the phone by `src/lib/pull-request-query.ts`.

## Addendum (2026-11-09): updates over the public route

- **Address.** Tailscale is gone (ADR-198), so the only address the phone reaches is the Cloudflare route in `remote-public.json` (`https://native.siruscode.com`); the build's update URL is that plus `/api/app-update/manifest`. `renew-iphone.sh` derives it from the file's `url` (never the Access token) and warns when `ios/` was prebuilt with another.
- **Access headers.** The route is behind Cloudflare Access, which answers `expo-updates` with a login redirect unless the service token travels along. `app.config.ts` declares `CF-Access-Client-Id` and `CF-Access-Client-Secret` empty next to `Authorization`, and `checkForAppUpdate` fills all three from the paired computer. Runtime 5 builds from before this cannot update.
