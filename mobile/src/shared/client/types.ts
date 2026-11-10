import type { ShortcutId } from "../lib/keybindings";

export const AGENT_PROVIDER_IDS = ["codex", "claude", "opencode", "cursor", "grok", "antigravity", "droid", "pi", "devin", "hermes"] as const;

export type AgentProviderId = typeof AGENT_PROVIDER_IDS[number];

export type SessionStatus =
  | "idle"
  | "starting"
  | "running"
  | "waiting"
  | "completed"
  | "failed"
  | "stopped";

export type MessageRole = "user" | "agent" | "system";

export type ChangeKind = "added" | "modified" | "deleted" | "renamed" | "untracked";

export interface Project {
  id: string;
  name: string;
  path: string;
  addedAt: string;
  lastOpenedAt: string;
  /** Display-only folder colour, emoji or logo (ADR-059). */
  look?: ProjectLook;
  /** The pixel avatar the owner customised (ADR-175); absent means the defaults taken from the name. */
  appearance?: ProjectAppearance;
  /** Owner-saved Setup and On finish shell scripts (ADR-078). */
  scripts?: ProjectScripts;
}

/** Avatar choices (ADR-175, v2): `#rrggbb` colour, eye style and accessory names. `accessory` is absent on appearances saved before v2 (the name-based default applies). Validated in Rust (`sidebar.rs`). */
export interface ProjectAppearance { color: string; eyes: "bars" | "dots" | "sleepy" | "happy"; accessory?: "none" | "tophat" | "crown" | "antenna" | "bow" | "flower" | "beanie" | "headset" | "scarf" }

/** `previewUrl` is a loopback page a script serves; `openPreview` opens it in the browser once it answers (ADR-151). */
export interface ProjectScripts { setup?: string | null; onFinish?: string | null; previewUrl?: string | null; openPreview?: boolean }

export type ScriptKind = "setup" | "finish";

export type ScriptRunStatus = "running" | "succeeded" | "failed" | "timedOut" | "cancelled";

/** Latest run of one project script in a session's worktree; `output` is the bounded tail. */
export interface ScriptRun { id: string; kind: ScriptKind; status: ScriptRunStatus; startedAt: string; finishedAt?: string | null; exitCode?: number | null; output: string }

export interface ProjectFolder { id: string; name: string; look: ProjectLook; projectIds: string[]; collapsed: boolean }

export interface ProjectLook { color?: string | null; emoji?: string | null; logo?: string | null; astro?: ProjectAstroIcon | null; icon?: string | null }

/** One of the Astro cosmic icons as a project icon, drawn in the project colour (ADR-116 kept them). */
export interface ProjectAstroIcon { icon: AstroIconId; style: AstroStyle }

export type AstroIconId = "orbita" | "saturno" | "lua" | "sol" | "galaxia" | "nebulosa" | "cometa" | "buraco" | "estrela" | "pulsar" | "constelacao" | "satelite" | "foguete" | "planeta" | "asteroide" | "eclipse";

export type AstroStyle = "metal" | "neon";

export interface Worktree {
  path: string;
  branch: string;
  isolated: boolean;
}

export type ActivityKind = "read" | "edit" | "command" | "tool" | "agent" | "skill";

export type ActivityState = "running" | "completed" | "failed" | "stopped" | "unknown";

export interface ActivityStep { id: string; kind: ActivityKind; label: string; state: ActivityState; }

/** Child rows (`kind: "agent"`) may carry their native observation window and their own generic steps. */
/** `detail` names what the step touched (path, command line, query, skill); `offset` is the reply length when it began (UTF-16), placing it in the text. */
export interface ActivityItem { id: string; kind: ActivityKind; label: string; state: ActivityState; model: string | null; detail?: string; offset?: number; output?: string;
  /** Set when the full output is stored apart (ADR-147): `output` is its last lines, this its full size in bytes; `toolOutput` fetches it. */
  outputBytes?: number; startedAt?: number | null; endedAt?: number | null; steps?: ActivityStep[]; hiddenSteps?: number;
  /** Claude subagent launched with `run_in_background` (ADR-097); `timedOut` when the turn's limit stopped it. */
  background?: boolean; timedOut?: boolean;
  /** A background shell command rather than a subagent (its `Bash` call is a row of its own). */
  shell?: boolean;
  /** A marker where a background subagent finished (ADR-101): `label` names it, `state` is its outcome; the reply after it is its own answer. */
  finishedTask?: boolean;
  /** Child rows: the Claude tool call that launched the subagent (ADR-111). */
  source?: string;
  /** Child rows: a Claude workflow's phases and members (ADR-115). */
  workflow?: WorkflowProgress; }

export interface WorkflowProgress { name?: string; phases: { index: number; title: string }[]; agents: WorkflowAgent[] }

export interface WorkflowAgent { index: number; label: string; state: "queued" | "running" | "completed" | "failed"; phaseIndex?: number; model?: string; attempt?: number; tokens?: number; toolCalls?: number; durationMs?: number; result?: string; /** While it runs: the end of its answer so far (live only, never saved). */ partial?: string }

/** One entry of a subagent's own conversation, for its live view (ADR-146). Ids are stable while it streams. */
export interface SubagentFeedEntry { id: string; kind: "prompt" | "text" | "thinking" | "tool"; text?: string; label?: string; tool?: ActivityKind; detail?: string; output?: string; state?: ActivityState }

/** `subagent_feed`: `live` while this run of the app saw it stream, `transcript` read from the provider's own file, `none` when neither exists. */
export interface SubagentFeed { key: string; entries: SubagentFeedEntry[]; dropped: number; source: "live" | "transcript" | "none" }

export interface TurnReview {
  /** `committedAt`: included in a commit made through Sirus; it leaves the review (ADR-108). `diffOmitted`: transcripts and session events leave the text out; `turnReview` fetches it (ADR-164). */
  files: (FileChange & { binary: boolean; diff: string | null; diffOmitted?: boolean; undoneAt?: string | null; committedAt?: string | null })[];
  partial: boolean;
  sharedWorkspace: boolean;
  keptAt: string | null;
  expired: boolean;
  /** Commit id of the workspace's HEAD when the turn settled; undo refuses once it moved (ADR-187). */
  head?: string | null;
}

export interface TurnActivity {
  review?: TurnReview | null;
  provider: AgentProviderId; model: string | null; effort?: string | null; fast?: boolean; startedAt: number; endedAt: number | null;
  waitingSince: number | null; pausedMs: number; status: SessionStatus; items: ActivityItem[]; truncated: boolean;
  /** First sentence of the agent's latest reasoning while the turn runs (ADR-101); live only, never in transcript files. */
  thought?: string | null;
  /** The agent's own to-do plan, the whole list as of its latest update, normalized across providers (ADR-182). */
  plan?: PlanEntry[];
}

export type PlanStatus = "pending" | "inProgress" | "completed";

export interface PlanEntry { text: string; status: PlanStatus }

export interface Message {
  id: string;
  sessionId: string;
  role: MessageRole;
  content: string;
  createdAt: string;
  streaming: boolean;
  activity?: TurnActivity | null;
  /** Instructions sent into this running reply; `offset` is the reply's length when each arrived (ADR-062). */
  steers?: { text: string; at: string; offset: number }[];
  /** User messages: what was attached; images keep a thumbnail (see `thumbnailUrl`). */
  attachments?: MessageAttachment[];
  /** Agent replies: sessions started or delegated during the turn (ADR-111). */
  launched?: string[];
  /** Agent replies: HTML visuals published with `sirus_show_visual` (ADR-123). */
  visuals?: MessageVisual[];
  /** User messages: the send's `clientRequestId`, so a doubled delivery starts one turn. */
  clientRequestId?: string;
  /** User messages: this send started a fresh provider session because the context was nearly full (ADR-181). */
  rotation?: { used: number; window: number };
}

/** A self-contained HTML visual shown as a sandboxed card in a reply; the same title in a turn replaces it (ADR-123). */
export interface MessageVisual {
  id: string;
  title: string;
  html: string;
  createdAt: string;
  revision: number;
  /** Instructions the reply had taken when it was published. */
  steersBefore?: number;
}

export interface MessageAttachment {
  id?: string;
  name: string;
  kind: "file" | "folder";
  mimeType?: string;
  size?: number;
  /** Legacy or fallback only: an embedded `data:image/jpeg;base64,…` copy. */
  thumbnail?: string;
  /** The image's JPEG copy is a file on the Mac, loaded by id (`thumbnailUrl`). */
  hasThumbnail?: boolean;
}

export interface NativeThread {
  providerAccountId?: string;
  threadId: string;
  sessionId: string;
  projectId: string;
  cwd: string;
  model: string | null;
}

export interface InputQuestion {
  id: string;
  header: string;
  question: string;
  isOther: boolean;
  isSecret: boolean;
  options: { label: string; description: string }[] | null;
}

export type PendingRequestKind =
  | { type: "command"; command: string; cwd: string | null; reason: string | null }
  | { type: "fileChange"; reason: string | null; changes: { path: string; kind: string; diff: string; movePath: string | null }[] }
  | { type: "userInput"; questions: InputQuestion[] }
  | { type: "tool"; name: string; input: unknown; reason: string | null; /** What made Claude ask; explains a full-access card. */ askedBy?: "rule" | "hook" | "check" };

export interface PendingRequest {
  requestId: string;
  generation: string;
  turnId: string;
  itemId: string;
  kind: PendingRequestKind;
}

export type AgentResponse =
  | { type: "approval"; decision: "accept" | "decline" | "cancel" }
  | { type: "userInput"; answers: Record<string, string[]> };

export interface ImportOrigin {
  provider: AgentProviderId;
  conversationId: string;
}

export interface BranchInfo {
  name: string;
  current: boolean;
}

/** A custom persistent browser profile (ADR-150). */
export interface BrowserProfile { id: string; name: string }

/** One side of a conversation moved between computers (ADR-133). */
export interface SessionMoveNote {
  computer: string;
  branch: string;
  at: string;
}

export interface HandoffOrigin {
  from: AgentProviderId;
  brief: string;
  request: string;
  pending: boolean;
  /** The recap went out with a prompt the provider has not taken yet (ADR-101). */
  delivering?: boolean;
  /** A context rotation (same provider, fresh session, code-built brief), not a provider switch (ADR-181). */
  rotated?: boolean;
}

export interface Session {
  /** Native count of user/assistant messages, sent with metadata-only sessions (ADR-048). */
  transcriptLength?: number;
  /** What the last assistant reply said, natively: its last paragraph as plain text, masked, at most 280 characters (ADR-139 hover card). */
  lastReplyPreview?: string;
  /** On session events: `messages` holds only `messages[from..]` of `total` (ADR-048). */
  transcriptWindow?: { from: number; total: number };
  goal?: string | null;
  pinnedMessageIds?: string[];
  forkOrigin?: { sourceSessionId: string; sourceMessageId: string; sourceTitle: string; inheritedMessageCount: number; seededNativeThreadId?: string | null } | null;
  importOrigin?: ImportOrigin | null;
  /** This conversation continued on another computer (ADR-133). */
  movedTo?: SessionMoveNote | null;
  /** This conversation was moved here from another computer (ADR-133). */
  movedFrom?: SessionMoveNote | null;
  handoff?: HandoffOrigin | null;
  accountBindings?: Partial<Record<AgentProviderId, string>>;
  providerAccountId?: string;
  id: string;
  title: string;
  projectId: string;
  agent: AgentProviderId;
  status: SessionStatus;
  createdAt: string;
  lastActivityAt: string;
  worktree: Worktree;
  messages: Message[];
  lastError: string | null;
  model?: string | null;
  nativeThread?: NativeThread | null;
  /** Latest context-window reading the provider reported (ADR-057). */
  /** `cacheExpiresAt`: when Claude's prompt cache expires (Unix ms; ADR-157), when known. */
  contextUsage?: { used: number; window: number | null; cacheExpiresAt?: number } | null;
  /** The last turn hit the provider's usage limit; `resetsAt` in UTC ms when reported. */
  usageLimit?: { resetsAt: number | null } | null;
  /** When the person last pressed Stop (ADR-101); cleared by their next send. */
  stoppedByUserAt?: string | null;
  /** Automatic continuations after a crash in a row (ADR-118); absent when 0. */
  autoContinuations?: number;
  /** RFC 3339 instant a snoozed conversation returns (ADR-103); hidden from tabs and lists until then. */
  snoozedUntil?: string | null;
  /** When a snooze reminder fired (ADR-103); the conversation reads as unread until opened. */
  snoozeReminderAt?: string | null;
  execution?: ExecutionOptions;
  pendingRequests?: PendingRequest[];
  /** Coordinator sessions own a team plan and its progress (ADR-043). */
  team?: Team | null;
  /** Worker sessions point back to their coordinator task. */
  teamWorker?: TeamWorker | null;
  /** A side chat answers about this main session and is hidden from session lists (ADR-049). */
  sideChat?: { parentSessionId: string } | null;
  /** An "Ask" conversation about a GitHub pull request or issue: hidden from session lists, read-only on every turn (ADR-050). */
  pullAsk?: PullAskOrigin | null;
  /** The session that delegated this one (`sirus_delegate`) or started it (`sirus_create_session`), ADR-111. */
  delegatedBy?: { sessionId: string; turn: string; notify: boolean; settled: boolean; delivered: boolean; asked?: boolean } | null;
  /** Project script runs in this session's worktree; `setupPending` until its first turn (ADR-078). */
  scripts?: { setupPending?: boolean; runs?: ScriptRun[] } | null;
}

export type TeamStatus = "planning" | "proposed" | "running" | "ready" | "done" | "stopped" | "failed";

export type TeamTaskState = "pending" | "running" | "done" | "failed" | "stopped";

export type TeamMergeState = "merged" | "skipped" | "conflict" | "outOfScope";

export interface TeamTask {
  id: string; title: string; instructions: string; provider: AgentProviderId; model: string | null; paths: string[]; after: string | null;
  workerSessionId: string | null; state: TeamTaskState; merge: TeamMergeState | null; note: string | null; allowOutside?: boolean;
}

export interface Team { messageId: string; status: TeamStatus; summary: string | null; error: string | null; approval: ApprovalMode | null; tasks: TeamTask[]; }

export interface TeamWorker { coordinatorSessionId: string; taskId: string; }

/** What an Ask conversation is about; the URL is built natively from the repository and number. */
export type PullAskState = "open" | "draft" | "closed" | "merged";

export interface PullAskContext { title: string; state: PullAskState; author: string | null; headRef: string | null; baseRef: string | null; description: string }

export interface PullAskOrigin extends PullAskContext { repository: string; number: number; kind: GithubItemKind }

export type MonoFontId = "plexMono" | "jetbrains" | "fira" | "geistMono" | "source" | "roboto" | "ubuntu" | "sfMono" | "menlo" | "cascadia" | "hack" | "consolas";

/** One TextMate rule reduced to what the code highlighter shows; an empty `scope` is the default text rule. */
export interface VsCodeTokenRule { scope: string[]; foreground?: string; fontStyle?: string }

/** An installed or importable VS Code colour theme (ADR-185): only the colours Sirus maps, as `#rrggbb[aa]`. */
export interface VsCodeTheme {
  id: string;
  name: string;
  kind: "light" | "dark";
  /** `open-vsx:<namespace>.<name>` or `local:<file name>`. */
  source: string;
  colors: Record<string, string>;
  tokenColors: VsCodeTokenRule[];
}

export type NotificationSound = "glass" | "ping" | "pop" | "submarine" | "tink" | "hero";

export interface NotificationPreferences {
  toasts: boolean; system: boolean; sounds: boolean; foreground: boolean;
  permissions: boolean; questions: boolean; completion: boolean;
  /** Someone else opened a pull request or asked for your review (ADR-050). */
  pullRequests: boolean;
  permissionSound: NotificationSound; questionSound: NotificationSound; completionSound: NotificationSound;
}

export type WindowSnapShortcut = "controlOptionCommandS" | "optionShiftS" | "controlShiftS";

/** Chat behavior: a plain click on a web link opens the Sirus browser or the system browser (ADR-149). */
export type LinkOpenTarget = "app" | "system";

export interface AppSettings {
  notifications: NotificationPreferences;
  usageProviders: AgentProviderId[];
  /** Providers whose quota ring shows in the sidebar rail (at most two). */
  sidebarUsageProviders: AgentProviderId[];
  /** Messages sent while a Codex/Claude reply runs steer it instead of waiting in the queue (ADR-062). */
  steerWhileRunning: boolean;
  /** Owner-authorized CI auto-fix (ADR-064). */
  ciAutoFix: boolean;
  /** Owner switch for PR watch (ADR-079); on by default, each watch is still per session. */
  prWatch: boolean;
  /** Chat behavior (Settings → Chat behavior). */
  dictationEnterSends: boolean;
  foldFinishedTurns: boolean;
  /** The "Working" panel beside the composer while two or more sessions work (ADR-094). */
  showWorkingPanel: boolean;
  /** Chat behavior: macOS spelling and autocorrect in the composers. */
  composerAutocorrect: boolean;
  /** Chat behavior: conversations as tabs in the title bar. */
  showSessionTabs: boolean;
  /** Chat behavior: after a crash, conversations cut mid-turn continue on their own (ADR-118). */
  continueInterruptedSessions: boolean;
  /** Beta: idle Claude conversations over 200K tokens compact shortly before the cache expires (ADR-157). */
  compactIdleBeforeCacheExpiry: boolean;
  /** Claude compacts at about this many tokens (100000 to 1000000); empty leaves it to Claude Code. */
  claudeAutoCompactWindow?: string;
  /** Claude lists only the skills that fit the project; the others stay callable by name. */
  skillsByProject?: boolean;
  /** At 80% of the context window the next send starts a fresh session seeded with a brief built by code, no model call (ADR-181). */
  rotateAtContextLimit: boolean;
  /** Sounds (off by default): soft chimes when an agent finishes a turn or needs you (ADR-171). */
  agentSounds: boolean;
  /** Providers → Advanced: finished conversations that keep their agent running (16 = all). */
  keepIdleAgents: number;
  /** Browser recordings (ADR-112): 30 or 60 fps, and key presses and clicks drawn over the video. */
  browserRecordingFps: number;
  browserRecordingKeys: boolean;
  browserRecordingClicks: boolean;
  /** Browser (ADR-151): pages an agent opens or navigates float over the conversation. */
  browserShowAgentPages: boolean;
  githubLinksInApp: boolean;
  /** Where a plain click on a web link in chat or a terminal opens (ADR-149). */
  linkOpenTarget: LinkOpenTarget;
  diffWordWrap: boolean;
  confirmTerminalClose: boolean;
  /** System-wide shortcut that snaps the frontmost app window into the open composer (ADR-054). */
  windowSnapEnabled: boolean;
  windowSnapShortcut: WindowSnapShortcut;
  /** Pinned review-inbox items as `owner/repo#number` (ADR-050). */
  githubPins: string[];
  /** Unread tracking on the Pull requests page: `owner/repo#number` → the `updatedAt` last seen (at most 500),
   * and the RFC 3339 time before which everything counts as seen (null until the first load). */
  githubSeen: Record<string, string>;
  githubSeenFloor: string | null;
  /** Pull request alerts already given, `opened:owner/repo#n` or `review:owner/repo#n` (at most 200), so a restart does not repeat them. */
  githubNotified: string[];
  /** Rail customization (ADR-052): item order, hidden items and project shortcuts. */
  railItemOrder: string[];
  hiddenRailItems: string[];
  railProjectShortcuts: string[];
  customShortcuts: Partial<Record<ShortcutId, string>>;
  defaultAgent: AgentProviderId;
  openLastProject: boolean;
  /** Projects without a chosen icon show their own favicon or logo. */
  /** The chat background image file on this Mac (ADR-089), or null. */
  chatBackground: string | null;
  /** How the background is drawn. */
  chatBackgroundEffect: "none" | "dither" | "ascii" | "halftone" | "scanlines" | "haze";
  /** Show it in empty conversations only, or in all of them. */
  chatBackgroundShowOn: "empty" | "all";
  /** Image strength (%) before a conversation has messages, and once it has. */
  chatBackgroundEmptyVisibility: number;
  chatBackgroundSessionVisibility: number;
  /** The dithered glow in the project's colour behind the conversation, when no image is chosen. */
  projectWallpaper: boolean;
  worktreeBasePath: string | null;
  defaultSessionWorkspace: "ask" | "checkout" | "worktree";
  confirmCloseRunning: boolean;
  restorePreviousSessions: boolean;
  checkForUpdates: boolean;
  disabledProviders: AgentProviderId[];
  /** A turn stopped by the provider's usage limit continues on the next provider of the order by itself (ADR-198). */
  limitFallbackAuto?: boolean;
  /** Where a conversation goes when its provider's limit runs out, first to last (ADR-198). */
  limitFallbackOrder?: AgentProviderId[];
  providerPaths: Partial<Record<AgentProviderId, string>>;
  theme: "dark" | "light" | "system";
  darkWindowTranslucent: boolean;
  lightWindowTranslucent: boolean;
  darkWindowOpacity: number;
  lightWindowOpacity: number;
  darkSidebarTranslucent: boolean;
  lightSidebarTranslucent: boolean;
  darkSidebarOpacity: number;
  lightSidebarOpacity: number;
  translucentOpacity: number;
  systemUiFont: boolean;
  uiFont: "inter" | "geist" | "dmSans" | "plexSans" | "humanist" | "helvetica";
  codeFont: MonoFontId;
  codeFontSize: number;
  terminalFont: MonoFontId;
  fontSmoothing: boolean;
  dockIcon: "default" | "smokedGlass" | "white";
  /** Imported VS Code colour themes (ADR-185), at most 48; the two ids are in use for the light and dark palettes (null is Default (Sirus)). */
  vscodeThemes: VsCodeTheme[];
  lightVscodeTheme: string | null;
  darkVscodeTheme: string | null;
  density: "compact" | "default" | "comfortable";
  animations: boolean;
  composerLineSpeed: "slow" | "smooth" | "fast";
  glass: boolean;
  pointerGlow: boolean;
  reduceMotion: boolean;
  uiFontSize: number;
  gitAutoFetch: boolean;
  gitShowUntracked: boolean;
  gitConfirmDestructive: boolean;
  worktreeLocation: "automatic" | "custom";
  worktreeBranchPattern: string;
  terminalUseSystemShell: boolean;
  terminalFontSize: number;
  terminalCursorStyle: string;
  /** "alacritty" or "ghostty": the engine new terminals on the Mac open on (ADR-199). */
  terminalEngine?: string;
  terminalScrollback: number;
  developerLogs: boolean;
  /** Offers the app-owned computer-use tools to Codex/Claude/OpenCode turns (ADR-038). */
  computerUseEnabled: boolean;
  /** Lists the `sirus_*` session tools on every session's MCP bridge (ADR-076). */
  agentsManageSessions: boolean;
  experimental: boolean;
  disabledModels: string[];
  disabledSkills: string[];
  favoriteModels: string[];
  defaultModel: string | null;
  modelExecution: Record<string, ExecutionOptions>;
  sidebarCollapsed: boolean;
  /** What the sidebar shows when not hidden: the icon rail, the conversation list or one project at a time (ADR-139). */
  sidebarMode: "rail" | "list";
  /** The conversation list's order: flat by last activity, or project folders (ADR-139). */
  sidebarListOrder: "recent" | "project";
  /** Projects whose folder is open in the By project order (at most 100 ids, ADR-139 round 2); null until one is opened or closed, when the selected project's folder shows open. */
  sidebarOpenFolders: string[] | null;
  sidebarProjectOrder: string[];
  sidebarProjectSortOrder: "manual" | "created_at";
  sidebarThreadSortOrder: "updated_at" | "created_at";
  /** Working, waiting and unread sessions float above the thread order (ADR-121). */
  sidebarAttentionFirst: boolean;
  /** Idle conversations settle after this many days, 1 to 365 (ADR-198). */
  settleIdleDays: number;
  /** Idle conversations settle at all (ADR-198). */
  settleIdleEnabled: boolean;
  /** When each settled conversation was last opened or sent to, by session id (ADR-198). */
  settleRevived: Record<string, string>;
  /** Order of the sidebar's navigation rows, by id, as dragged on the Mac (ADR-198). */
  sidebarNavOrder: string[];
  pinnedProjectIds: string[];
  /** Switcher folders that group projects (display only). */
  projectFolders: ProjectFolder[];
  pinnedSessionIds: string[];
  /** Remote MCP servers every supported agent receives, per project or for all (ADR-137). */
  connectors: Connector[];
  environmentPanelDefaultOpen: boolean;
  showEnvironmentUsage: boolean;
  showEnvironmentRepository: boolean;
  showEnvironmentEditor: boolean;
  showEnvironmentPullRequest: boolean;
  showEnvironmentPinned: boolean;
  showEnvironmentNotepad: boolean;
  showEnvironmentInstructions: boolean;
  enableProviderUpdateChecks: boolean;
  /** A turn stopped by a usage limit continues by itself at the reset (ADR-063). */
  usageAutoResume: boolean;
  /** In-app updates (ADR-140): release builds look for a new version on their own. */
  appUpdateAutoCheck: boolean;
  /** A version found on its own downloads and installs in the background, then waits for a restart. */
  appUpdateAutoDownload: boolean;
  /** The first launch of a new version shows its notes once. */
  appUpdateWhatsNew: boolean;
  /** Notes of the last installed update, for What's new and Settings. */
  appUpdateNotes?: AppUpdateNotes | null;
  locale: "pt-BR" | "en";
  /** New conversations start on this approval mode when the provider offers it (ADR-130). */
  defaultApproval: ApprovalMode;
  /** Fast-forward the project checkout's default branch while it has no local changes (ADR-130). */
  gitAutoPull: boolean;
  /** New worktrees start from origin's default branch after a fetch (ADR-130). */
  worktreesFromOrigin: boolean;
  /** How reply text appears while it streams (ADR-130). */
  responseStreaming: ResponseStreaming;
  /** Agents may use the in-app browser tools (ADR-130). */
  agentBrowserAccess: boolean;
  /** Custom persistent browser profiles, at most 10 (ADR-150). */
  browserProfiles: BrowserProfile[];
  /** The profile new browser tabs open in: `default`, `private` or a custom id (ADR-150). */
  defaultBrowserProfile: string;
}

export type ResponseStreaming = "turn" | "paragraph" | "token";

export interface UsageWindow {
  id: string;
  usedPercent: number | null;
  resetsAt: number | null;
  durationMinutes: number | null;
}

export interface UsageAccount {
  email: string | null;
  name: string | null;
  plan: string | null;
  keyFingerprint: string | null;
}

export interface ProviderUsage {
  providerAccountId?: string;
  provider: AgentProviderId;
  status: "available" | "unavailable" | "error";
  windows: UsageWindow[];
  updatedAt: number;
  note: string | null;
  account: UsageAccount | null;
  resetCount: number;
  resetOffer: string | null;
}

export interface ProviderAccount { id: string; provider: AgentProviderId; label: string }

export interface AppData {
  contextTexts?: Record<string, string>;
  providerAccounts?: ProviderAccount[];
  selectedProviderAccounts?: Partial<Record<AgentProviderId, string>>;
  projects: Project[];
  sessions: Session[];
  settings: AppSettings;
  composerDrafts: Record<string, string>;
}

export interface GitIdentity {
  isRepo: boolean;
  root: string | null;
  branch: string | null;
  detached: boolean;
}

export interface FileChange {
  path: string;
  kind: ChangeKind;
  additions: number;
  deletions: number;
}

export interface FileEntry {
  name: string;
  path: string;
  isDir: boolean;
}

export interface AgentInstall {
  id: AgentProviderId;
  name: string;
  binary: string;
  installed: boolean;
  path: string | null;
  version: string | null;
  /** Every install found on this machine, for Settings → Providers to choose from. */
  candidates?: ExecutableCandidate[];
}

export interface ExecutableCandidate {
  path: string;
  version: string | null;
}

export interface AgentEvent {
  sessionId: string;
  messageId: string;
  /** Native UTF-16 offset for idempotent renderer updates. */
  offset: number;
  message: Message | null;
  stream: string;
  chunk: string;
}

export interface GitCommitResult {
  hash: string;
  summary: string;
}

export interface GitPushResult {
  branch: string;
}

export interface TextFileSnapshot {
  path: string;
  content: string;
  size: number;
  binary: boolean;
}

export type ApprovalMode = "ask" | "auto" | "full";

/** Connectors (ADR-137). Mirrors Rust `connectors::Connector`; sign-in tokens never travel here. */
export type ConnectorAuth = "oauth" | "token" | "none";

export interface Connector {
  id: string;
  /** The catalog entry it came from; none for a custom URL. */
  catalogId?: string;
  name: string;
  url: string;
  /** Only this project's sessions receive it; none means every project. */
  projectId?: string;
  agents: AgentProviderId[];
  enabled: boolean;
  auth: ConnectorAuth;
}

export interface ExecutionOptions { effort?: string | null; fast?: boolean; planning?: boolean; approval?: ApprovalMode | null }

export interface SkillOwner { projectId: string | null; sessionId: string | null }
export interface WorkspaceFileSuggestion { path: string; isDir: boolean }
export interface WorkspaceFiles { entries: WorkspaceFileSuggestion[]; truncated: boolean }
export interface SkillSource { id: string; origin: string; scope: "project" | "user"; path: string }
export interface AgentSkill { name: string; description: string; sources: SkillSource[] }
export interface SkillsCatalog { portableDir: string; skills: AgentSkill[]; truncated: boolean }
export type SkillActionResponse = { type: "catalog"; catalog: SkillsCatalog } | { type: "preview"; document: string };

export interface PromptAttachment { id: string; name: string; kind: "file" | "folder"; content: string; truncated: boolean; mimeType?: string; size?: number; previewUrl?: string | null; owner?: string }

export interface DiscoveredModel {
  parameterized?: boolean;
  defaultFast?: boolean;
  effortLevels?: string[];
  defaultEffort?: string | null;
  fastMode?: boolean;
  fastUnavailableReason?: "usage-credits" | "organization" | "model" | "unavailable" | null;
  id: string;
  displayName: string;
  availability: "available" | "unavailable" | "unknown";
}

export interface ProviderModelList {
  provider: AgentProviderId;
  source: string;
  note: string;
  models: DiscoveredModel[];
}

/** Live Git workspace metadata; renames are explicit deletion/addition paths. */
export interface GitWorkspaceEntry {
  path: string;
  kind: FileChange["kind"];
  conflicted: boolean;
  actionable: boolean;
}

export interface GitHistoryEntry {
  hash: string;
  parents: string[];
  subject: string;
  author: string;
  authoredAt: string;
  refs: string[];
}

export interface GitWorkspaceSnapshot {
  identity: GitIdentity;
  head: string | null;
  indexToken: string;
  staged: GitWorkspaceEntry[];
  unstaged: GitWorkspaceEntry[];
  truncated: boolean;
  conflicts: boolean;
  outsideScope: boolean;
  ahead: number | null;
  behind: number | null;
  history: GitHistoryEntry[];
  historyTruncated: boolean;
}

export type GitWorkspaceResponse =
  | { type: "snapshot"; snapshot: GitWorkspaceSnapshot }
  | { type: "diff"; diff: string }
  | { type: "history"; entries: GitHistoryEntry[]; truncated: boolean }
  | { type: "pulled"; branch: string; summary: string };

export type TranscriptResponse =
  /** `start` is the first message's index in the transcript of `total` (older hosts omit both). */
  | { type: "transcript"; sessionId: string; messages: Message[]; start?: number; total?: number }
  | { type: "candidates"; sessions: { sessionId: string; messages: Message[] }[]; truncated: boolean }
  /** One command row's full output stored apart (ADR-147). */
  | { type: "toolOutput"; text: string }
  | { type: "turnReview"; review: TurnReview | null };

// Review inbox (ADR-050): pull requests and issues of owned GitHub repositories.
export type GithubItemKind = "pullRequest" | "issue";

/** List filter: `closed` means closed without merging for pull requests. */
export type GithubItemState = "open" | "closed" | "merged";

export type GithubMergeMethod = "merge" | "squash" | "rebase";

export type GithubLookupStatus = "ready" | "cliMissing" | "authRequired" | "unavailable" | string;

export interface GithubLabel { name: string; color: string | null }

export interface GithubItem {
  kind: GithubItemKind;
  repository: string;
  number: number;
  title: string;
  url: string;
  state: "open" | "closed" | "merged";
  isDraft: boolean;
  author: string | null;
  createdAt: string;
  updatedAt: string;
  headRef: string | null;
  baseRef: string | null;
  additions: number;
  deletions: number;
  reviewDecision: string | null;
  mergeable: string | null;
  labels: GithubLabel[];
  commentCount: number;
  assignees: string[];
  reviewRequests: string[];
  checks: string | null;
}

export interface GithubInbox {
  viewer: string | null;
  repositories: { repository: string; projectIds: string[] }[];
  items: GithubItem[];
  failures: { repository: string; status: GithubLookupStatus }[];
  checkedAt: string;
}

export interface GithubDetail {
  item: GithubItem;
  body: string;
  headOid: string | null;
  mergedAt: string | null;
  closedAt: string | null;
  mergeStateStatus: string | null;
  changedFiles: number;
  viewerCanUpdate: boolean;
  mergeMethods: GithubMergeMethod[];
  checks: GithubCheck[];
  reviewers: string[];
  reviews: { author: string | null; state: string; body: string; submittedAt: string | null }[];
  comments: { author: string | null; body: string; createdAt: string }[];
  commits: { oid: string; headline: string; author: string | null; committedAt: string | null }[];
  files: { path: string; additions: number; deletions: number; changeType: string }[];
  filesTruncated: boolean;
}

export type PullRequestResponse =
  | { type: "repositories"; repositories: GithubInbox["repositories"] }
  | ({ type: "inbox" } & GithubInbox)
  | ({ type: "detail" } & GithubDetail)
  | { type: "diff"; text: string; truncated: boolean }
  | { type: "failures"; checks: GithubFailedCheck[]; truncated: boolean }
  | { type: "created"; url: string }
  | { type: "threads"; threads: GithubReviewThread[]; truncated: boolean }
  | { type: "attachment"; dataUrl: string }
  | { type: "done" };

export interface GithubCheck { name: string; status: "passed" | "failed" | "pending" | "skipped" | "unknown"; url: string | null; startedAt?: string | null; completedAt?: string | null }

/** An inline review conversation (ADR-050): `line` is null once the thread is outdated. */
export interface GithubReviewThread {
  id: string;
  path: string;
  line: number | null;
  startLine: number | null;
  originalLine: number | null;
  isResolved: boolean;
  isOutdated: boolean;
  viewerCanReply: boolean;
  viewerCanResolve: boolean;
  viewerCanUnresolve: boolean;
  diffHunk: string | null;
  comments: { author: string | null; body: string; createdAt: string }[];
  commentCount: number;
}

/** One failing check with its annotations and an error-focused job log excerpt. */
export interface GithubFailedCheck { name: string; url: string | null; summary: string | null; annotations: string[]; log: string | null }

export interface DeviceProxy { port: number; token: string }

/** Where the stream client reaches the device hub proxy: base URLs plus the query token. */
export interface DeviceHubAccess { httpBase: string; wsBase: string; token: string }

/** Release notes of an installed update (What's new): shown once, `seen` after that. */
export interface AppUpdateNotes { version: string; notes: string; date?: string | null; seen: boolean }
