import { hostFeatures } from "@/lib/phone-host";
import { isColumnSidebar } from "@/lib/conversation-list";
import { githubSeenFloor } from "@/lib/github-seen";
import { planPullAlerts, pullRefreshDue } from "@/lib/github-notify";
import { appendDiffComment } from "@/lib/diff-comment";
import { startRecording, stopRecording } from "@/lib/browser-recordings";
import { promptWantsBrowserCapture } from "@/lib/browser-url";
import { forgetUnanswered, rememberUnanswered, retryIdFor } from "@/lib/unanswered-sends";
import { isUnansweredSend } from "@/client/remote-transport";
import { activeSession, lastAssistantId, observedPromptAdmission, observedQueuedAdmission, PROMPT_QUEUE_LIMIT, queueBinding, sameQueueBinding, validQueuedPrompt, type PromptQueue } from "@/lib/prompt-queue";
import { acknowledgeEditorSave, editorKey } from "@/lib/editor-state";
import { SIDEBAR_MIN_WIDTH, SIDEBAR_RAIL_WIDTH } from "@/lib/sidebar-panels";
import { clampConversationListWidth } from "@/lib/conversation-list";
import { closeTab as closeTabRule, finishedUnseen, moveTab as moveTabRule, openTab as openTabRule, TAB_LIMIT, visibleTabSessions } from "@/lib/header-tabs";
import { retainActivityNotifications } from "@/lib/notifications";
import { agentCueFor, isViewingNotice, playAgentCue } from "@/lib/agent-sounds";
import { isSnoozed } from "@/lib/snooze";
import { isAuxiliarySession, pullAskContext, pullAskKey } from "@/lib/pull-ask";
import { moveItem } from "@/lib/queue-order";
import { initialSplit, leafShowing, splitDrop, splitLeaves, splitRemove, splitResize, splitSync, type SplitEdge, type SplitLayout, type SplitTarget } from "@/lib/split-layout";
import { activeProviderAccount } from "@/lib/provider-accounts";
import type { ComposerContext } from "@/lib/composer-context";
import { OutputScheduler } from "@/lib/output-scheduler";
import { applyAgentOutputs } from "@/lib/agent-events";
import { mergeLoadedTranscript, mergeSessionEvent, transcriptsToKeep } from "@/lib/transcripts";
import { appendAttachments } from "@/lib/composer-attachments";
import { canPreviewAttachment, documentError } from "@/lib/document-reader";
import { composerContextForOwner, composerPrompt, emptyComposerContext } from "@/lib/composer-context";
import { supportsPlanning } from "@/lib/execution-options";
import { secondOpinionPrompt, secondOpinionTurn } from "@/lib/second-opinion";
import { appendTranscriptQuote } from "@/lib/transcript-selection";
import { createDraftWriter } from "@/lib/draft-persistence";
import { CONTEXT_TEXT_LIMIT, hasContextOwner, normalizeContextTexts } from "@/lib/context-text";
import { newId, newRequestId } from "@/lib/ids";
import { scanLocalServers, type NativeLocalServer } from "@/lib/local-servers";
import type { PlayerCorner } from "@/lib/mini-player";
import { pruneSidebarSettings, sidebarGroups } from "@/lib/sidebar-layout";
import type { PullRequestLoadState } from "@/lib/pull-requests";
import { create } from "zustand";
import { client } from "@/client";
import type {
  AgentInstall,
  BrowserBounds,
  BrowserSessionState,
  ComputerAction,
  LoginPrompts,
  SecretAction,
  SecretSnapshot,
  ComputerSnapshot,
  ExecutionOptions,
  AgentEvent,
  AgentProviderId,
  AppSettings,
  HostInfo,
  FileChange,
  GitIdentity,
  GitStatus,
  Project,
  ProviderModelList,
  ProviderUsage,
  Session,
  WindowSnapEvent,
} from "@/client/types";
import { formatUnknownError } from "@/lib/format-error";
import { COMPACT_COMMAND, compactRefusal } from "@/lib/compact-before-send";
import { runningTerminalTarget, withTerminalRunning, type RunningTerminals } from "@/lib/terminal-running";
import { insertAt, popClosed, pushClosed, rememberBrowserTab, type ClosedItem } from "@/lib/closed-items";
import { CONTINUE_PROMPT, exhaustedReset, RESUME_RECHECK_MS, resumeArmed, resumeAt } from "@/lib/usage-limit";
import { providerById, PROVIDERS } from "@/lib/provider-registry";
import { focusComposer } from "@/lib/composer-focus";
import {
  defaultSettings,
  mergeSettings,
  modelKey,
  parseModelKey,
  shareSettings,
  type SettingsSectionId,
} from "@/lib/settings";

export type DockPaneKind = "terminal" | "files" | "changes" | "editor" | "browser" | "document" | "review" | "sidechat" | "devices" | "subagent";
export interface DockPane {
  id: string;
  kind: DockPaneKind;
  /** Editor panes are bound to the session that opened the file. */
  sessionId?: string;
  path?: string;
  review?: { messageId: string; path?: string };
  document?: { owner: string; scope: string; attachmentId: string; name: string };
  /** A subagent's live view (ADR-146): its turn, its child row and the feed it reads. */
  subagent?: { messageId: string; itemId: string; key: string; label: string };
}
export interface TerminalRef {
  id: string;
  number: number;
}
/** A split region; its terminals are tabs inside that region. */
export interface TerminalPane {
  id: string;
  terminals: TerminalRef[];
  activeTerminalId: string;
}
export interface TerminalWorkspace {
  split: "columns" | "rows";
  panes: TerminalPane[];
}
export function terminalCount(workspace: TerminalWorkspace | undefined): number {
  return (workspace?.panes ?? []).reduce((count, pane) => count + pane.terminals.length, 0);
}
function nextTerminal(workspace: TerminalWorkspace | undefined): TerminalRef {
  const numbers = (workspace?.panes ?? []).flatMap((pane) => pane.terminals.map((terminal) => terminal.number));
  return { id: newId(), number: numbers.reduce((max, number) => Math.max(max, number), 0) + 1 };
}
function newPane(terminal: TerminalRef): TerminalPane {
  return { id: newId(), terminals: [terminal], activeTerminalId: terminal.id };
}
function withWorkspace(
  state: { terminalWorkspacesBySession: Record<string, TerminalWorkspace> },
  sessionId: string,
  workspace: TerminalWorkspace,
) {
  return { terminalWorkspacesBySession: { ...state.terminalWorkspacesBySession, [sessionId]: workspace } };
}
/** Main column: conversation or a page (pull requests; ADR-050). The Kanban board was retired (ADR-110). */
export type MainView = "session" | "pulls" | "usage";
export interface NavEntry {
  settingsPage?: { section: SettingsSectionId } | null;
  mainView: MainView;
  projectId: string | null;
  sessionId: string | null;
}
/** Mirrors the native bound in commands.rs. */
export const MAX_TERMINALS_PER_SESSION = 8;
/** Bounded browser-style navigation history. */
const MAX_NAV_ENTRIES = 50;
/** Unsaved editor buffers survive pane/session switches until discarded. */
export interface EditorBuffer {
  identity: string;
  content: string;
  saved: string;
}
export type SettingsSection = SettingsSectionId;

interface AppStore {
  transcriptSearch: import("@/lib/conversation-search").TranscriptSearch | null;
  openTranscriptSearch: (scope?: "session" | "all") => void;
  updateTranscriptSearch: (change: Partial<Pick<import("@/lib/conversation-search").TranscriptSearch, "query" | "scope">>) => void;
  closeTranscriptSearch: () => void;
  addReviewComment: (sessionId: string, messageId: string, selection: import("@/lib/diff-comment").DiffComment) => boolean;
  activityNotifications: import("@/client/types").ActivityNotification[];
  providerAccounts: import("@/client/types").ProviderAccount[];
  selectedProviderAccounts: Partial<Record<AgentProviderId, string>>;
  addProviderAccount: (provider: AgentProviderId, name: string) => Promise<import("@/client/types").ProviderAccount>;
  selectProviderAccount: (provider: AgentProviderId, id: string) => Promise<void>;
  renameProviderAccount: (provider: AgentProviderId, id: string, name: string) => Promise<void>;
  ready: boolean;
  error: string | null;
  projects: Project[];
  sessions: Session[];
  settings: AppSettings;
  agents: AgentInstall[];
  modelsByProvider: Partial<Record<AgentProviderId, ProviderModelList>>;
  usageByProvider: Partial<Record<AgentProviderId, ProviderUsage>>;
  usageLoading: Partial<Record<AgentProviderId, boolean>>;
  refreshProviderUsage: (provider: AgentProviderId, force?: boolean) => Promise<void>;
  providerUpdates: import("@/client/types").ProviderUpdate[] | null;
  updatesChecking: boolean;
  updatesApplying: boolean;
  updateError: string | null;
  checkProviderUpdates: (force?: boolean) => Promise<void>;
  applyProviderUpdates: () => Promise<void>;
  importSessions: (sessions: Session[]) => void;
  refreshProjectGit: (projectId: string) => Promise<void>;
  consumeCodexReset: (offer: string) => Promise<import("@/client/types").CodexResetResult>;
  modelChangesPending: Record<string, number>;
  selectedProjectId: string | null;
  selectedSessionId: string | null;
  mainView: MainView;
  setMainView: (view: MainView) => void;
  sidebarWidth: number;
  sidebarCollapsed: boolean;
  draftIsolated: boolean;
  /** The new conversation works in this session's worktree ("New session in this worktree"). */
  draftWorktreeOf: string | null;
  /** The landing's Temporary toggle: the session its first send creates is deleted once left. */
  draftTemporary: boolean;
  setDraftTemporary: (temporary: boolean) => void;
  /** Temporary sessions (memory-only); each is deleted after the person leaves it and it settles. */
  temporarySessionIds: string[];
  /** Usage-limit notices (memory-only): armed resumes, dismissed notices and resets read from provider usage. */
  usageResumeArmed: Record<string, true>;
  /** Automatic resumes the person cancelled, by session: the reset they were for (kept across launches). */
  usageResumeCancelled: Record<string, number>;
  usageLimitDismissed: Record<string, true>;
  usageLimitResets: Record<string, number>;
  /** Continue the session by itself once its usage limit resets (explicit, per notice). */
  armUsageResume: (sessionId: string, armed: boolean) => void;
  dismissUsageLimit: (sessionId: string) => void;
  /** Sends the continue turn now; a queue paused by the limit follows once it succeeds. */
  resumeAfterUsageLimit: (sessionId: string) => Promise<boolean>;
  /**
   * Moves a usage-limited session to another signed-in account of its provider (native copies
   * the transcript, or arms a handoff recap) and then sends the continue turn. Throws when the
   * move fails, so the picker can show why; a failed send lands in `error` like Resume.
   */
  continueOnAccount: (sessionId: string, accountId: string) => Promise<import("@/client/types").AccountMove["outcome"]>;
  /** Sends text into the running reply instead of the queue (Codex/Claude, ADR-062). */
  steerTurn: (sessionId: string, text: string) => Promise<boolean>;
  /** When the stream did not say when the limit resets, asks the provider's usage report. */
  lookupUsageLimitReset: (sessionId: string) => Promise<void>;
  composerDrafts: Record<string, string>;
  contextTexts: Record<string, string>;
  contextTextStatus: Record<string, { saving: boolean; error: string | null }>;
  setContextText: (key: string, value: string) => Promise<void>;
  flushContextText: (key: string) => Promise<void>;
  composerContexts: Record<string, ComposerContext>;
  promptQueues: Record<string, PromptQueue>;
  pausePromptQueue: (sessionId: string) => void;
  resumePromptQueue: (sessionId: string) => void;
  editQueuedPrompt: (sessionId: string, id: string, text: string) => boolean;
  cancelQueuedPrompt: (sessionId: string, id: string) => void;
  /** Moves a queued request one place earlier (-1) or later (1); one that is sending or was tried stays first. */
  moveQueuedPrompt: (sessionId: string, id: string, delta: -1 | 1) => boolean;
  setComposerContext: (key: string, context: ComposerContext) => void;
  setComposerDraft: (key: string, value: string) => void;
  dockWidth: number;
  dockOpen: boolean;
  dockMaximized: boolean;
  dockPanes: DockPane[];
  dockActivePaneId: string | null;
  terminalWorkspacesBySession: Record<string, TerminalWorkspace>;
  /** Terminals whose shell is running, per session (ADR-121); set from PTY start/exit, never polled. */
  runningTerminals: RunningTerminals;
  setTerminalRunning: (sessionId: string, terminalId: string, running: boolean) => void;
  /** Selects the session and shows its running terminal in the dock, without starting a new one. */
  revealTerminal: (sessionId: string) => void;
  localServersBySession: Record<string, string[]>;
  /** The page the floating agent-page player shows, per session (ADR-151); absent when hidden. */
  agentPageBySession: Record<string, string>;
  /** Where the floating player rests; a drag ends at the nearest corner (ADR-151). */
  playerCorner: PlayerCorner;
  /** Listening local servers from the native scan, per session (ADR-151). */
  detectedServersBySession: Record<string, NativeLocalServer[]>;
  /** Bumped when a script or terminal starts, so the open views scan once more (ADR-151). */
  serverScanTick: Record<string, number>;
  selectedFileBySession: Record<string, string | undefined>;
  editorBuffers: Record<string, EditorBuffer>;
  editorSaving: Record<string, boolean>;
  editors: import("@/client/types").EditorInstall[] | null;
  editorIcons: Record<string, string>;
  remoteUrlByProject: Record<string, string | null | undefined>;
  environmentOpen: boolean;
  navHistory: NavEntry[];
  navIndex: number;
  goBack: () => void;
  goForward: () => void;
  paletteOpen: boolean;
  settingsOpen: boolean;
  settingsSection: SettingsSection;
  newSessionOpen: boolean;
  gitByPath: Record<string, GitIdentity>;
  /** Uncommitted lines per project checkout (`+additions -deletions`), refreshed on demand. */
  projectDiffs: Record<string, { additions: number; deletions: number }>;
  refreshProjectDiffs: () => Promise<void>;
  gitStatus: GitStatus | null;
  selectedDiff: FileChange | null;
  diffText: string | null;
  hostInfo: HostInfo | null;
  bootstrap: () => Promise<void>;
  refreshAgents: () => Promise<void>;
  addProjectFromPicker: () => Promise<void>;
  openProjectPath: (path: string) => Promise<void>;
  /** "Create new project…" dialog. */
  createProjectOpen: boolean;
  setCreateProjectOpen: (open: boolean) => void;
  /** Creates the folder, repository and first commit natively, then opens a new thread there. Throws on failure. */
  createProject: (name: string, parent: string) => Promise<void>;
  cloneProject: (source: string, parent: string) => Promise<void>;
  /** Selects a project the app just made (created or cloned) and opens a new thread there. */
  addCreatedProject: (project: Project) => Promise<void>;
  selectProject: (projectId: string) => Promise<void>;
  removeProject: (projectId: string) => Promise<void>;
  renameProject: (projectId: string, name: string) => Promise<boolean>;
  setProjectAppearance: (projectId: string, appearance: import("@/client/types").ProjectAppearance | null) => Promise<boolean>;
  /** Folder colour, emoji or logo of a project (ADR-059); applied at once. */
  createSession: (
    agent: AgentProviderId,
    isolatedWorktree: boolean,
    title?: string,
    model?: string | null,
  ) => Promise<void>;
  selectSession: (sessionId: string) => Promise<void>;
  forkSession: (sessionId: string, messageId: string) => Promise<boolean>;
  handoffSession: (sessionId: string, messageId: string, agent: import("@/client/types").AgentProviderId, model: string | null, newWorktree?: boolean) => Promise<boolean>;
  dismissHandoff: (sessionId: string) => Promise<boolean>;
  /**
   * Second opinion (ADR-056): a new session for `agent`/`model` in the same
   * workspace, opened beside the source, reviews the settled turn read-only.
   */
  secondOpinion: (sessionId: string, messageId: string, agent: AgentProviderId, model: string | null) => Promise<boolean>;
  browserBySession: Record<string, BrowserSessionState>;
  /** Native computer-use state (permissions, pending approvals, grants, history); memory-only. */
  computer: ComputerSnapshot | null;
  computerAction: (action: ComputerAction) => Promise<boolean>;
  /** Private secret cards waiting for the person (ADR-077); values never enter the store. */
  secrets: SecretSnapshot | null;
  secretAction: (action: SecretAction) => Promise<boolean>;
  /** Save offers and agent sign-in approvals of the browser login vault (ADR-186); no passwords. */
  loginPrompts: LoginPrompts | null;
  loadBrowser: (sessionId: string) => Promise<void>;
  openBrowser: (sessionId: string) => Promise<boolean>;
  closeBrowser: (sessionId: string) => Promise<boolean>;
  browserNewTab: (sessionId: string, url?: string | null, profile?: string | null) => Promise<boolean>;
  browserCloseTab: (sessionId: string, tabId: string) => Promise<boolean>;
  browserSelectTab: (sessionId: string, tabId: string) => Promise<boolean>;
  browserNavigate: (sessionId: string, tabId: string, url: string) => Promise<boolean>;
  browserReload: (sessionId: string, tabId: string) => Promise<boolean>;
  browserBack: (sessionId: string, tabId: string) => Promise<boolean>;
  browserForward: (sessionId: string, tabId: string) => Promise<boolean>;
  setBrowserBounds: (sessionId: string, bounds: BrowserBounds | null) => void;
  browserAnnotateStart: (sessionId: string, tabId: string) => Promise<boolean>;
  browserAnnotateFinish: (sessionId: string, tabId: string) => Promise<import("@/client/types").BrowserAnnotation[] | null>;
  browserAnnotateCancel: (sessionId: string, tabId: string) => Promise<void>;
  browserCopyLink: (sessionId: string, tabId: string) => Promise<boolean>;
  browserCapture: (sessionId: string, tabId: string) => Promise<string | null>;
  /** Annotations with a screenshot (ADR-151): the marked elements and a picture of the page. */
  browserAnnotateCapture: (sessionId: string, tabId: string) => Promise<import("@/client/types").BrowserAnnotationCapture | null>;
  /** Shows an agent's page in the floating player; hidden while the dock's browser shows the session. */
  showAgentPage: (sessionId: string, url: string, ignoreSetting?: boolean) => void;
  hideAgentPage: (sessionId: string) => void;
  setPlayerCorner: (corner: PlayerCorner) => void;
  /** Opens a project script's preview in the session's browser (ADR-151). */
  openProjectPreview: (sessionId: string, url: string) => Promise<void>;
  refreshLocalServers: (sessionId: string) => Promise<void>;
  noteServerProcessStarted: (sessionId: string) => void;
  browserHistoryBySession: Record<string, string[]>;
  noteBrowserUrl: (sessionId: string, url: string) => void;
  setMessagePinned: (sessionId: string, messageId: string, pinned: boolean) => Promise<boolean>;
  messageJump: { sessionId: string; messageId: string; sequence: number; searchStart?: number } | null;
  jumpToMessage: (sessionId: string, messageId: string, searchStart?: number) => Promise<void>;
  /**
   * Sends to the selected session, or to `sessionId` (a side chat) when given. A retry of a
   * send passes its `clientRequestId` again (the phone's outbox); otherwise each call is a new send.
   */
  sendPrompt: (prompt: string, execution?: ExecutionOptions, sessionId?: string, clientRequestId?: string) => Promise<boolean>;
  /**
   * Compacts the conversation's native context (ADR-057): `/compact` alone, without the
   * composer's draft or attachments. Refused with a message when it cannot run now.
   */
  compactSession: (sessionId: string) => Promise<boolean>;
  /** Compact and send: runs `/compact`, then queues the request behind it. */
  compactAndSend: (prompt: string, execution: ExecutionOptions | undefined, sessionId: string) => Promise<boolean>;
  /** Review inbox lists per `${kind}:${state}` (ADR-050); memory-only, refreshed on demand. */
  githubInbox: Record<string, { data: import("@/client/types").GithubInbox | null; loading: boolean; error: string | null }>;
  loadGithubInbox: (kind: import("@/client/types").GithubItemKind, state: import("@/client/types").GithubItemState) => Promise<void>;
  /** `owner/repo#n` a chat link asked the Pull requests page to show (Chat behavior). */
  pullsSelection: string | null;
  /** Result of a discard in the Changes pane, shown by `ChangesToast` (already translated). */
  changesNotice: { id: number; variant: "success" | "error"; title: string; description?: string } | null;
  /** Saves a project's Setup and On finish scripts; blank removes one. */
  saveProjectScripts: (projectId: string, setup: string, onFinish: string, previewUrl?: string, openPreview?: boolean) => Promise<boolean>;
  /** Run again, stop or hide a session's project script run. */
  projectScriptAction: (action: Exclude<import("@/client/types").ProjectScriptsAction, { type: "save" }>) => Promise<void>;
  /** CI auto-fix progress per session (ADR-064), refreshed by native events. */
  ciAutoFix: import("@/client/types").CiFixState[];
  ciAutofixAction: (action: import("@/client/types").CiFixAction) => Promise<void>;
  /** Pull requests watched by sessions (ADR-079), refreshed by native events. */
  prWatches: import("@/client/types").PrWatch[];
  /** Closed `pr_watch_action`; failures land in `error` and resolve false. */
  prWatchAction: (action: import("@/client/types").PrWatchAction) => Promise<boolean>;
  /** Closed `pull_request_action`; failures land in `error` and resolve null. */
  pullRequestAction: (action: import("@/client/types").PullRequestAction) => Promise<import("@/client/types").PullRequestResponse | null>;
  /** Opens (or reuses) the parent's side chat in the right dock (ADR-049); `quote` starts its draft. */
  openSideChat: (parentSessionId: string, quote?: string) => Promise<boolean>;
  /** ⌥⌘S: shows the selected session's side chat, or hides it when it is showing. */
  toggleSideChat: () => void;
  /** Whether the Pull requests detail shows its Ask panel (ADR-050); memory-only. */
  pullAskOpen: boolean;
  /** Opens (or reuses) the item's read-only Ask conversation; failures land in `error` and resolve null. */
  openPullAsk: (item: import("@/client/types").GithubItem, body: string) => Promise<Session | null>;
  renameSession: (sessionId: string, title: string) => Promise<boolean>;
  /**
   * Snoozes a conversation until `until` (RFC 3339) or, with null, returns it now (ADR-103).
   * Snoozing closes its tab (moving the selection like closing it would) and raises the undo toast.
   */
  snoozeSession: (sessionId: string, until: string | null) => Promise<boolean>;
  /** The undo toast of the latest snooze (ADR-103). */
  /** `undo` returns a conversation of another computer (ADR-132), which the store does not hold. */
  snoozeNotice: { id: number; sessionId: string; until: string; wasSelected: boolean; undo?: () => Promise<boolean> } | null;
  deleteSession: (sessionId: string, removeWorktree: boolean) => Promise<boolean>;
  respondAgentRequest: (request: import("@/client/types").RespondAgentRequest) => Promise<boolean>;
  stopAgent: (sessionId?: string) => Promise<void>;
  /** Closed team actions (ADR-043); upserts returned sessions and drops removed workers. */
  teamAction: (action: import("@/client/types").TeamAction) => Promise<boolean>;
  refreshGitStatus: () => Promise<void>;
  loadDiff: (change: FileChange) => Promise<void>;
  setSidebarWidth: (width: number) => void;
  resizeSidebar: (delta: number) => void;
  toggleSidebar: () => void;
  setDraftIsolated: (isolated: boolean) => void;
  setEnvironmentOpen: (open: boolean) => void;
  toggleEnvironment: () => void;
  setDockWidth: (width: number) => void;
  toggleDock: () => void;
  toggleDockMaximized: () => void;
  openDockPane: (kind: DockPaneKind, path?: string) => void;
  openTurnReview: (sessionId: string, messageId: string, path?: string) => void;
  /** Opens a subagent's live view in the right dock (ADR-146); one pane per subagent, at most six. */
  openSubagentView: (sessionId: string, messageId: string, item: { id: string; key: string; label: string }) => void;
  keepTurnChanges: (sessionId: string, messageId: string) => Promise<void>;
  openAttachmentReader: (scope: string, attachment: import("@/client/types").PromptAttachment) => void;
  /** The attachment open in the centered attachment modal: a draft one, or a
   * sent one (`sent`) that the Mac still holds since it was attached. */
  attachmentModal: { scope: string; attachmentId: string; sent?: { name: string; size?: number; mimeType?: string } } | null;
  /** Opens a file from a sent message; `owners` are the draft scopes it may have been attached in. */
  /** Opens a sent file in the attachment viewer; resolves to null, or the reader's message key. */
  openSentAttachment: (owners: string[], attachment: import("@/client/types").MessageAttachment) => Promise<string | null>;
  closeAttachmentModal: () => void;
  closeDockPane: (paneId: string) => void;
  /** Moves a dock tab before/after another (drag to reorder). */
  moveDockPane: (paneId: string, targetId: string, edge: "before" | "after") => void;
  setActiveDockPane: (paneId: string) => void;
  ensureTerminal: (sessionId: string) => void;
  addTerminalTab: (sessionId: string, paneId: string) => void;
  setActiveTerminal: (sessionId: string, paneId: string, terminalId: string) => void;
  closeTerminal: (sessionId: string, terminalId: string) => void;
  splitTerminalPane: (sessionId: string, paneId: string, split: "columns" | "rows") => void;
  moveTerminalToOwnPane: (sessionId: string, paneId: string) => void;
  noteLocalServers: (sessionId: string, urls: string[]) => void;
  setSelectedFile: (sessionId: string, path: string) => void;
  setEditorBuffer: (key: string, content: string, saved?: string) => void;
  saveEditorBuffer: (sessionId: string, path: string) => Promise<void>;
  discardEditorBuffer: (key: string) => void;
  loadEditors: () => Promise<void>;
  loadProjectRemote: (projectId: string) => Promise<void>;
  pullRequestsBySession: Record<string, PullRequestLoadState>;
  refreshPullRequest: (sessionId: string, force?: boolean) => Promise<void>;
  setPaletteOpen: (open: boolean) => void;
  setSettingsOpen: (open: boolean) => void;
  setSettingsSection: (section: SettingsSection) => void;
  requestNewSession: () => void;
  /** The new-conversation landing, working in `sessionId`'s worktree. */
  requestNewSessionIn: (sessionId: string) => Promise<void>;
  /** Outcome of the latest global window snap, shown as a toast (ADR-054). */
  windowSnapNotice: { id: number; kind: "added" | "permission" | "own" | "failed" | "noComposer"; app?: string } | null;
  /** What a prompt link did, or why it did not (ADR-122); shown by `PromptLinkToast`. */
  promptLinkNotice: PromptLinkNotice | null;
  /** Side-by-side conversations (memory-only); the active pane always shows the selected session. */
  splitLayout: SplitLayout;
  /** The conversation being dragged toward the panes and where it would land. */
  splitDrag: { sessionId: string; drop: SplitTarget | null; state: "ok" | "full" | "none" } | null;
  setSplitDrag: (drag: AppStore["splitDrag"]) => void;
  dropOnSplit: (sessionId: string, drop: SplitTarget) => void;
  /** Opens a conversation beside the active pane (keyboard and menu alternative to dragging). */
  openInSplit: (sessionId: string, edge: Exclude<SplitEdge, "center">) => void;
  activateSplitPane: (leafId: string) => void;
  closeSplitPane: (leafId: string) => void;
  resizeSplit: (branchId: string, ratio: number) => void;
  /** Header tabs (memory-only): a per-project view over sessions; closing a tab never changes the session. */
  openTabsByProject: Record<string, string[]>;
  lastActiveTabByProject: Record<string, string>;
  /** Closed header tabs, dock panes and browser tabs, most recent last (bounded, memory-only). */
  closedItems: ClosedItem[];
  /** A blank "New session" tab per project while its landing is open; the first send turns it into the session's tab. */
  draftTabByProject: Record<string, boolean>;
  closeDraftTab: (projectId: string) => void;
  unseenSessionIds: string[];
  /** Sessions whose full transcript is in memory, with a recency tick (ADR-048). */
  loadedTranscripts: Record<string, number>;
  /** Loads a session's transcript once; resolves false when it cannot be loaded. */
  ensureTranscript: (sessionId: string, refresh?: boolean) => Promise<boolean>;
  projectSwitcherOpen: boolean;
  setProjectSwitcherOpen: (open: boolean) => void;
  closeHeaderTab: (projectId: string, sessionId: string) => void;
  closeOtherHeaderTabs: (projectId: string, sessionId: string) => void;
  closeHeaderTabsToRight: (projectId: string, sessionId: string) => void;
  /** Reopens the most recently closed header tab. */
  reopenHeaderTab: () => void;
  /** ⌘⇧T: reopens the most recently closed tab, dock pane or browser tab (ADR-102). */
  reopenClosed: () => void;
  moveHeaderTab: (projectId: string, sessionId: string, targetId: string, edge?: "before" | "after") => void;
  switchProject: (projectId: string) => Promise<void>;
  setNewSessionOpen: (open: boolean) => void;
  saveSettings: (settings: AppSettings) => Promise<void>;
  setSessionAgent: (agent: AgentProviderId) => Promise<void>;
  /** Changes the selected session's model, or `sessionId`'s (a side chat, which leaves the defaults alone). */
  setSessionModel: (provider: AgentProviderId, model: string | null, sessionId?: string) => Promise<void>;
  loadProviderModels: (id: AgentProviderId, force?: boolean) => Promise<void>;

  loadAllModels: (force?: boolean) => Promise<void>;
  toggleModelFavorite: (provider: AgentProviderId, modelId: string) => void;
}

/** Messages a transcript opened over the network shows before the rest arrives (ADR-164). */
const FIRST_PAGE = 60;
let gitRequestSequence = 0;
let diffRequestSequence = 0;
let projectRequestSequence = 0;
let settingsSaveQueue: Promise<void> = Promise.resolve();
/** Local saves still on their way: a `settings-changed` echo must not undo a newer local choice. */
let settingsSavesPending = 0;
/** When the open pull requests were last asked for (start-up, focus refresh or the page). */
let lastOpenPullsRead: number | null = null;

/**
 * Pull request alerts after a list of open pull requests loads (ADR-050 amendment, 2026-10-18):
 * remembers them in `githubNotified` first, so a restart never repeats one, then asks native,
 * which confirms each against its own list and delivers through the person's channels. Silent
 * until the first load ever has set `githubSeenFloor`.
 */
function announcePullRequests(data: import("@/client/types").GithubInbox) {
  const { ready, settings, saveSettings } = useAppStore.getState();
  if (!ready) return;
  const plan = planPullAlerts(data.items, data.viewer, settings.githubSeen, settings.githubSeenFloor, settings.githubNotified);
  if (plan.notified === settings.githubNotified) return;
  void saveSettings({ ...settings, githubNotified: [...plan.notified] });
  if (plan.alerts.length && settings.notifications.pullRequests) void client.notificationAction({ type: "pullRequests", items: plan.alerts }).catch(() => undefined);
}

/** Opens the Pull requests page from an alert, on its item when it names one. */
export function openPullAlert(pull: string | undefined) {
  const store = useAppStore.getState();
  store.setSettingsOpen(false);
  if (pull) useAppStore.setState({ pullsSelection: pull });
  store.setMainView("pulls");
}

/**
 * Reads the open pull requests again when the window comes back after a while, so new ones
 * alert without polling: at most once per `PULL_REFRESH_INTERVAL_MS` (five minutes), counting
 * every read, because each read runs one GitHub search per saved repository.
 */
function refreshPullsOnReturn() {
  if (document.visibilityState === "hidden" || !pullRefreshDue(lastOpenPullsRead)) return;
  const { ready, settings, loadGithubInbox } = useAppStore.getState();
  if (!ready || settings.hiddenRailItems?.includes("pulls")) return;
  void loadGithubInbox("pullRequest", "open");
}
const pinQueues = new Map<string, Promise<boolean>>();
const browserBoundsSignatures = new Map<string, string>();
const pullRequestRequests = new Map<string, { promise: Promise<void>; workspacePath: string; branch: string | null }>();
const directAdmissions = new Map<string, { previousMessageId: string | null; prompt: string; clear: () => void }>();
const modelSelectionQueues = new Map<string, Promise<void>>();
let modelSelectionSequence = 0;
const modelRequests = new Map<AgentProviderId, Promise<void>>();
const usageRequests = new Map<AgentProviderId, { path: string | undefined; promise: Promise<void> }>();
let resetRequest: Promise<import("@/client/types").CodexResetResult> | null = null;
const draftWriter = createDraftWriter((key, value) => client.saveComposerDraft(key, value), (error) => useAppStore.setState({ error: formatUnknownError(error) }));
const contextWriter = createDraftWriter(async (key, edit: { value: string; flush: boolean }) => {
  const state = useAppStore.getState();
  if (!hasContextOwner(key, state.projects, state.sessions)) return;
  useAppStore.setState(current => ({ contextTextStatus: { ...current.contextTextStatus, [key]: { saving: true, error: null } } }));
  await client.saveContextText(key, edit.value, edit.flush);
}, (error, key) => {
  const state = useAppStore.getState();
  if (hasContextOwner(key, state.projects, state.sessions)) useAppStore.setState(current => ({ contextTextStatus: { ...current.contextTextStatus, [key]: { saving: false, error: formatUnknownError(error) } } }));
}, (previous, next) => ({ ...next, flush: previous.flush || next.flush }));
async function writeContextText(key: string, value: string, flush: boolean) {
  await contextWriter.write(key, { value, flush });
  const state = useAppStore.getState();
  if (hasContextOwner(key, state.projects, state.sessions) && (state.contextTexts[key] ?? "") === value && !state.contextTextStatus[key]?.error) {
    useAppStore.setState(current => ({ contextTextStatus: { ...current.contextTextStatus, [key]: { saving: false, error: null } } }));
  }
}
function persistDraftChanges(before: Record<string, string>, after: Record<string, string>) {
  for (const key of new Set([...Object.keys(before), ...Object.keys(after)])) {
    if (before[key] !== after[key]) void draftWriter.write(key, after[key] ?? "");
  }
}

/** Minimum chat width kept while the dock grows. */
const MIN_CHAT_WIDTH = 440;

function clampDockWidth(width: number, state: Pick<AppStore, "sidebarCollapsed" | "sidebarWidth" | "settings">): number {
  const viewport = typeof window === "undefined" ? 1440 : window.innerWidth;
  // The sidebar is the icon rail (ADR-092) or the conversation list column (ADR-139); collapsing hides it.
  const sidebar = state.sidebarCollapsed ? 0 : isColumnSidebar(state.settings.sidebarMode) ? state.sidebarWidth : SIDEBAR_RAIL_WIDTH;
  const max = Math.max(320, viewport - sidebar - MIN_CHAT_WIDTH);
  return Math.min(max, Math.max(280, width));
}

/** Counts `bootstrap` runs so a slower earlier one cannot overwrite a newer one's state. */
let bootstrapRun = 0;
let navApplying = false;
let previousStatuses = new Map<string, Session["status"]>();
let previousSessionIds = new Set<string>();
/** Snooze reminders already counted as unread, and those acknowledged by opening the session (ADR-103). */
const noticedReminders = new Set<string>();
const acknowledgedReminders = new Set<string>();
let snoozeSequence = 0;
let lastNavKey: string | null = null;

function navKeyOf(fields: NavEntry): string {
  return `${fields.mainView}\0${fields.projectId ?? ""}\0${fields.sessionId ?? ""}\0${fields.settingsPage?.section ?? ""}`;
}

function currentNavEntry(state: AppStore): NavEntry {
  return { mainView: state.mainView, projectId: state.selectedProjectId, sessionId: state.selectedSessionId,
    settingsPage: state.settingsOpen ? { section: state.settingsSection } : null };
}

/** Applies a history entry only while its project/session still exist. */
function resolveNavEntry(entry: NavEntry, state: AppStore) {
  const selectedProjectId = entry.projectId && state.projects.some((project) => project.id === entry.projectId) ? entry.projectId : null;
  const session = selectedProjectId && entry.sessionId
    ? state.sessions.find((item) => item.id === entry.sessionId && item.projectId === selectedProjectId)?.id ?? null
    : null;
  const selectedSessionId = selectedProjectId && entry.sessionId && !session
    ? state.sessions.find((item) => item.projectId === selectedProjectId)?.id ?? null
    : session;
  return { mainView: entry.mainView, selectedProjectId, selectedSessionId,
    settingsOpen: !!entry.settingsPage, settingsSection: entry.settingsPage?.section ?? state.settingsSection };
}

function recordNavigation() {
  const state = useAppStore.getState();
  const key = navKeyOf(currentNavEntry(state));
  const current = state.navHistory[state.navIndex];
  if (current && navKeyOf(current) === key) return;
  const entry = currentNavEntry(state);
  const navHistory = [...state.navHistory.slice(0, state.navIndex + 1), entry].slice(-MAX_NAV_ENTRIES);
  useAppStore.setState({ navHistory, navIndex: navHistory.length - 1 });
}

/** True when the dock's browser pane is the one showing `sessionId` (ADR-151). */
export function isDockBrowserShowing(state: { dockOpen: boolean; dockPanes: DockPane[]; dockActivePaneId: string | null }, sessionId: string): boolean {
  if (!state.dockOpen) return false;
  const pane = state.dockPanes.find((item) => item.id === state.dockActivePaneId);
  return pane?.kind === "browser" && pane.sessionId === sessionId;
}

export const useAppStore = create<AppStore>((set, get) => ({
  providerAccounts: [], selectedProviderAccounts: {},
  addProviderAccount: async (provider, name) => {
    const account = await client.createProviderAccount(provider, name);
    set((state) => ({ providerAccounts: [...state.providerAccounts, account] }));
    return account;
  },
  selectProviderAccount: async (provider, id) => {
    await client.selectProviderAccount(provider, id);
    set((state) => ({ selectedProviderAccounts: { ...state.selectedProviderAccounts, [provider]: id } }));
    await get().refreshProviderUsage(provider, true);
  },
  renameProviderAccount: async (provider, id, name) => {
    const account = await client.renameProviderAccount(provider, id, name);
    set((state) => ({ providerAccounts: state.providerAccounts.map((current) => current.id === id ? account : current) }));
  },
  usageByProvider: {},
  usageLoading: {},
  providerUpdates: null,
  updatesChecking: false,
  updatesApplying: false,
  updateError: null,
  refreshProjectGit: async (projectId) => {
    const project = get().projects.find((item) => item.id === projectId);
    if (!project) return;
    try {
      const identity = await client.gitIdentity(project.path);
      set((state) => ({ gitByPath: { ...state.gitByPath, [project.path]: identity } }));
    } catch (error) {
      set({ error: formatUnknownError(error) });
    }
  },
  importSessions: (imported) => {
    if (!imported.length) return;
    set((state) => ({
      sessions: [...imported, ...state.sessions],
      loadedTranscripts: { ...state.loadedTranscripts, ...Object.fromEntries(imported.map((session) => [session.id, ++transcriptTick])) },
      selectedProjectId: imported[0].projectId,
      selectedSessionId: imported[0].id,
      mainView: "session",
      error: null,
    }));
    void get().refreshGitStatus();
  },
  checkProviderUpdates: async (force = false) => {
    if (!get().settings.enableProviderUpdateChecks) {
      set({ providerUpdates: [] });
      return;
    }
    if (get().updatesChecking) return;
    set({ updatesChecking: true });
    try {
      const updates = await client.providerUpdates(force);
      set({ providerUpdates: updates });
    } catch {
      set({ providerUpdates: [] });
    } finally {
      set({ updatesChecking: false });
    }
  },
  applyProviderUpdates: async () => {
    const outdated = (get().providerUpdates ?? []).filter((update) => update.updateAvailable && update.updateSupported);
    if (!outdated.length || get().updatesApplying) return;
    set({ updatesApplying: true, updateError: null, error: null });
    try {
      const results = await client.updateProviders(outdated.map((update) => update.provider));
      await get().refreshAgents();
      await get().checkProviderUpdates(true);
      // Surface failures last: agent refresh clears the shared error slot.
      const failures = results.filter((result) => !result.ok);
      if (failures.length) {
        const message = failures.map((failure) => `${providerById(failure.provider).name}: ${failure.message || "Update command failed."}`).join(" · ");
        set({ updateError: message, error: message });
      }
    } catch (error) {
      const message = formatUnknownError(error);
      set({ updateError: message, error: message });
    } finally {
      set({ updatesApplying: false });
    }
  },
  refreshProviderUsage: (provider, force = false) => {
    const accountId = activeProviderAccount(provider, selectCurrentSession(get()), get().selectedProviderAccounts);
    const stamp = () => `${get().settings.providerPaths[provider] ?? ""}::${activeProviderAccount(provider, selectCurrentSession(get()), get().selectedProviderAccounts)}`;
    const paths = stamp();
    const existing = usageRequests.get(provider);
    if (existing) return existing.path === paths ? existing.promise : existing.promise.then(() => get().refreshProviderUsage(provider, force));
    const request = (async () => {
      if (provider === "codex" && resetRequest) await resetRequest.catch(() => undefined);
      set((state) => ({ usageLoading: { ...state.usageLoading, [provider]: true }, usageByProvider: (state.usageByProvider[provider]?.providerAccountId ?? "default") === accountId ? state.usageByProvider : { ...state.usageByProvider, [provider]: undefined } }));
      try {
        const usage = await client.providerUsage(provider, force, accountId);
        if (stamp() === paths) set((state) => ({ usageByProvider: { ...state.usageByProvider, [provider]: usage } }));
      } catch {
        if (stamp() === paths) set((state) => ({ usageByProvider: { ...state.usageByProvider, [provider]: { provider, providerAccountId: accountId, status: "error", windows: [], updatedAt: Date.now(), note: "Could not read usage. Check the existing CLI login and version, then refresh.", resetCount: 0, resetOffer: null, account: null } } }));
      } finally {
        set((state) => ({ usageLoading: { ...state.usageLoading, [provider]: false } }));
      }
    })().finally(() => usageRequests.delete(provider));
    usageRequests.set(provider, { path: paths, promise: request });
    return request;
  },
  consumeCodexReset: (offer) => {
    if (resetRequest) return Promise.reject(new Error("A Codex reset is already in progress."));
    const stamp = () => `${get().settings.providerPaths.codex ?? ""}::${activeProviderAccount("codex", selectCurrentSession(get()), get().selectedProviderAccounts)}`;
    const paths = stamp();
    const request = (async () => {
      await usageRequests.get("codex")?.promise;
      set((state) => ({
        usageLoading: { ...state.usageLoading, codex: true },
        usageByProvider: state.usageByProvider.codex ? { ...state.usageByProvider, codex: { ...state.usageByProvider.codex, resetOffer: null } } : state.usageByProvider,
      }));
      try {
        const result = await client.consumeCodexReset(offer);
        if (stamp() === paths) set((state) => ({ usageByProvider: { ...state.usageByProvider, codex: result.usage } }));
        return result;
      } finally {
        set((state) => ({ usageLoading: { ...state.usageLoading, codex: false } }));
      }
    })().finally(() => { resetRequest = null; });
    resetRequest = request;
    return request;
  },
  ready: false,
  activityNotifications: [],
  error: null,
  projects: [],
  sessions: [],
  settings: defaultSettings,
  hostInfo: null,
  agents: [],
  modelsByProvider: {},
  modelChangesPending: {},
  selectedProjectId: null,
  selectedSessionId: null,
  mainView: "session",
  messageJump: null,
  transcriptSearch: null,
  openTranscriptSearch: (scope = "session") => set(state => ({ dockMaximized: false, transcriptSearch: { query: state.transcriptSearch?.query ?? "", scope: state.selectedSessionId ? scope : "all", revision: (state.transcriptSearch?.revision ?? 0) + 1 } })),
  updateTranscriptSearch: (change) => set(state => ({ transcriptSearch: state.transcriptSearch ? { ...state.transcriptSearch, ...change, query: (change.query ?? state.transcriptSearch.query).slice(0, 256) } : null })),
  closeTranscriptSearch: () => set({ transcriptSearch: null }),
  addReviewComment: (sessionId, messageId, selection) => {
    const state = get();
    const session = state.sessions.find(item => item.id === sessionId);
    const message = session?.messages.find(item => item.id === messageId && item.sessionId === sessionId);
    const review = message?.activity?.review;
    const file = review?.files.find(item => item.path === selection.path);
    if (!session || !state.projects.some(project => project.id === session.projectId) || state.selectedSessionId !== sessionId || state.selectedProjectId !== session.projectId || state.mainView !== "session" || state.settingsOpen || state.paletteOpen || state.newSessionOpen || message?.role !== "agent" || message.streaming || message.activity?.endedAt == null || review?.expired || !file || file.binary || file.diff == null || file.diff !== selection.diff) return false;
    const owner = `session:${sessionId}`;
    const next = appendDiffComment(state.composerDrafts[owner] ?? "", selection.path, file.diff, selection.from, selection.to, selection.comment);
    if (next === null) return false;
    state.setComposerDraft(owner, next);
    set({ dockMaximized: false });
    return true;
  },
  setMainView: (mainView) => set({ mainView }),
  sidebarWidth: SIDEBAR_MIN_WIDTH,
  sidebarCollapsed: false,
  draftIsolated: false,
  draftWorktreeOf: null,
  draftTemporary: false,
  setDraftTemporary: (draftTemporary) => set({ draftTemporary }),
  temporarySessionIds: [],
  usageResumeArmed: {},
  usageResumeCancelled: readResumeCancelled(),
  usageLimitDismissed: {},
  usageLimitResets: {},
  armUsageResume: (sessionId, armed) => set((state) => {
    const usageResumeArmed = { ...state.usageResumeArmed };
    const usageResumeCancelled = { ...state.usageResumeCancelled };
    if (armed) { usageResumeArmed[sessionId] = true; delete usageResumeCancelled[sessionId]; }
    else {
      delete usageResumeArmed[sessionId];
      // Cancelling also keeps the automatic resume from arming this reset again.
      const reset = limitReset(state, sessionId);
      if (reset != null) usageResumeCancelled[sessionId] = reset;
    }
    writeResumeCancelled(usageResumeCancelled);
    return { usageResumeArmed, usageResumeCancelled };
  }),
  dismissUsageLimit: (sessionId) => set((state) => {
    const usageResumeArmed = { ...state.usageResumeArmed };
    delete usageResumeArmed[sessionId];
    return { usageResumeArmed, usageLimitDismissed: { ...state.usageLimitDismissed, [sessionId]: true } };
  }),
  resumeAfterUsageLimit: async (sessionId) => {
    const session = get().sessions.find((item) => item.id === sessionId);
    if (!session?.usageLimit || activeSession(session) || resumingAfterLimit.has(sessionId)) return false;
    resumingAfterLimit.add(sessionId);
    get().armUsageResume(sessionId, false);
    try {
      await client.sendPrompt({ sessionId, prompt: CONTINUE_PROMPT, execution: session.execution ?? undefined, attachmentIds: [], attachmentOwner: `session:${sessionId}` });
      if (get().promptQueues[sessionId]?.items.length) queueAfterLimit.add(sessionId);
      return true;
    } catch (error) {
      set({ error: formatUnknownError(error) });
      return false;
    } finally {
      resumingAfterLimit.delete(sessionId);
    }
  },
  continueOnAccount: async (sessionId, accountId) => {
    const session = get().sessions.find((item) => item.id === sessionId);
    if (!session?.usageLimit || activeSession(session) || resumingAfterLimit.has(sessionId)) throw new Error("This conversation is busy; try again in a moment.");
    get().armUsageResume(sessionId, false);
    const moved = await client.moveSessionAccount(sessionId, accountId);
    const next = moved.session;
    // The usage limit stays until the continue turn clears it, so Resume can still send it.
    set((state) => ({
      sessions: state.sessions.map((item) => item.id === sessionId ? { ...item, providerAccountId: next.providerAccountId, accountBindings: next.accountBindings, forkOrigin: next.forkOrigin, nativeThread: next.nativeThread, handoff: next.handoff, lastActivityAt: next.lastActivityAt } : item),
      usageLimitResets: Object.fromEntries(Object.entries(state.usageLimitResets).filter(([id]) => id !== sessionId)),
    }));
    await get().resumeAfterUsageLimit(sessionId);
    return moved.outcome;
  },
  steerTurn: async (sessionId, text) => {
    try {
      await client.steerTurn(sessionId, text);
      return true;
    } catch (error) {
      set({ error: formatUnknownError(error) });
      return false;
    }
  },
  lookupUsageLimitReset: async (sessionId) => {
    const session = get().sessions.find((item) => item.id === sessionId);
    if (!session?.usageLimit || session.usageLimit.resetsAt != null || get().usageLimitResets[sessionId] || resetLookups.has(sessionId)) return;
    if (session.agent !== "codex" && session.agent !== "claude") return;
    resetLookups.add(sessionId);
    try {
      const reset = exhaustedReset(await client.providerUsage(session.agent, true, session.providerAccountId ?? "default"));
      if (reset && get().sessions.find((item) => item.id === sessionId)?.usageLimit) set((state) => ({ usageLimitResets: { ...state.usageLimitResets, [sessionId]: reset } }));
    } catch { /* The notice still offers Resume by hand. */ }
  },
  composerDrafts: {},
  contextTexts: {},
  contextTextStatus: {},
  setContextText: async (key, value) => {
    if (!hasContextOwner(key, get().projects, get().sessions)) return;
    if (value.length > CONTEXT_TEXT_LIMIT) {
      set(state => ({ contextTextStatus: { ...state.contextTextStatus, [key]: { saving: false, error: "Notes exceed the 16,384 character limit" } } }));
      return;
    }
    set(state => {
      const contextTexts = { ...state.contextTexts };
      if (value) contextTexts[key] = value; else delete contextTexts[key];
      return { contextTexts, contextTextStatus: { ...state.contextTextStatus, [key]: { saving: true, error: null } } };
    });
    await writeContextText(key, value, false);
  },
  flushContextText: async key => {
    if (hasContextOwner(key, get().projects, get().sessions)) await writeContextText(key, get().contextTexts[key] ?? "", true);
  },
  composerContexts: {},
  promptQueues: {},
  pausePromptQueue: (sessionId) => {
    const queue = get().promptQueues[sessionId];
    const session = get().sessions.find(session => session.id === sessionId);
    if (queue || session) set((state) => ({ promptQueues: { ...state.promptQueues, [sessionId]: { ...(queue ?? { items: [], waitingFor: session ? lastAssistantId(session) : null }), paused: true, reason: "queue.manual" } } }));
  },
  resumePromptQueue: (sessionId) => {
    const queue = get().promptQueues[sessionId];
    const session = get().sessions.find(item => item.id === sessionId);
    if (!queue || !session || queue.inFlight || !queue.items.length) return;
    if (get().modelChangesPending[sessionId] || !queue.items.every(item => sameQueueBinding(session, item))) {
      set((state) => ({ promptQueues: { ...state.promptQueues, [sessionId]: { ...queue, paused: true, reason: "queue.contextChanged" } } }));
      return;
    }
    set((state) => ({ promptQueues: { ...state.promptQueues, [sessionId]: { ...queue, paused: false, reason: undefined, waitingFor: activeSession(session) ? lastAssistantId(session) : queue.waitingFor !== lastAssistantId(session) ? queue.waitingFor : null } } }));
    void advancePromptQueue(sessionId);
  },
  editQueuedPrompt: (sessionId, id, text) => {
    const queue = get().promptQueues[sessionId];
    const item = queue?.items.find(item => item.id === id);
    if (!item || queue.inFlight === id || item.afterMessageId !== undefined || !get().sessions.some(session => session.id === sessionId)) return false;
    const prompt = composerPrompt(text, item.context);
    if (!validQueuedPrompt(prompt)) return false;
    set((state) => ({ promptQueues: { ...state.promptQueues, [sessionId]: { ...queue, items: queue.items.map(item => item.id === id ? { ...item, text, prompt } : item) } } }));
    return true;
  },
  cancelQueuedPrompt: (sessionId, id) => {
    const queue = get().promptQueues[sessionId];
    const item = queue?.items.find(item => item.id === id);
    if (!item || queue.inFlight === id) return;
    set((state) => ({ promptQueues: { ...state.promptQueues, [sessionId]: { ...queue, items: queue.items.filter(item => item.id !== id) } } }));
    releaseUnusedQueueAttachments(item.context.attachments, `session:${sessionId}`);
  },
  moveQueuedPrompt: (sessionId, id, delta) => {
    const queue = get().promptQueues[sessionId];
    const items = queue ? moveItem(queue.items, id, delta, item => queue.inFlight === item.id || item.afterMessageId !== undefined) : null;
    if (!queue || !items) return false;
    set((state) => ({ promptQueues: { ...state.promptQueues, [sessionId]: { ...queue, items } } }));
    return true;
  },
  setComposerContext: (key, context) => {
    const owner = key.startsWith("session:") ? get().sessions.some((session) => `session:${session.id}` === key) : get().projects.some((project) => `project:${project.id}` === key);
    if (owner) {
      const removed = (get().composerContexts[key]?.attachments ?? []).filter((file) => !context.attachments.some((next) => next.id === file.id));
      const releases = new Map<string, string[]>();
      for (const file of removed) {
        const shared = Object.entries(get().composerContexts).some(([other, draft]) => other !== key && draft.attachments.some((reference) => reference.id === file.id));
        const queued = Object.values(get().promptQueues).some(queue => queue.items.some(item => item.context.attachments.some(reference => reference.id === file.id)));
        if (!shared && !queued) {
          const nativeOwner = file.owner ?? key;
          releases.set(nativeOwner, [...(releases.get(nativeOwner) ?? []), file.id]);
        }
      }
      for (const [nativeOwner, ids] of releases) void client.releasePromptAttachments(nativeOwner, ids).catch(() => {});
      set((state) => ({ composerContexts: { ...state.composerContexts, [key]: context },
        dockPanes: state.dockPanes.filter((pane) => !pane.document || !(releases.get(pane.document.owner) ?? []).includes(pane.document.attachmentId)) }));
    }
  },
  setComposerDraft: (key, value) => {
    const before = get().composerDrafts;
    set((state) => {
    const composerDrafts = { ...state.composerDrafts };
    if (value) composerDrafts[key] = value; else delete composerDrafts[key];
    return { composerDrafts };
    });
    persistDraftChanges(before, get().composerDrafts);
  },
  dockWidth: 480,
  dockOpen: false,
  dockMaximized: false,
  dockPanes: [],
  moveDockPane: (paneId, targetId, edge) => set((state) => {
    const moving = state.dockPanes.find((pane) => pane.id === paneId);
    if (!moving || paneId === targetId) return state;
    const rest = state.dockPanes.filter((pane) => pane.id !== paneId);
    const at = rest.findIndex((pane) => pane.id === targetId);
    if (at < 0) return state;
    rest.splice(edge === "after" ? at + 1 : at, 0, moving);
    return { dockPanes: rest };
  }),
  dockActivePaneId: null,
  terminalWorkspacesBySession: {},
  runningTerminals: {},
  setTerminalRunning: (sessionId, terminalId, running) => set((state) => {
    const runningTerminals = withTerminalRunning(state.runningTerminals, sessionId, terminalId, running);
    return runningTerminals === state.runningTerminals ? state : { runningTerminals };
  }),
  revealTerminal: (sessionId) => {
    if (!get().sessions.some((session) => session.id === sessionId)) return;
    // selectSession switches synchronously, before it refreshes the workspace status.
    if (get().mainView !== "session" || get().selectedSessionId !== sessionId) void get().selectSession(sessionId);
    if (get().selectedSessionId !== sessionId) return;
    const target = runningTerminalTarget(get().terminalWorkspacesBySession[sessionId], get().runningTerminals[sessionId] ?? []);
    if (target) get().setActiveTerminal(sessionId, target.paneId, target.terminalId);
    // The workspace already exists, so this only opens and focuses the Terminal pane.
    get().openDockPane("terminal");
  },
  localServersBySession: {},
  agentPageBySession: {},
  playerCorner: "bottom-right",
  detectedServersBySession: {},
  serverScanTick: {},
  selectedFileBySession: {},
  browserBySession: {},
  windowSnapNotice: null,
  promptLinkNotice: null,
  splitLayout: initialSplit(),
  splitDrag: null,
  setSplitDrag: (splitDrag) => set({ splitDrag }),
  dropOnSplit: (sessionId, drop) => {
    const state = get();
    const next = splitDrop(state.splitLayout, sessionId, drop);
    if (next === state.splitLayout) return;
    set({ splitLayout: next });
    get().activateSplitPane(next.activeLeafId);
  },
  openInSplit: (sessionId, edge) => {
    const layout = get().splitLayout;
    const from = leafShowing(layout, sessionId);
    // A conversation shown in the active pane moves beside the pane that was open before it.
    const target = from?.id === layout.activeLeafId ? splitLeaves(layout.root).find((leaf) => leaf.id !== from.id)?.id : layout.activeLeafId;
    if (target) get().dropOnSplit(sessionId, { target, edge });
  },
  activateSplitPane: (leafId) => {
    const state = get();
    const leaf = splitLeaves(state.splitLayout.root).find((item) => item.id === leafId);
    if (!leaf) return;
    if (state.splitLayout.activeLeafId !== leafId) set({ splitLayout: { ...state.splitLayout, activeLeafId: leafId } });
    if (leaf.sessionId) { if (state.selectedSessionId !== leaf.sessionId || state.mainView !== "session") void get().selectSession(leaf.sessionId); }
    else if (state.selectedSessionId) get().requestNewSession();
  },
  closeSplitPane: (leafId) => {
    const state = get();
    const next = splitRemove(state.splitLayout, leafId);
    if (next === state.splitLayout) return;
    set({ splitLayout: next });
    if (next.activeLeafId !== state.splitLayout.activeLeafId) get().activateSplitPane(next.activeLeafId);
  },
  resizeSplit: (branchId, ratio) => set((state) => ({ splitLayout: splitResize(state.splitLayout, branchId, ratio) })),
  openTabsByProject: {},
  lastActiveTabByProject: {},
  closedItems: [],
  draftTabByProject: {},
  closeDraftTab: (projectId) => {
    const state = get();
    const tabs = visibleTabSessions(state.openTabsByProject[projectId] ?? [], state.sessions, projectId).map(session => session.id);
    const onDraft = state.mainView === "session" && !state.selectedSessionId && state.selectedProjectId === projectId;
    // The blank tab is the landing itself; it can only go away when another tab can take focus.
    const fallback = state.lastActiveTabByProject[projectId] && tabs.includes(state.lastActiveTabByProject[projectId]) ? state.lastActiveTabByProject[projectId] : tabs.at(-1);
    if (onDraft && !fallback) return;
    // Select the fallback in the same update, or the subscription would recreate the blank tab.
    set({ draftTabByProject: { ...state.draftTabByProject, [projectId]: false }, ...(onDraft && fallback ? { selectedSessionId: fallback } : {}) });
    if (onDraft && fallback) void get().selectSession(fallback);
  },
  unseenSessionIds: [],
  githubInbox: {},
  pullsSelection: null,
  pullAskOpen: false,
  changesNotice: null,
  snoozeNotice: null,
  saveProjectScripts: async (projectId, setup, onFinish, previewUrl = "", openPreview = false) => {
    try {
      const updated = await client.projectScriptsAction({ type: "save", projectId, setup: setup.trim() || null, onFinish: onFinish.trim() || null, previewUrl: previewUrl.trim() || null, openPreview });
      if (updated) set((state) => ({ error: null, projects: state.projects.map((project) => project.id === updated.id ? { ...project, scripts: updated.scripts } : project) }));
      return Boolean(updated);
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },
  projectScriptAction: async (action) => {
    try {
      await client.projectScriptsAction(action);
      if (action.type === "run") get().noteServerProcessStarted(action.sessionId);
    } catch (error) { set({ error: formatUnknownError(error) }); }
  },
  ciAutoFix: [],
  prWatches: [],
  loadedTranscripts: {},
  ensureTranscript: (sessionId, refresh = false) => {
    // A load in flight (the rest of a first page, below) answers before "already loaded".
    const pending = transcriptRequests.get(sessionId);
    if (pending) return pending;
    if (!refresh && get().loadedTranscripts[sessionId] !== undefined) {
      useAppStore.setState((state) => ({ loadedTranscripts: { ...state.loadedTranscripts, [sessionId]: ++transcriptTick } }));
      return Promise.resolve(true);
    }
    const apply = (messages: import("@/client/types").Message[]) => useAppStore.setState((state) => {
      if (!state.sessions.some((session) => session.id === sessionId)) return {};
      return {
        sessions: state.sessions.map((session) => session.id === sessionId ? { ...session, messages: mergeLoadedTranscript(session.messages, messages) } : session),
        loadedTranscripts: { ...state.loadedTranscripts, [sessionId]: ++transcriptTick },
      };
    });
    // Over the network (the phone's web app, another computer) the newest messages show first
    // and the rest follows (ADR-164). Callers that need the whole transcript (search, jumps,
    // forks) await this request, which settles only once it is all here.
    const firstPage = !refresh && client.overNetwork && get().loadedTranscripts[sessionId] === undefined
      ? client.loadTranscriptPage(sessionId, FIRST_PAGE).then((page) => {
        const start = page.start ?? 0;
        if (start === 0) return page.messages;
        // Only a page well short of the whole: a live event's window then always starts past
        // what the page holds, so it reloads instead of splicing into the partial list.
        if (start >= FIRST_PAGE) apply(page.messages);
        return null;
      }, () => null)
      : Promise.resolve(null);
    const request = firstPage.then((whole) => whole ?? client.loadTranscript(sessionId)).then((messages) => {
      apply(messages);
      releaseTranscripts();
      return true;
    }, () => false).finally(() => transcriptRequests.delete(sessionId));
    transcriptRequests.set(sessionId, request);
    return request;
  },
  projectSwitcherOpen: false,
  setProjectSwitcherOpen: (open) => set({ projectSwitcherOpen: open }),
  closeHeaderTab: (projectId, sessionId) => {
    const state = get();
    const result = closeTabRule(state.openTabsByProject[projectId] ?? [], sessionId);
    if (result.index < 0) return;
    const wasActive = state.mainView === "session" && state.selectedSessionId === sessionId;
    // Move the selection in the same update, or the tab-opening subscription would reopen the closed tab.
    set({
      openTabsByProject: { ...state.openTabsByProject, [projectId]: result.tabs },
      closedItems: pushClosed(state.closedItems, [{ kind: "session", projectId, sessionId, index: result.index }]),
      ...(wasActive ? { selectedSessionId: result.neighbor } : {}),
    });
    if (wasActive) {
      if (result.neighbor) void get().selectSession(result.neighbor);
      else get().requestNewSession();
    }
  },
  closeOtherHeaderTabs: (projectId, sessionId) => {
    const state = get();
    const tabs = state.openTabsByProject[projectId] ?? [];
    set({
      openTabsByProject: { ...state.openTabsByProject, [projectId]: tabs.filter(id => id === sessionId) },
      closedItems: pushClosed(state.closedItems, tabs.flatMap((id, index) => id === sessionId ? [] : [{ kind: "session" as const, projectId, sessionId: id, index }])),
      selectedSessionId: sessionId,
    });
    if (state.selectedSessionId !== sessionId) void get().selectSession(sessionId);
  },
  closeHeaderTabsToRight: (projectId, sessionId) => {
    const state = get();
    const tabs = state.openTabsByProject[projectId] ?? [];
    const cut = tabs.indexOf(sessionId) + 1;
    if (cut <= 0) return;
    set({
      openTabsByProject: { ...state.openTabsByProject, [projectId]: tabs.slice(0, cut) },
      closedItems: pushClosed(state.closedItems, tabs.slice(cut).map((id, offset) => ({ kind: "session" as const, projectId, sessionId: id, index: cut + offset }))),
      ...(state.selectedSessionId && tabs.slice(cut).includes(state.selectedSessionId) ? { selectedSessionId: sessionId } : {}),
    });
    if (state.selectedSessionId && tabs.slice(cut).includes(state.selectedSessionId)) void get().selectSession(sessionId);
  },
  reopenHeaderTab: () => {
    const state = get();
    const { item, rest } = popClosed(state.closedItems, (entry) => closedItemReopenable(state, entry), "session");
    set({ closedItems: rest });
    if (item) reopenItem(item);
  },
  reopenClosed: () => {
    const state = get();
    const { item, rest } = popClosed(state.closedItems, (entry) => closedItemReopenable(state, entry));
    set({ closedItems: rest });
    if (item) reopenItem(item);
  },
  moveHeaderTab: (projectId, sessionId, targetId, edge) => {
    const tabs = get().openTabsByProject[projectId] ?? [];
    set({ openTabsByProject: { ...get().openTabsByProject, [projectId]: moveTabRule(tabs, sessionId, targetId, edge) } });
  },
  switchProject: async (projectId) => {
    set({ projectSwitcherOpen: false });
    const before = get();
    const alive = (id: string | undefined) => !!id && before.sessions.some(session => session.id === id && session.projectId === projectId);
    const remembered = before.lastActiveTabByProject[projectId];
    const target = alive(remembered) ? remembered : (before.openTabsByProject[projectId] ?? []).find(alive);
    await get().selectProject(projectId);
    // Returning to a project restores its last active tab; a project without tabs keeps selectProject's choice.
    if (target) await get().selectSession(target);
  },
  secrets: null,
  secretAction: async (action) => {
    try {
      set({ secrets: await client.secretAction(action) });
      return true;
    } catch (error) {
      set({ error: formatUnknownError(error) });
      return false;
    }
  },
  loginPrompts: null,
  computer: null,
  computerAction: async (action) => {
    try {
      set({ computer: await client.computerAction(action) });
      return true;
    } catch (error) {
      set({ error: formatUnknownError(error) });
      return false;
    }
  },
  browserHistoryBySession: {},
  editorBuffers: {},
  editorSaving: {},
  editors: null,
  editorIcons: {},
  remoteUrlByProject: {},
  environmentOpen: false,
  navHistory: [],
  navIndex: -1,
  setEnvironmentOpen: (open) => set({ environmentOpen: open }),
  toggleEnvironment: () => {
    const environmentOpen = !get().environmentOpen;
    set({ environmentOpen });
    // The header toggle is the manual preference: actions and Escape only close.
    if (get().settings.environmentPanelDefaultOpen !== environmentOpen) {
      void get().saveSettings({ ...get().settings, environmentPanelDefaultOpen: environmentOpen });
    }
  },
  goBack: () => {
    const state = get();
    if (state.navIndex <= 0) return;
    const entry = state.navHistory[state.navIndex - 1];
    navApplying = true;
    set({ ...resolveNavEntry(entry, state), navIndex: state.navIndex - 1, gitStatus: null, selectedDiff: null, diffText: null });
    navApplying = false;
    void get().refreshGitStatus();
  },
  goForward: () => {
    const state = get();
    if (state.navIndex >= state.navHistory.length - 1) return;
    const entry = state.navHistory[state.navIndex + 1];
    navApplying = true;
    set({ ...resolveNavEntry(entry, state), navIndex: state.navIndex + 1, gitStatus: null, selectedDiff: null, diffText: null });
    navApplying = false;
    void get().refreshGitStatus();
  },
  paletteOpen: false,
  settingsOpen: false,
  settingsSection: "general",
  newSessionOpen: false,
  gitByPath: {},
  projectDiffs: {},
  refreshProjectDiffs: async () => {
    try {
      const stats = await client.projectDiffStats();
      set({ projectDiffs: Object.fromEntries(stats.map(({ projectId, additions, deletions }) => [projectId, { additions, deletions }])) });
    } catch {
      // A missing folder or Git error just leaves the numbers out.
    }
  },
  gitStatus: null,
  selectedDiff: null,
  diffText: null,

  bootstrap: async () => {
    // A newer bootstrap (a quick second computer switch) wins: this one's late results are dropped.
    const run = ++bootstrapRun;
    const stale = () => run !== bootstrapRun;
    try {
      // CLI version probes and other projects' Git identities fill in after the first paint.
      const detection = client.detectAgents().then((agents) => ({ agents }), (error: unknown) => ({ error }));
      const [data, hostInfo] = await Promise.all([client.loadState(), client.hostInfo().catch(() => null)]);
      const projects = [...data.projects].sort((a, b) => b.lastOpenedAt.localeCompare(a.lastOpenedAt));
      const settings = mergeSettings(data.settings);
      const selectedProjectId = settings.openLastProject ? (projects[0]?.id ?? null) : null;
      const sessions = data.sessions;
      const selectedSessionId =
        settings.restorePreviousSessions && selectedProjectId
          ? (sessions.find((session) => session.projectId === selectedProjectId && !isAuxiliarySession(session))?.id ?? null)
          : null;
      // Only the selected transcript loads before the first paint (ADR-048).
      const initialTranscript = selectedSessionId ? await client.loadTranscript(selectedSessionId).catch(() => null) : null;
      const identify = async (project: Project) => {
        try { return [project.path, await client.gitIdentity(project.path)] as const; }
        catch (error) { set({ error: formatUnknownError(error) }); return null; }
      };
      const selectedProject = projects.find((project) => project.id === selectedProjectId);
      const identities = selectedProject ? [await identify(selectedProject)] : [];
      if (stale()) return;
      set({
        ready: true,
        hostInfo,
        projects,
        sessions: initialTranscript ? sessions.map((session) => session.id === selectedSessionId ? { ...session, messages: initialTranscript } : session) : sessions,
        loadedTranscripts: initialTranscript && selectedSessionId ? { [selectedSessionId]: ++transcriptTick } : {},
        settings,
        selectedProjectId,
        selectedSessionId,
        gitByPath: Object.fromEntries(identities.filter((item) => item !== null)),
        sidebarCollapsed: settings.sidebarCollapsed,
        environmentOpen: settings.environmentPanelDefaultOpen,
        composerDrafts: data.composerDrafts ?? {},
        contextTexts: normalizeContextTexts(data.contextTexts, projects, sessions),
        contextTextStatus: {},
        pullRequestsBySession: {},
        browserBySession: {},
        browserHistoryBySession: {},
        providerAccounts: data.providerAccounts ?? [],
        selectedProviderAccounts: data.selectedProviderAccounts ?? {},
      });
      void detection.then((result) => { if (!stale()) set("agents" in result ? { agents: result.agents } : { error: formatUnknownError(result.error) }); });
      scheduleProjectDiffs();
      // One read of the open pull requests after start-up, so the rail's unread dot is right
      // before the page is opened (no polling).
      if (!settings.hiddenRailItems?.includes("pulls")) {
        const timer = setTimeout(() => { if (!stale()) void get().loadGithubInbox("pullRequest", "open"); }, 4000);
        (timer as { unref?: () => void }).unref?.();
      }
      void Promise.all(projects.filter((project) => project !== selectedProject).map(identify)).then((rest) => !stale() && set((state) => ({
        gitByPath: { ...Object.fromEntries(rest.filter((item) => item !== null)), ...state.gitByPath },
      })));
      if (settings.enableProviderUpdateChecks) void get().checkProviderUpdates();
      if (selectedSessionId) {
        await get().refreshGitStatus();
      }
    } catch (error) {
      if (stale()) return;
      set({
        ready: true,
        error: formatUnknownError(error),
      });
    }
  },

  refreshAgents: async () => {
    try {
      const agents = await client.detectAgents();
      set({ agents, error: null });
    } catch (error) {
      set({ error: formatUnknownError(error) });
    }
  },

  addProjectFromPicker: async () => {
    // A server has no folder picker (ADR-131): the New project dialog clones or takes a typed path.
    if (!hostFeatures(get().hostInfo).pickers) { set({ createProjectOpen: true }); return; }
    const path = await client.pickFolder();
    if (path) await get().openProjectPath(path);
  },
  openProjectPath: async (path) => {
    const project = await client.addProject(path);
    const identity = await client.gitIdentity(project.path);
    set((state) => ({
      projects: state.projects.some((item) => item.id === project.id) ? state.projects.map((item) => item.id === project.id ? project : item) : [project, ...state.projects],
      selectedProjectId: project.id,
      selectedSessionId: state.sessions.find((session) => session.projectId === project.id && !isAuxiliarySession(session))?.id ?? null,
      gitStatus: null, selectedDiff: null, diffText: null,
      gitByPath: { ...state.gitByPath, [project.path]: identity },
    }));
  },

  createProjectOpen: false,
  setCreateProjectOpen: (open) => set({ createProjectOpen: open }),
  createProject: async (name, parent) => get().addCreatedProject(await client.createProject(name, parent)),
  cloneProject: async (source, parent) => get().addCreatedProject(await client.cloneProject(source, parent)),
  addCreatedProject: async (project) => {
    const identity = await client.gitIdentity(project.path).catch(() => null);
    set((state) => ({
      projects: [project, ...state.projects.filter((item) => item.id !== project.id)],
      selectedProjectId: project.id,
      gitStatus: null, selectedDiff: null, diffText: null,
      ...(identity ? { gitByPath: { ...state.gitByPath, [project.path]: identity } } : {}),
    }));
    get().requestNewSession();
  },

  selectProject: async (projectId) => {
    const sequence = ++projectRequestSequence;
    const project = await client.openProject(projectId);
    if (sequence !== projectRequestSequence) return;
    const sessions = get().sessions.filter((session) => session.projectId === projectId && !isAuxiliarySession(session));
    set({
      selectedProjectId: projectId,
      selectedSessionId: sessions[0]?.id ?? null,
      gitStatus: null,
      selectedDiff: null,
      diffText: null,
      projects: get().projects.map((item) => item.id === projectId ? project : item),
    });
    if (sessions[0]) await get().refreshGitStatus();
  },

  removeProject: async (projectId) => {
    await client.removeProject(projectId);
    const projects = get().projects.filter((project) => project.id !== projectId);
    const removedSessionIds = new Set(get().sessions.filter((session) => session.projectId === projectId).map((session) => session.id));
    const removedSessions = new Set([...removedSessionIds].map((id) => `session:${id}`));
    for (const key of [`project:${projectId}`, ...removedSessions]) draftWriter.forget(key);
    for (const key of [`project:${projectId}`, ...removedSessions]) contextWriter.forget(key);
    const composerDrafts = Object.fromEntries(Object.entries(get().composerDrafts).filter(([key]) => key !== `project:${projectId}` && !removedSessions.has(key)));
    const sessions = get().sessions.filter((session) => session.projectId !== projectId);
    const nextProject = get().selectedProjectId === projectId ? projects[0]?.id ?? null : get().selectedProjectId;
    const terminalWorkspacesBySession = Object.fromEntries(Object.entries(get().terminalWorkspacesBySession).filter(([key]) => !removedSessionIds.has(key)));
    const localServersBySession = Object.fromEntries(Object.entries(get().localServersBySession).filter(([key]) => !removedSessionIds.has(key)));
    const selectedFileBySession = Object.fromEntries(Object.entries(get().selectedFileBySession).filter(([key]) => !removedSessionIds.has(key)));
    const dockPanes = get().dockPanes.filter((pane) => !((pane.kind === "editor" || pane.kind === "review" || pane.kind === "subagent") && pane.sessionId && removedSessionIds.has(pane.sessionId)) && (!pane.document || (![pane.document.owner, pane.document.scope].some((key) => key === `project:${projectId}` || removedSessions.has(key)))));
    const dockActivePaneId = dockPanes.some((pane) => pane.id === get().dockActivePaneId) ? get().dockActivePaneId : (dockPanes[dockPanes.length - 1]?.id ?? null);
    const editorBuffers = Object.fromEntries(Object.entries(get().editorBuffers).filter(([key]) => ![...removedSessionIds].some((id) => key.startsWith(`${id}:`))));
    for (const sessionId of removedSessionIds) {
      browserBoundsSignatures.delete(sessionId);
      void client.browserClose(sessionId).catch(() => undefined);
    }
    set({
      projects,
      sessions,
      settings: pruneSidebarSettings(get().settings, projects, sessions),
      composerDrafts,
      promptQueues: Object.fromEntries(Object.entries(get().promptQueues).filter(([id]) => !removedSessionIds.has(id))),
      unseenSessionIds: get().unseenSessionIds.filter((id) => !removedSessionIds.has(id)),
      loadedTranscripts: Object.fromEntries(Object.entries(get().loadedTranscripts).filter(([id]) => !removedSessionIds.has(id))),
      contextTexts: normalizeContextTexts(get().contextTexts, projects, sessions),
      pullRequestsBySession: Object.fromEntries(Object.entries(get().pullRequestsBySession).filter(([id]) => !removedSessionIds.has(id))),
      browserBySession: Object.fromEntries(Object.entries(get().browserBySession).filter(([id]) => !removedSessionIds.has(id))),
      browserHistoryBySession: Object.fromEntries(Object.entries(get().browserHistoryBySession).filter(([id]) => !removedSessionIds.has(id))),
      contextTextStatus: Object.fromEntries(Object.entries(get().contextTextStatus).filter(([key]) => key !== `project:${projectId}` && !removedSessions.has(key))),
      composerContexts: Object.fromEntries(Object.entries(get().composerContexts).filter(([key]) => key !== `project:${projectId}` && !removedSessions.has(key))),
      selectedProjectId: nextProject,
      selectedSessionId: sessions.find((session) => session.id === get().selectedSessionId)?.id ?? sessions.find((session) => session.projectId === nextProject && !isAuxiliarySession(session))?.id ?? null,
      terminalWorkspacesBySession,
      localServersBySession,
      selectedFileBySession,
      dockPanes,
      dockActivePaneId,
      editorBuffers,
      gitStatus: null, selectedDiff: null, diffText: null,
    });
    await get().refreshGitStatus();
  },

  setProjectAppearance: async (projectId, appearance) => {
    try {
      const updated = await client.setProjectAppearance(projectId, appearance);
      set((state) => ({ projects: state.projects.map((project) => project.id === projectId ? { ...project, appearance: updated.appearance } : project) }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },
  renameProject: async (projectId, name) => {
    try {
      const updated = await client.renameProject(projectId, name.trim());
      set((state) => ({ projects: state.projects.map((project) => project.id === projectId ? { ...project, name: updated.name } : project) }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  createSession: async (agent, isolatedWorktree, title, model) => {
    const projectId = get().selectedProjectId;
    if (!projectId) return;
    try {
      const session = await client.createSession({
        projectId,
        agent,
        isolatedWorktree,
        title,
        model: model ?? null,
      });
      set((state) => ({
        sessions: [session, ...state.sessions],
        loadedTranscripts: { ...state.loadedTranscripts, [session.id]: ++transcriptTick },
        ...(state.selectedProjectId === projectId ? { selectedSessionId: session.id, mainView: "session" as const } : {}),
        newSessionOpen: false,
        error: null,
      }));
      await get().refreshGitStatus();
    } catch (error) {
      set({ error: formatUnknownError(error) });
    }
  },

  selectSession: async (sessionId) => {
    ++projectRequestSequence;
    const session = get().sessions.find((item) => item.id === sessionId);
    if (!session) return;
    // A side chat lives beside its main session (notifications can name it).
    if (session.sideChat) {
      await get().selectSession(session.sideChat.parentSessionId);
      void get().openSideChat(session.sideChat.parentSessionId);
      return;
    }
    // An Ask conversation lives beside its pull request or issue (ADR-050).
    if (session.pullAsk) {
      set({ pullsSelection: pullAskKey(session.pullAsk), pullAskOpen: true });
      get().setMainView("pulls");
      return;
    }
    set({ mainView: "session", selectedSessionId: sessionId, selectedProjectId: session.projectId, gitStatus: null, selectedDiff: null, diffText: null });
    await get().refreshGitStatus();
  },

  jumpToMessage: async (sessionId, messageId, searchStart) => {
    // Search results can point into a transcript that is not loaded yet.
    if (!(await get().ensureTranscript(sessionId))) return;
    const session = get().sessions.find((item) => item.id === sessionId);
    if (!session?.messages.some((message) => message.id === messageId)) return;
    const changed = get().selectedSessionId !== sessionId;
    set((state) => ({ mainView: "session", selectedSessionId: sessionId, selectedProjectId: session.projectId,
      messageJump: { sessionId, messageId, searchStart, sequence: (state.messageJump?.sequence ?? 0) + 1 },
      ...(changed ? { gitStatus: null, selectedDiff: null, diffText: null } : {}),
    }));
    if (changed) void get().refreshGitStatus();
  },

  forkSession: async (sessionId, messageId) => {
    try {
      const fork = await client.forkSession(sessionId, messageId);
      set((state) => ({ sessions: [fork, ...state.sessions], loadedTranscripts: { ...state.loadedTranscripts, [fork.id]: ++transcriptTick }, error: null,
        ...(state.selectedSessionId === sessionId ? { selectedSessionId: fork.id, selectedProjectId: fork.projectId,
          mainView: "session" as const, gitStatus: null, selectedDiff: null, diffText: null } : {}),
      }));
      await get().refreshGitStatus();
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  handoffSession: async (sessionId, messageId, agent, model, newWorktree = false) => {
    try {
      const handoff = await client.handoffSession(sessionId, messageId, agent, model, newWorktree);
      set((state) => ({ sessions: [handoff, ...state.sessions], loadedTranscripts: { ...state.loadedTranscripts, [handoff.id]: ++transcriptTick }, error: null }));
      // The handoff opens beside its source, which stays visible (ADR-053 panes).
      if (get().selectedSessionId !== sessionId || get().mainView !== "session") await get().selectSession(sessionId);
      const before = get().splitLayout;
      get().openInSplit(handoff.id, "right");
      if (get().splitLayout === before) await get().selectSession(handoff.id);
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  secondOpinion: async (sessionId, messageId, agent, model) => {
    if (!(await get().ensureTranscript(sessionId))) return false;
    const source = get().sessions.find((session) => session.id === sessionId);
    if (!source) return false;
    const portuguese = get().settings.locale !== "en";
    const prompt = secondOpinionPrompt(secondOpinionTurn(source, messageId), providerById(source.agent).name, portuguese);
    try {
      // Handoff provides the same-workspace session; its recap is dropped for the review prompt.
      const created = await client.handoffSession(sessionId, messageId, agent, model);
      const dismissed = await client.dismissHandoff(created.id);
      const titled = await client.renameSession(created.id, `${portuguese ? "Segunda opinião" : "Second opinion"} · ${source.title}`.slice(0, 200));
      set((state) => ({ error: null, sessions: [{ ...created, handoff: dismissed.handoff, title: titled.title, lastActivityAt: titled.lastActivityAt }, ...state.sessions],
        loadedTranscripts: { ...state.loadedTranscripts, [created.id]: ++transcriptTick } }));
      if (get().selectedSessionId !== sessionId) await get().selectSession(sessionId);
      const before = get().splitLayout;
      get().openInSplit(created.id, "right");
      if (get().splitLayout === before) await get().selectSession(created.id);
      return await get().sendPrompt(prompt, supportsPlanning(agent, model) ? { planning: true } : undefined, created.id);
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  dismissHandoff: async (sessionId) => {
    try {
      const updated = await client.dismissHandoff(sessionId);
      set((state) => ({ error: null, sessions: state.sessions.map((session) => session.id === sessionId
        ? { ...session, handoff: updated.handoff } : session) }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  loadBrowser: async (sessionId) => {
    try {
      const browser = await client.browserState(sessionId);
      set((state) => ({ error: null, browserBySession: { ...state.browserBySession, [browser.sessionId]: browser } }));
    } catch (error) { set({ error: formatUnknownError(error) }); }
  },

  openBrowser: async (sessionId) => {
    try {
      const browser = await client.browserOpen(sessionId);
      set((state) => ({ error: null, browserBySession: { ...state.browserBySession, [browser.sessionId]: browser } }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  closeBrowser: async (sessionId) => {
    try {
      await client.browserClose(sessionId);
      browserBoundsSignatures.delete(sessionId);
      set((state) => {
        const browserBySession = { ...state.browserBySession };
        delete browserBySession[sessionId];
        return { error: null, browserBySession };
      });
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  browserNewTab: async (sessionId, url = null, profile = null) => {
    try {
      const browser = await client.browserNewTab(sessionId, url ?? null, profile);
      set((state) => ({ error: null, browserBySession: { ...state.browserBySession, [browser.sessionId]: browser } }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  browserCloseTab: async (sessionId, tabId) => {
    try {
      const closing = get().browserBySession[sessionId]?.tabs.find((tab) => tab.id === tabId);
      const browser = await client.browserCloseTab(sessionId, tabId);
      set((state) => ({ error: null, browserBySession: { ...state.browserBySession, [browser.sessionId]: browser },
        ...(closing && rememberBrowserTab(closing.url) ? { closedItems: pushClosed(state.closedItems, [{ kind: "browser", sessionId, url: closing.url, title: closing.title }]) } : {}) }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  browserSelectTab: async (sessionId, tabId) => {
    try {
      const browser = await client.browserSelectTab(sessionId, tabId);
      set((state) => ({ error: null, browserBySession: { ...state.browserBySession, [browser.sessionId]: browser } }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  browserNavigate: async (sessionId, tabId, url) => {
    try {
      const browser = await client.browserNavigate(sessionId, tabId, url);
      set((state) => ({ error: null, browserBySession: { ...state.browserBySession, [browser.sessionId]: browser } }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  browserReload: async (sessionId, tabId) => {
    try {
      const browser = await client.browserReload(sessionId, tabId);
      set((state) => ({ error: null, browserBySession: { ...state.browserBySession, [browser.sessionId]: browser } }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  browserBack: async (sessionId, tabId) => {
    try {
      const browser = await client.browserBack(sessionId, tabId);
      set((state) => ({ error: null, browserBySession: { ...state.browserBySession, [browser.sessionId]: browser } }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  browserForward: async (sessionId, tabId) => {
    try {
      const browser = await client.browserForward(sessionId, tabId);
      set((state) => ({ error: null, browserBySession: { ...state.browserBySession, [browser.sessionId]: browser } }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  setBrowserBounds: (sessionId, bounds) => {
    const signature = bounds
      ? `${Math.round(bounds.x)}:${Math.round(bounds.y)}:${Math.round(bounds.width)}:${Math.round(bounds.height)}`
      : "hidden";
    if (browserBoundsSignatures.get(sessionId) === signature) return;
    browserBoundsSignatures.set(sessionId, signature);
    void client.browserSetBounds(sessionId, bounds).catch(() => undefined);
  },

  browserAnnotateStart: async (sessionId, tabId) => {
    try {
      await client.browserAnnotateStart(sessionId, tabId);
      set({ error: null });
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  browserAnnotateFinish: async (sessionId, tabId) => {
    try {
      return await client.browserAnnotateFinish(sessionId, tabId);
    } catch (error) { set({ error: formatUnknownError(error) }); return null; }
  },

  browserAnnotateCancel: async (sessionId, tabId) => {
    try { await client.browserAnnotateCancel(sessionId, tabId); } catch { /* overlay already gone */ }
  },

  browserCopyLink: async (sessionId, tabId) => {
    try {
      await client.browserCopyLink(sessionId, tabId);
      set({ error: null });
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  browserCapture: async (sessionId, tabId) => {
    try {
      return await client.browserCapture(sessionId, tabId);
    } catch (error) { set({ error: formatUnknownError(error) }); return null; }
  },

  browserAnnotateCapture: async (sessionId, tabId) => {
    try {
      return await client.browserAnnotateCapture(sessionId, tabId);
    } catch (error) { set({ error: formatUnknownError(error) }); return null; }
  },

  // The floating player (ADR-151). The dock's browser pane for the session shows the page itself,
  // so the player stays out of its way; `ignoreSetting` is for pages the person asked for.
  showAgentPage: (sessionId, url, ignoreSetting = false) => {
    const state = get();
    if (!ignoreSetting && !state.settings.browserShowAgentPages) return;
    if (isDockBrowserShowing(state, sessionId)) return;
    set((current) => ({ agentPageBySession: { ...current.agentPageBySession, [sessionId]: url } }));
  },
  hideAgentPage: (sessionId) => set((state) => {
    const agentPageBySession = { ...state.agentPageBySession };
    delete agentPageBySession[sessionId];
    return { agentPageBySession };
  }),
  setPlayerCorner: (corner) => set({ playerCorner: corner }),
  openProjectPreview: async (sessionId, url) => {
    const state = get();
    if (!state.browserBySession[sessionId]?.open && !(await state.openBrowser(sessionId))) return;
    if (!(await get().browserNewTab(sessionId, url))) return;
    get().showAgentPage(sessionId, url, true);
  },
  refreshLocalServers: async (sessionId) => {
    try {
      const servers = await client.localServersList(sessionId);
      set((state) => ({ detectedServersBySession: { ...state.detectedServersBySession, [sessionId]: servers } }));
    } catch { /* the scan is best effort; the scanned output still lists servers */ }
  },
  noteServerProcessStarted: (sessionId) => {
    // A listener binds a moment after its process starts, so the rescan waits briefly.
    setTimeout(() => set((state) => ({ serverScanTick: { ...state.serverScanTick, [sessionId]: (state.serverScanTick[sessionId] ?? 0) + 1 } })), 1500);
  },


  noteBrowserUrl: (sessionId, url) => {
    if (!/^https?:\/\//i.test(url)) return;
    set((state) => {
      const current = state.browserHistoryBySession[sessionId] ?? [];
      if (current[0] === url) return state;
      const next = [url, ...current.filter((entry) => entry !== url)].slice(0, 50);
      return { browserHistoryBySession: { ...state.browserHistoryBySession, [sessionId]: next } };
    });
  },

  setMessagePinned: (sessionId, messageId, pinned) => {
    // Serialize metadata acknowledgments without replacing newer native output.
    const request = (pinQueues.get(sessionId) ?? Promise.resolve(true)).then(async () => {
      try {
        const pinnedMessageIds = await client.setMessagePinned(sessionId, messageId, pinned);
        set((state) => ({ sessions: state.sessions.map((session) => session.id === sessionId ? { ...session, pinnedMessageIds } : session), error: null }));
        return true;
      } catch (error) { set({ error: formatUnknownError(error) }); return false; }
    }).finally(() => { if (pinQueues.get(sessionId) === request) pinQueues.delete(sessionId); });
    pinQueues.set(sessionId, request);
    return request;
  },

  sendPrompt: async (prompt, execution, targetSessionId, givenRequestId) => {
    const selectedId = targetSessionId ?? get().selectedSessionId;
    const originKey = selectedId ? `session:${selectedId}` : `project:${get().selectedProjectId}`;
    // Sending the same text again after an unanswered send repeats that send, not a new one.
    const clientRequestId = givenRequestId ?? retryIdFor(originKey, prompt) ?? newRequestId();
    const submittedDraft = get().composerDrafts[originKey];
    const originalContext = get().composerContexts[originKey];
    const restoredContext = composerContextForOwner(originKey, get().composerContexts, get().sessions);
    let submittedContext = originalContext;
    const initialSessionId = selectedId;
    const initialSession = get().sessions.find(session => session.id === initialSessionId);
    const initialBinding = initialSession ? queueBinding(initialSession) : null;
    const initialMessageId = initialSession ? lastAssistantId(initialSession) : null;
    const initialQueue = initialSessionId ? get().promptQueues[initialSessionId] : undefined;
    const intendedQueue = Boolean(initialSession && activeSession(initialSession) || initialQueue?.items.length);
    const capturedAttachment = await captureBrowserAttachment(prompt, initialSessionId);
    if (capturedAttachment) {
      try {
        submittedContext = { ...restoredContext, attachments: appendAttachments(restoredContext.attachments, [capturedAttachment]) };
      } catch (error) {
        void client.releasePromptAttachments(originKey, [capturedAttachment.id]).catch(() => undefined);
        set({ error: formatUnknownError(error) });
        return false;
      }
    }
    try {
      if (initialSessionId) {
        const session = get().sessions.find(item => item.id === initialSessionId);
        const queue = get().promptQueues[initialSessionId];
        if (!session) return false;
        if (!initialBinding || !sameQueueBinding(session, { binding: initialBinding }) || get().modelChangesPending[initialSessionId]) {
          if (capturedAttachment) releaseUnusedQueueAttachments([capturedAttachment], originKey);
          set({ error: "queue.contextChanged" });
          return false;
        }
        if (intendedQueue || activeSession(session) || queue?.items.length) {
          if (!validQueuedPrompt(prompt) || (queue?.items.length ?? 0) >= PROMPT_QUEUE_LIMIT || get().modelChangesPending[initialSessionId]) {
            if (capturedAttachment) releaseUnusedQueueAttachments([capturedAttachment], originKey);
            set({ error: "queue.limit" });
            return false;
          }
          const context = submittedContext ?? restoredContext;
          const item = { id: newId(), clientRequestId, text: submittedDraft ?? (context.attachments.length ? "" : prompt), prompt, context: { ...context, attachments: [...context.attachments] }, execution: execution ? { ...execution } : undefined, binding: initialBinding };
          const before = get().composerDrafts;
          const paused = queue?.paused || initialQueue?.paused || intendedQueue && (session.status === "failed" || session.status === "stopped");
          const base = queue ?? { paused: Boolean(paused), reason: paused ? "queue.failed" : undefined, waitingFor: activeSession(session) ? lastAssistantId(session) : null };
          set((state) => ({ promptQueues: { ...state.promptQueues, [initialSessionId]: { ...base, paused: Boolean(paused), items: [...(queue?.items ?? []), item] } } }));
          // Move references into the queue before clearing the draft. They retain
          // their native owner and must not be released by chip removal.
          if (get().composerDrafts[originKey] === submittedDraft) get().setComposerDraft(originKey, "");
          if (get().composerContexts[originKey] === originalContext) get().setComposerContext(originKey, { ...context, attachments: [], snippets: [] });
          persistDraftChanges(before, get().composerDrafts);
          set({ error: null });
          void advancePromptQueue(initialSessionId);
          return true;
        }
      }
      let sessionId = initialSessionId;
      if (!sessionId) {
        const projectId = get().selectedProjectId;
        if (!projectId) return false;
        const { settings, draftIsolated, draftTemporary, draftWorktreeOf, sessions } = get();
        const parsed = parseModelKey(settings.defaultModel);
        // Only a worktree of this project: the landing may have moved to another one since.
        const shareWorktreeOf = sessions.find((session) => session.id === draftWorktreeOf)?.projectId === projectId ? draftWorktreeOf : null;
        const session = await client.createSession({ projectId, agent: settings.defaultAgent,
          isolatedWorktree: draftIsolated && !shareWorktreeOf, shareWorktreeOf,
          model: parsed?.provider === settings.defaultAgent ? parsed.id : null });
        sessionId = session.id;
        const before = get().composerDrafts;
        set((state) => {
          const composerDrafts = { ...state.composerDrafts };
          const stayingOnOrigin = state.selectedProjectId === projectId && !state.selectedSessionId;
          const draft = stayingOnOrigin ? composerDrafts[originKey] : submittedDraft;
          if (draft) composerDrafts[`session:${session.id}`] = draft;
          if (stayingOnOrigin) delete composerDrafts[originKey];
          const composerContexts = { ...state.composerContexts };
          const transferredContext = stayingOnOrigin ? composerContexts[originKey] : submittedContext;
          // Team mode is one-shot: the plan request does not carry it into the new session.
          if (transferredContext) composerContexts[`session:${session.id}`] = transferredContext.team ? { ...transferredContext, team: false } : transferredContext;
          if (stayingOnOrigin) delete composerContexts[originKey];
          return { ...(draftTemporary ? { temporarySessionIds: [...state.temporarySessionIds, session.id], draftTemporary: false } : {}), sessions: [session, ...state.sessions], loadedTranscripts: { ...state.loadedTranscripts, [session.id]: ++transcriptTick }, composerDrafts, composerContexts,
            dockPanes: state.dockPanes.map((pane) => stayingOnOrigin && pane.document?.scope === originKey ? { ...pane, document: { ...pane.document, scope: `session:${session.id}` } } : pane),
            ...(stayingOnOrigin ? { selectedSessionId: session.id } : {}) };
        });
        persistDraftChanges(before, get().composerDrafts);
      }
      if (!sessionId) return false;
      const ownedSessionId = sessionId;
      if (!get().promptQueues[ownedSessionId]?.items.length && !get().promptQueues[ownedSessionId]?.inFlight) {
        set(state => { const promptQueues = { ...state.promptQueues }; delete promptQueues[ownedSessionId]; return { promptQueues }; });
      }
      let cleared = false;
      const previousGoal = get().sessions.find(session => session.id === ownedSessionId)?.goal;
      const admission = {
        previousMessageId: lastAssistantId(get().sessions.find(session => session.id === ownedSessionId)!),
        prompt,
        clear: () => {
          if (cleared) return;
          cleared = true;
          const before = get().composerDrafts;
          set((state) => {
            const composerDrafts = { ...state.composerDrafts };
            for (const key of [originKey, `session:${sessionId}`]) {
              if (composerDrafts[key] === submittedDraft) delete composerDrafts[key];
            }
            const composerContexts = { ...state.composerContexts };
            for (const key of [originKey, `session:${sessionId}`]) if (composerContexts[key] === submittedContext ||
              (capturedAttachment && originalContext !== undefined && composerContexts[key] === originalContext)) {
              if (submittedContext) composerContexts[key] = { ...submittedContext, attachments: [], snippets: [] };
              else delete composerContexts[key];
            }
            return { composerDrafts, composerContexts };
          });
          persistDraftChanges(before, get().composerDrafts);
        },
      };
      directAdmissions.set(ownedSessionId, admission);
      let response: Session;
      try {
        response = await client.sendPrompt({ sessionId, prompt, execution, clientRequestId, team: Boolean(submittedContext?.team) || undefined, debugging: Boolean(submittedContext?.debugging) && !submittedContext?.planning && !submittedContext?.team, goal: submittedContext?.goal, attachmentIds: submittedContext?.attachments.map((file) => file.id) ?? [], attachmentOwner: originKey, queuedAfter: initialBinding ? { ...initialBinding, messageId: initialMessageId } : undefined });
      } finally {
        if (directAdmissions.get(ownedSessionId) === admission) directAdmissions.delete(ownedSessionId);
      }
      admission.clear();
      forgetUnanswered(originKey);
      set(state => ({ error: null, sessions: state.sessions.map(session => session.id === ownedSessionId && (session.goal === previousGoal || lastAssistantId(session) === lastAssistantId(response)) ? { ...session, goal: response.goal ?? null } : session) }));
      return true;
    } catch (error) {
      const message = formatUnknownError(error);
      if (isUnansweredSend(message)) rememberUnanswered(originKey, prompt, clientRequestId);
      set({ error: message });
      return false;
    }
  },

  compactSession: async (sessionId) => {
    const refusal = compactRefusal(get().sessions.find(item => item.id === sessionId));
    if (refusal) {
      set({ error: refusal });
      return false;
    }
    try {
      // Straight to IPC: the composer's draft, attachments and modes stay where they are.
      await client.sendPrompt({ sessionId, prompt: COMPACT_COMMAND, attachmentIds: [], attachmentOwner: `session:${sessionId}` });
      set({ error: null });
      return true;
    } catch (error) {
      set({ error: formatUnknownError(error) });
      return false;
    }
  },

  compactAndSend: async (prompt, execution, sessionId) => {
    const session = get().sessions.find(item => item.id === sessionId);
    if (!session || activeSession(session)) return get().sendPrompt(prompt, execution, sessionId);
    const before = lastAssistantId(session);
    try {
      // Straight to IPC, so the draft and its attachments stay for the request itself.
      await client.sendPrompt({ sessionId, prompt: COMPACT_COMMAND, attachmentIds: [], attachmentOwner: `session:${sessionId}` });
    } catch (error) {
      set({ error: formatUnknownError(error) });
      return false;
    }
    await sessionMoved(sessionId, before);
    // A running compaction queues the request (sent on its successful settlement); a finished one sends it now.
    return get().sendPrompt(prompt, execution, sessionId);
  },

  renameSession: async (sessionId, title) => {
    try {
      const updated = await client.renameSession(sessionId, title.trim());
      set((state) => ({ sessions: state.sessions.map((session) => session.id === sessionId ? { ...session, title: updated.title, lastActivityAt: updated.lastActivityAt } : session) }));
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },
  snoozeSession: async (sessionId, until) => {
    try {
      const updated = await client.snoozeSession(sessionId, until);
      const before = get();
      const wasSelected = before.mainView === "session" && before.selectedSessionId === sessionId;
      set((state) => ({ sessions: state.sessions.map((session) => session.id === sessionId ? { ...session, snoozedUntil: updated.snoozedUntil ?? null, snoozeReminderAt: updated.snoozeReminderAt ?? null } : session) }));
      const session = get().sessions.find((item) => item.id === sessionId);
      if (!until || !session?.snoozedUntil) return true;
      // The tab closes like any other; the neighbour is a tab that is still shown.
      const state = get();
      const tabs = state.openTabsByProject[session.projectId] ?? [];
      const listed = new Set(selectListedSessions(state).map((item) => item.id));
      const neighbor = closeTabRule(tabs.filter((id) => id === sessionId || listed.has(id)), sessionId).neighbor;
      set({
        openTabsByProject: tabs.includes(sessionId) ? { ...state.openTabsByProject, [session.projectId]: tabs.filter((id) => id !== sessionId) } : state.openTabsByProject,
        snoozeNotice: { id: ++snoozeSequence, sessionId, until: session.snoozedUntil, wasSelected },
        ...(wasSelected ? { selectedSessionId: neighbor } : {}),
      });
      if (wasSelected) {
        if (neighbor) void get().selectSession(neighbor);
        else get().requestNewSession();
      }
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },
  deleteSession: async (sessionId, removeWorktree) => {
    try {
      await client.deleteSession(sessionId, removeWorktree, true);
      draftWriter.forget(`session:${sessionId}`);
      contextWriter.forget(`session:${sessionId}`);
      browserBoundsSignatures.delete(sessionId);
      void client.browserClose(sessionId).catch(() => undefined);
      set((state) => {
        // Native deletes the parent's side chats with it (ADR-049).
        const sideChats = new Set(state.sessions.filter((session) => session.sideChat?.parentSessionId === sessionId).map((session) => session.id));
        const sessions = state.sessions.filter((session) => session.id !== sessionId && !sideChats.has(session.id));
        const composerDrafts = { ...state.composerDrafts };
        delete composerDrafts[`session:${sessionId}`];
        const composerContexts = { ...state.composerContexts }; delete composerContexts[`session:${sessionId}`];
        const contextTexts = { ...state.contextTexts }; delete contextTexts[`session:${sessionId}`];
        const contextTextStatus = { ...state.contextTextStatus }; delete contextTextStatus[`session:${sessionId}`];
        const terminalWorkspacesBySession = { ...state.terminalWorkspacesBySession }; delete terminalWorkspacesBySession[sessionId];
        const localServersBySession = { ...state.localServersBySession }; delete localServersBySession[sessionId];
        const selectedFileBySession = { ...state.selectedFileBySession }; delete selectedFileBySession[sessionId];
        const sideParent = state.sessions.find((session) => session.id === sessionId)?.sideChat?.parentSessionId;
        const dockPanes = state.dockPanes.filter((pane) => !((pane.kind === "editor" || pane.kind === "review" || pane.kind === "sidechat" || pane.kind === "subagent") && pane.sessionId === sessionId) &&
          !(pane.kind === "sidechat" && pane.sessionId === sideParent) && (!pane.document || (pane.document.scope !== `session:${sessionId}` && pane.document.owner !== `session:${sessionId}`)));
        const dockActivePaneId = dockPanes.some((pane) => pane.id === state.dockActivePaneId) ? state.dockActivePaneId : (dockPanes[dockPanes.length - 1]?.id ?? null);
        const editorBuffers = Object.fromEntries(Object.entries(state.editorBuffers).filter(([key]) => !key.startsWith(`${sessionId}:`)));
        const pullRequestsBySession = { ...state.pullRequestsBySession }; delete pullRequestsBySession[sessionId];
        const browserBySession = { ...state.browserBySession }; delete browserBySession[sessionId];
        const browserHistoryBySession = { ...state.browserHistoryBySession }; delete browserHistoryBySession[sessionId];
        const promptQueues = { ...state.promptQueues }; delete promptQueues[sessionId];
        const loadedTranscripts = { ...state.loadedTranscripts }; delete loadedTranscripts[sessionId];
        return { sessions, promptQueues, loadedTranscripts, unseenSessionIds: state.unseenSessionIds.filter((id) => id !== sessionId), settings: pruneSidebarSettings(state.settings, state.projects, sessions), composerDrafts, composerContexts, contextTexts, contextTextStatus, pullRequestsBySession, browserBySession, browserHistoryBySession, terminalWorkspacesBySession, localServersBySession, selectedFileBySession, dockPanes, dockActivePaneId, editorBuffers, ...(state.selectedSessionId === sessionId ? {
          selectedSessionId: sessions.find((session) => session.projectId === state.selectedProjectId && !isAuxiliarySession(session))?.id ?? null,
          gitStatus: null, selectedDiff: null, diffText: null,
        } : {}) };
      });
      await get().refreshGitStatus();
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  respondAgentRequest: async (request) => {
    try { await client.respondAgentRequest(request); set({ error: null }); return true; }
    catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  stopAgent: async (target) => {
    const sessionId = target ?? get().selectedSessionId;
    if (!sessionId) return;
    get().pausePromptQueue(sessionId);
    try { await client.stopAgent(sessionId); } catch (error) { set({ error: formatUnknownError(error) }); }
  },

  loadGithubInbox: async (kind, state) => {
    const key = `${kind}:${state}`;
    if (get().githubInbox[key]?.loading) return;
    if (key === "pullRequest:open") lastOpenPullsRead = Date.now();
    set((current) => ({ githubInbox: { ...current.githubInbox, [key]: { data: current.githubInbox[key]?.data ?? null, loading: true, error: null } } }));
    try {
      const response = await client.pullRequestAction({ type: "list", kind, state });
      const data = response.type === "inbox" ? response : null;
      set((current) => ({ githubInbox: { ...current.githubInbox, [key]: { data, loading: false, error: null } } }));
      if (data && kind === "pullRequest" && state === "open") announcePullRequests(data);
      // The first list that loads marks everything already there as seen (no flood of unread dots).
      const { ready, settings, saveSettings } = get();
      if (data && ready && settings.githubSeenFloor === null && !data.failures.some((failure) => failure.status === "cliMissing" || failure.status === "authRequired"))
        void saveSettings({ ...settings, githubSeenFloor: githubSeenFloor(data.items, null) });
    } catch (error) {
      set((current) => ({ githubInbox: { ...current.githubInbox, [key]: { data: current.githubInbox[key]?.data ?? null, loading: false, error: formatUnknownError(error) } } }));
    }
  },

  ciAutofixAction: async (action) => {
    try {
      set({ ciAutoFix: await client.ciAutofixAction(action) });
    } catch (error) { set({ error: formatUnknownError(error) }); }
  },

  prWatchAction: async (action) => {
    try {
      set({ prWatches: await client.prWatchAction(action) });
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  pullRequestAction: async (action) => {
    try { return await client.pullRequestAction(action); }
    catch (error) { set({ error: formatUnknownError(error) }); return null; }
  },

  openPullAsk: async (item, body) => {
    try {
      const session = await client.pullAskAction({ type: "open", repository: item.repository, number: item.number, kind: item.kind, context: pullAskContext(item, body) });
      const owner = `session:${session.id}`;
      set((state) => ({
        sessions: state.sessions.some((row) => row.id === session.id) ? state.sessions.map((row) => row.id === session.id ? { ...row, pullAsk: session.pullAsk } : row) : [session, ...state.sessions],
        // Every turn runs read-only natively; the composer shows planning where the provider has it.
        composerContexts: state.composerContexts[owner] ? state.composerContexts : { ...state.composerContexts, [owner]: { ...emptyComposerContext, planning: supportsPlanning(session.agent, session.model ?? null) } },
      }));
      void get().ensureTranscript(session.id);
      return get().sessions.find((row) => row.id === session.id) ?? session;
    } catch (error) { set({ error: formatUnknownError(error) }); return null; }
  },

  openSideChat: async (parentSessionId, quote) => {
    try {
      const side = await client.sideChatAction({ type: "open", parentSessionId });
      showSideChat(parentSessionId, side, quote);
      return true;
    } catch (error) { set({ error: formatUnknownError(error) }); return false; }
  },

  toggleSideChat: () => {
    const state = get();
    const parent = state.selectedSessionId;
    if (!parent || state.mainView !== "session") return;
    const pane = state.dockPanes.find((item) => item.kind === "sidechat" && item.sessionId === parent);
    if (pane && state.dockOpen && state.dockActivePaneId === pane.id) {
      state.closeDockPane(pane.id);
      requestAnimationFrame(() => focusComposer(`session:${parent}`));
      return;
    }
    void state.openSideChat(parent);
  },

  teamAction: async (action) => {
    try {
      const response = await client.teamAction(action);
      set((state) => {
        const removed = new Set(response.removed);
        const returned = new Map(response.sessions.map((session) => [session.id, session]));
        const known = new Set(state.sessions.map((session) => session.id));
        const kept = state.sessions.filter((session) => !removed.has(session.id)).map((session) => {
          const next = returned.get(session.id);
          return next ? { ...next, pinnedMessageIds: session.pinnedMessageIds ?? next.pinnedMessageIds } : session;
        });
        // Team actions return complete sessions.
        const loadedTranscripts = { ...state.loadedTranscripts };
        for (const session of response.sessions) loadedTranscripts[session.id] = ++transcriptTick;
        for (const id of removed) delete loadedTranscripts[id];
        return { error: null, sessions: [...response.sessions.filter((session) => !known.has(session.id)), ...kept], loadedTranscripts };
      });
      return true;
    } catch (error) {
      set({ error: formatUnknownError(error) });
      return false;
    }
  },

  refreshGitStatus: async () => {
    // Selections, commits and pushes all pass here; the project totals follow.
    scheduleProjectDiffs();
    const sequence = ++gitRequestSequence;
    const sessionId = get().selectedSessionId;
    if (!sessionId) {
      set({ gitStatus: null });
      return;
    }
    try {
      const gitStatus = await client.gitStatus(sessionId);
      if (sequence === gitRequestSequence && get().selectedSessionId === sessionId) set({ gitStatus });
    } catch (error) {
      if (sequence === gitRequestSequence && get().selectedSessionId === sessionId) set({ gitStatus: null, error: formatUnknownError(error) });
    }
  },

  loadDiff: async (change) => {
    const sequence = ++diffRequestSequence;
    const sessionId = get().selectedSessionId;
    if (!sessionId) return;
    set({ selectedDiff: change, diffText: null });
    get().openDockPane("changes");
    try {
      const diffText = await client.gitDiff(sessionId, change.path);
      if (sequence === diffRequestSequence && get().selectedSessionId === sessionId && get().selectedDiff?.path === change.path) set({ diffText });
    } catch (error) { set({ error: formatUnknownError(error) }); }
  },

  setSidebarWidth: (width) => set({ sidebarWidth: clampConversationListWidth(width) }),
  resizeSidebar: (delta) =>
    set((state) => ({ sidebarWidth: clampConversationListWidth(state.sidebarWidth + delta) })),
  toggleSidebar: () => {
    const sidebarCollapsed = !get().sidebarCollapsed;
    set({ sidebarCollapsed });
    void get().saveSettings({ ...get().settings, sidebarCollapsed });
  },
  setDraftIsolated: (isolated) => set({ draftIsolated: isolated, draftWorktreeOf: null }),
  setDockWidth: (width) => set((state) => ({ dockWidth: clampDockWidth(width, state) })),
  toggleDock: () => set((state) => ({
    dockOpen: !state.dockOpen,
    dockMaximized: state.dockOpen ? false : state.dockMaximized,
    ...(state.dockOpen ? {} : { dockWidth: clampDockWidth(state.dockWidth, state) }),
  })),
  toggleDockMaximized: () => set((state) => ({ dockMaximized: !state.dockMaximized, dockOpen: true })),
  keepTurnChanges: async (sessionId, messageId) => {
    const review = await client.keepTurnChanges(sessionId, messageId);
    set((state) => ({ sessions: state.sessions.map((session) => session.id === sessionId ? {
      ...session, messages: session.messages.map((message) => message.id === messageId && message.activity?.review ? {
        ...message, activity: { ...message.activity, review: { ...message.activity.review, keptAt: review.keptAt } },
      } : message),
    } : session) }));
  },
  openSubagentView: (sessionId, messageId, item) => {
    set((state) => {
      if (state.selectedSessionId !== sessionId) return state;
      const existing = state.dockPanes.find((pane) => pane.kind === "subagent" && pane.sessionId === sessionId && pane.subagent?.itemId === item.id && pane.subagent.messageId === messageId);
      const pane: DockPane = existing ?? { id: newId(), kind: "subagent", sessionId, subagent: { messageId, itemId: item.id, key: item.key, label: item.label } };
      const panes = existing ? state.dockPanes : [...state.dockPanes, pane];
      const views = panes.filter((entry) => entry.kind === "subagent");
      return { dockPanes: views.length > 6 ? panes.filter((entry) => entry !== views[0]) : panes,
        dockActivePaneId: pane.id, dockOpen: true, dockWidth: clampDockWidth(state.dockWidth, state) };
    });
  },
  openTurnReview: (sessionId, messageId, path) => {
    set((state) => {
      const session = state.sessions.find((item) => item.id === sessionId);
      const review = session?.messages.find((m) => m.id === messageId && m.sessionId === sessionId && !m.streaming && m.activity?.endedAt != null)?.activity?.review;
      if (state.selectedSessionId !== sessionId || !review || (path && !review.files.some((f) => f.path === path))) return state;
      const existing = state.dockPanes.find((pane) => pane.kind === "review" && pane.sessionId === sessionId && pane.review?.messageId === messageId);
      const pane: DockPane = { ...(existing ?? { id: newId(), kind: "review" as const, sessionId }), review: { messageId, path: path ?? review.files[0]?.path } };
      const panes = existing ? state.dockPanes.map((item) => item.id === pane.id ? pane : item) : [...state.dockPanes, pane];
      const reviews = panes.filter((item) => item.kind === "review");
      return { dockPanes: reviews.length > 8 ? panes.filter((item) => item !== reviews[0]) : panes,
        dockActivePaneId: pane.id, dockOpen: true, dockWidth: clampDockWidth(state.dockWidth, state) };
    });
  },
  attachmentModal: null,
  closeAttachmentModal: () => set({ attachmentModal: null }),
  openSentAttachment: async (owners, attachment) => {
    if (!attachment.id || attachment.kind !== "file") return "attachment.sentUnavailable";
    // The file is held but cannot be read (encoding, size, format): that reason, not "gone".
    let failure: string | null = null;
    for (const scope of owners) {
      try {
        await client.attachmentPreview(scope, attachment.id);
        set({ attachmentModal: { scope, attachmentId: attachment.id, sent: { name: attachment.name, size: attachment.size, mimeType: attachment.mimeType } } });
        return null;
      } catch (error) {
        const reason = documentError(error);
        if (reason !== "reader.document_unavailable") failure ??= reason;
      }
    }
    return failure ?? "attachment.sentUnavailable";
  },
  openAttachmentReader: (scope, attachment) => {
    const state = get();
    if (!hasContextOwner(scope, state.projects, state.sessions) || !canPreviewAttachment(attachment)
      || !state.composerContexts[scope]?.attachments.some((file) => file.id === attachment.id)) return;
    // Attachments open centered over the app; the dock reader stays for older panes.
    set({ attachmentModal: { scope, attachmentId: attachment.id } });
  },
  openDockPane: (kind, path) => {
    if (kind === "document" || kind === "review" || kind === "subagent") return;
    if (kind === "sidechat") {
      const parent = get().selectedSessionId;
      if (parent) void get().openSideChat(parent);
      return;
    }
    const sessionId = get().selectedSessionId ?? undefined;
    let created: string | null = null;
    set((state) => {
      const existing =
        kind === "editor"
          ? state.dockPanes.find((pane) => pane.kind === "editor" && pane.path === path && pane.sessionId === sessionId)
          : state.dockPanes.find((pane) => pane.kind === kind);
      if (existing) return { dockOpen: true, dockActivePaneId: existing.id, dockWidth: clampDockWidth(state.dockWidth, state) };
      created = newId();
      const pane: DockPane = { id: created, kind, path, sessionId };
      const panes = [...state.dockPanes, pane];
      // Editor panes are per file; keep the working set bounded.
      const editors = panes.filter((item) => item.kind === "editor");
      const bounded = editors.length > 8 ? panes.filter((item) => item !== editors[0]) : panes;
      return { dockPanes: bounded, dockOpen: true, dockActivePaneId: created, dockWidth: clampDockWidth(state.dockWidth, state) };
    });
    if (kind === "terminal" && sessionId) get().ensureTerminal(sessionId);
  },
  closeDockPane: (paneId) =>
    set((state) => {
      const index = state.dockPanes.findIndex((pane) => pane.id === paneId);
      if (index === -1) return state;
      const dockPanes = state.dockPanes.filter((pane) => pane.id !== paneId);
      const active = state.dockActivePaneId === paneId
        ? (dockPanes[Math.min(index, dockPanes.length - 1)]?.id ?? null)
        : state.dockActivePaneId;
      // Document panes depend on attachments that may be released; they are not remembered.
      const { id, kind, sessionId, path, review } = state.dockPanes[index];
      const closedItems = kind === "document" || kind === "subagent" ? state.closedItems : pushClosed(state.closedItems, [{ kind: "dock", pane: { id, kind, sessionId, path, review }, index }]);
      return { dockPanes, dockActivePaneId: active, closedItems };
    }),
  setActiveDockPane: (paneId) => set({ dockActivePaneId: paneId }),
  ensureTerminal: (sessionId) => {
    const workspace = get().terminalWorkspacesBySession[sessionId];
    if (workspace?.panes.length) return;
    set((state) => withWorkspace(state, sessionId, { split: "columns", panes: [newPane(nextTerminal(undefined))] }));
  },
  addTerminalTab: (sessionId, paneId) =>
    set((state) => {
      const workspace = state.terminalWorkspacesBySession[sessionId];
      if (!workspace) {
        return withWorkspace(state, sessionId, { split: "columns", panes: [newPane(nextTerminal(undefined))] });
      }
      if (terminalCount(workspace) >= MAX_TERMINALS_PER_SESSION) return state;
      if (!workspace.panes.some((pane) => pane.id === paneId)) return state;
      const terminal = nextTerminal(workspace);
      return withWorkspace(state, sessionId, {
        ...workspace,
        panes: workspace.panes.map((pane) => pane.id === paneId
          ? { ...pane, terminals: [...pane.terminals, terminal], activeTerminalId: terminal.id }
          : pane),
      });
    }),
  setActiveTerminal: (sessionId, paneId, terminalId) =>
    set((state) => {
      const workspace = state.terminalWorkspacesBySession[sessionId];
      const pane = workspace?.panes.find((item) => item.id === paneId);
      if (!workspace || !pane?.terminals.some((terminal) => terminal.id === terminalId)) return state;
      return withWorkspace(state, sessionId, {
        ...workspace,
        panes: workspace.panes.map((item) => item.id === paneId ? { ...item, activeTerminalId: terminalId } : item),
      });
    }),
  closeTerminal: (sessionId, terminalId) =>
    set((state) => {
      const workspace = state.terminalWorkspacesBySession[sessionId];
      const pane = workspace?.panes.find((item) => item.terminals.some((terminal) => terminal.id === terminalId));
      if (!workspace || !pane) return state;
      const terminals = pane.terminals.filter((terminal) => terminal.id !== terminalId);
      const panes = terminals.length === 0
        ? workspace.panes.filter((item) => item.id !== pane.id)
        : workspace.panes.map((item) => item.id === pane.id
          ? { ...item, terminals, activeTerminalId: item.activeTerminalId === terminalId ? terminals[terminals.length - 1].id : item.activeTerminalId }
          : item);
      if (panes.length === 0) {
        const terminalWorkspacesBySession = { ...state.terminalWorkspacesBySession };
        delete terminalWorkspacesBySession[sessionId];
        return { terminalWorkspacesBySession };
      }
      return withWorkspace(state, sessionId, { ...workspace, panes });
    }),
  splitTerminalPane: (sessionId, paneId, split) =>
    set((state) => {
      const workspace = state.terminalWorkspacesBySession[sessionId];
      if (!workspace || terminalCount(workspace) >= MAX_TERMINALS_PER_SESSION) return state;
      const index = workspace.panes.findIndex((pane) => pane.id === paneId);
      if (index === -1) return state;
      const panes = [
        ...workspace.panes.slice(0, index + 1),
        newPane(nextTerminal(workspace)),
        ...workspace.panes.slice(index + 1),
      ];
      return withWorkspace(state, sessionId, { split, panes });
    }),
  moveTerminalToOwnPane: (sessionId, paneId) =>
    set((state) => {
      const workspace = state.terminalWorkspacesBySession[sessionId];
      const pane = workspace?.panes.find((item) => item.id === paneId);
      if (!workspace || !pane || pane.terminals.length < 2) return state;
      const index = workspace.panes.findIndex((item) => item.id === paneId);
      const moved = pane.terminals.find((terminal) => terminal.id === pane.activeTerminalId) ?? pane.terminals[pane.terminals.length - 1];
      const remaining = pane.terminals.filter((terminal) => terminal.id !== moved.id);
      const panes = [
        ...workspace.panes.slice(0, index),
        { ...pane, terminals: remaining, activeTerminalId: remaining[remaining.length - 1].id },
        newPane(moved),
        ...workspace.panes.slice(index + 1),
      ];
      return withWorkspace(state, sessionId, { ...workspace, panes });
    }),
  noteLocalServers: (sessionId, urls) => {
    if (!urls.length) return;
    const current = get().localServersBySession[sessionId] ?? [];
    const merged = [...new Set([...current, ...urls])].slice(0, 8);
    if (merged.length === current.length) return;
    set((state) => ({ localServersBySession: { ...state.localServersBySession, [sessionId]: merged } }));
  },
  setSelectedFile: (sessionId, path) =>
    set((state) => ({ selectedFileBySession: { ...state.selectedFileBySession, [sessionId]: path } })),
  setEditorBuffer: (key, content, saved) =>
    set((state) => ({
      editorBuffers: { ...state.editorBuffers, [key]: { identity: state.editorBuffers[key]?.identity ?? newId(), content, saved: saved ?? state.editorBuffers[key]?.saved ?? content } },
    })),
  saveEditorBuffer: async (sessionId, path) => {
    const key = editorKey(sessionId, path);
    const pending = get().editorBuffers[key];
    if (get().editorSaving[key] || !pending || pending.content === pending.saved) return;
    // One shared gate survives file switches and multiple views of the same buffer.
    set(state => ({ editorSaving: { ...state.editorSaving, [key]: true } }));
    try {
      await client.writeTextFile(sessionId, path, pending.content);
      set(state => {
        const current = state.editorBuffers[key];
        if (!current || current.identity !== pending.identity) return state;
        return { editorBuffers: { ...state.editorBuffers, [key]: acknowledgeEditorSave(current, pending.content) } };
      });
    } finally {
      set(state => {
        const editorSaving = { ...state.editorSaving };
        delete editorSaving[key];
        return { editorSaving };
      });
    }
  },
  discardEditorBuffer: (key) =>
    set((state) => {
      if (!(key in state.editorBuffers) || state.editorSaving[key]) return state;
      const editorBuffers = { ...state.editorBuffers };
      delete editorBuffers[key];
      return { editorBuffers };
    }),
  loadEditors: async () => {
    if (get().editors) return;
    // A server has no editor apps to open (ADR-131, ADR-133).
    if (!hostFeatures(get().hostInfo).openInEditor) { set({ editors: [] }); return; }
    try {
      const editors = await client.detectEditors();
      set({ editors });
    } catch (error) {
      set({ error: formatUnknownError(error) });
      return;
    }
    try {
      const icons = await client.editorAppIcons();
      set({ editorIcons: Object.fromEntries(icons.filter((icon) => icon.png).map((icon) => [icon.id, icon.png])) });
    } catch {
      // Real app icons are decorative; the list keeps working without them.
    }
  },
  pullRequestsBySession: {},
  refreshPullRequest: async (sessionId, force = false) => {
    const session = get().sessions.find(row => row.id === sessionId);
    if (!session || !get().projects.some(project => project.id === session.projectId)) return;
    const workspacePath = session.worktree.path;
    const existing = get().pullRequestsBySession[sessionId];
    const actualBranch = get().selectedSessionId === sessionId ? get().gitStatus?.identity.branch : undefined;
    if (!force && existing?.workspacePath === workspacePath && existing.snapshot && !existing.error &&
      (!actualBranch || existing.snapshot.branch === actualBranch) && Date.now() - Date.parse(existing.snapshot.checkedAt) < 60_000) return;
    const requestBranch = actualBranch ?? session.worktree.branch;
    const pending = pullRequestRequests.get(sessionId);
    if (pending) {
      if (pending.workspacePath === workspacePath && pending.branch === requestBranch) return pending.promise;
      await pending.promise;
      return get().refreshPullRequest(sessionId, true);
    }
    const owns = () => get().sessions.some(row => row.id === sessionId && row.projectId === session.projectId && row.worktree.path === workspacePath && get().projects.some(project => project.id === row.projectId));
    set(state => ({ pullRequestsBySession: { ...state.pullRequestsBySession, [sessionId]: { workspacePath, loading: true, error: null, snapshot: null } } }));
    const request = (async () => {
      try {
        const snapshot = await client.sessionPullRequest(sessionId);
        if (!owns()) return;
        if (snapshot.sessionId !== sessionId) throw new Error("Could not load the pull request. Check GitHub CLI access and refresh.");
        // Native revalidation is newer than a possibly stale frontend Git snapshot.
        set(state => ({ pullRequestsBySession: { ...state.pullRequestsBySession, [sessionId]: { workspacePath, loading: false, error: null, snapshot } } }));
      } catch (error) {
        if (owns()) set(state => ({ pullRequestsBySession: { ...state.pullRequestsBySession, [sessionId]: { workspacePath, loading: false, error: formatUnknownError(error), snapshot: null } } }));
      }
    })().finally(() => { if (pullRequestRequests.get(sessionId)?.promise === request) pullRequestRequests.delete(sessionId); });
    pullRequestRequests.set(sessionId, { promise: request, workspacePath, branch: requestBranch });
    return request;
  },
  loadProjectRemote: async (projectId) => {
    if (projectId in get().remoteUrlByProject) return;
    try {
      const remoteUrl = await client.projectRemoteUrl(projectId);
      set((state) => ({ remoteUrlByProject: { ...state.remoteUrlByProject, [projectId]: remoteUrl } }));
    } catch (error) {
      set((state) => ({ remoteUrlByProject: { ...state.remoteUrlByProject, [projectId]: null } }));
      set({ error: formatUnknownError(error) });
    }
  },
  setPaletteOpen: (open) => set({ paletteOpen: open }),
  setSettingsOpen: (open) =>
    set({ settingsOpen: open }),
  setSettingsSection: (section) => set({ settingsSection: section }),
  requestNewSession: () => {
    get().setSettingsOpen(false);
    const { settings, selectedProjectId } = get();
    // A project needs to be chosen before there is a landing to compose in.
    if (!selectedProjectId) { set({ newSessionOpen: true }); return; }
    // New thread opens the empty landing; the session is created on first send.
    set({
      newSessionOpen: false,
      selectedSessionId: null,
      mainView: "session",
      draftIsolated: settings.defaultSessionWorkspace === "worktree",
      draftWorktreeOf: null,
      draftTemporary: false,
    });
  },
  requestNewSessionIn: async (sessionId) => {
    const source = get().sessions.find((session) => session.id === sessionId);
    if (!source) return;
    if (get().selectedProjectId !== source.projectId) await get().selectProject(source.projectId);
    get().requestNewSession();
    // An isolated worktree is shared; the project checkout is just "Local".
    set(source.worktree.isolated ? { draftIsolated: false, draftWorktreeOf: source.id } : { draftIsolated: false });
  },
  setNewSessionOpen: (open) => set({ newSessionOpen: open }),
  saveSettings: async (settings) => {
    const previous = get().settings;
    const next = shareSettings(previous, mergeSettings(settings));
    const pathsChanged = JSON.stringify(previous.providerPaths) !== JSON.stringify(next.providerPaths);
    set({ settings: next, activityNotifications: retainActivityNotifications(get().activityNotifications, next.notifications), ...(pathsChanged ? { modelsByProvider: {}, usageByProvider: {} } : {}) });
    settingsSavesPending += 1;
    const task = settingsSaveQueue.catch(() => undefined).then(async () => {
      try {
        const saved = await client.saveSettings(next);
        if (get().settings === next) set({ settings: shareSettings(next, mergeSettings(saved)) });
        if (pathsChanged) await get().refreshAgents();
      } catch (error) {
        if (get().settings === next) set({ settings: previous });
        set({ error: formatUnknownError(error) });
      } finally {
        settingsSavesPending -= 1;
      }
    });
    settingsSaveQueue = task;
    await task;
  },

  setSessionAgent: async (agent) => {
    await get().setSessionModel(agent, null);
  },

  setSessionModel: async (provider, model, target) => {
    const sequence = ++modelSelectionSequence;
    const sessionId = target ?? get().selectedSessionId;
    const defaultModel = model ? modelKey(provider, model) : null;
    if (!sessionId) {
      await get().saveSettings({ ...get().settings, defaultAgent: provider, defaultModel });
      return;
    }
    set((state) => ({ modelChangesPending: { ...state.modelChangesPending, [sessionId]: (state.modelChangesPending[sessionId] ?? 0) + 1 } }));
    const task = (modelSelectionQueues.get(sessionId) ?? Promise.resolve()).catch(() => undefined).then(async () => {
      try {
        const session = await client.setSessionModel(sessionId, provider, model);
        set((state) => ({
          sessions: state.sessions.map((item) => item.id === session.id ? { ...item, agent: session.agent, model: session.model, providerAccountId: session.providerAccountId, accountBindings: session.accountBindings, forkOrigin: session.forkOrigin, nativeThread: session.nativeThread, pendingRequests: session.pendingRequests, handoff: session.handoff, contextUsage: session.contextUsage, lastActivityAt: session.lastActivityAt } : item),
        }));
        if (!target && sequence === modelSelectionSequence) await get().saveSettings({ ...get().settings, defaultAgent: provider, defaultModel });
      } catch (error) {
        set({ error: formatUnknownError(error) });
      } finally {
        set((state) => {
          const modelChangesPending = { ...state.modelChangesPending };
          if ((modelChangesPending[sessionId] ?? 0) <= 1) delete modelChangesPending[sessionId];
          else modelChangesPending[sessionId] -= 1;
          return { modelChangesPending };
        });
      }
    });
    modelSelectionQueues.set(sessionId, task);
    await task;
    if (modelSelectionQueues.get(sessionId) === task) modelSelectionQueues.delete(sessionId);
  },

  loadProviderModels: async (id, force = false) => {
    if (!force && get().modelsByProvider[id]) return;
    const pending = modelRequests.get(id);
    if (pending) { await pending; if (force) await get().loadProviderModels(id, true); return; }
    const paths = get().settings.providerPaths;
    const request = (async () => {
      try {
        const list = await client.listProviderModels(id, force);
        if (get().settings.providerPaths === paths) set((state) => ({ modelsByProvider: { ...state.modelsByProvider, [id]: list } }));
      } catch (error) { set({ error: formatUnknownError(error) }); }
    })();
    modelRequests.set(id, request);
    await request;
    if (modelRequests.get(id) === request) modelRequests.delete(id);
  },

  loadAllModels: async (force = false) => {
    await Promise.all(PROVIDERS.map((item) => get().loadProviderModels(item.id, force)));
  },

  toggleModelFavorite: (provider, modelId) => {
    const settings = get().settings;
    const key = modelKey(provider, modelId);
    const favoriteModels = settings.favoriteModels.includes(key)
      ? settings.favoriteModels.filter((item) => item !== key)
      : [...settings.favoriteModels, key];
    void get().saveSettings({ ...settings, favoriteModels });
  },

}));

// Header tabs: a project's tabs start as its sessions (sidebar order); any selection opens its tab;
// the landing shows a blank tab that the first send turns into the new session; finishing out of view marks it unseen.
useAppStore.subscribe((state, previous) => {
  // Keystrokes and other unrelated updates change none of these; skip the per-session scans.
  if (state.sessions === previous.sessions && state.projects === previous.projects && state.mainView === previous.mainView
    && state.selectedProjectId === previous.selectedProjectId && state.selectedSessionId === previous.selectedSessionId
    && state.unseenSessionIds === previous.unseenSessionIds && state.openTabsByProject === previous.openTabsByProject
    && state.draftTabByProject === previous.draftTabByProject && state.lastActiveTabByProject === previous.lastActiveTabByProject) return;
  const finished = finishedUnseen(previousStatuses, state.sessions, state.mainView === "session" ? state.selectedSessionId : null);
  previousStatuses = new Map(state.sessions.map(session => [session.id, session.status]));
  const created = state.sessions.filter(session => !previousSessionIds.has(session.id)).map(session => session.id);
  const firstLoad = previousSessionIds.size === 0;
  previousSessionIds = new Set(state.sessions.map(session => session.id));
  const projectId = state.selectedProjectId;
  const known = !!projectId && state.projects.some(project => project.id === projectId);
  const selected = state.mainView === "session" ? state.sessions.find(session => session.id === state.selectedSessionId && session.projectId === projectId) : undefined;
  let tabs = state.openTabsByProject, drafts = state.draftTabByProject, active = state.lastActiveTabByProject;
  if (known && !tabs[projectId!]) {
    const groups = sidebarGroups(state.projects, state.sessions, state.settings, { unseen: new Set(state.unseenSessionIds) });
    // Oldest on the left, so every new thread is born last, at the right end.
    tabs = { ...tabs, [projectId!]: [...groups.pinned, ...groups.nested].filter(session => session.projectId === projectId).map(session => session.id).slice(0, TAB_LIMIT).reverse() };
  }
  if (selected) {
    const current = tabs[selected.projectId] ?? [];
    if (!current.includes(selected.id)) tabs = { ...tabs, [selected.projectId]: openTabRule(current, selected.id) };
    // A session born from the landing takes over the blank tab.
    if (drafts[selected.projectId] && !firstLoad && created.includes(selected.id)) drafts = { ...drafts, [selected.projectId]: false };
    if (active[selected.projectId] !== selected.id) active = { ...active, [selected.projectId]: selected.id };
  }
  if (known && state.mainView === "session" && !state.selectedSessionId && !drafts[projectId!]) drafts = { ...drafts, [projectId!]: true };
  // A conversation back from snooze (ADR-103) is unread and gets its tab back; opening it
  // acknowledges the reminder natively, so it is unread again after a restart until then.
  const returned: string[] = [];
  for (const session of state.sessions) {
    if (!session.snoozeReminderAt || session.snoozedUntil) continue;
    const key = `${session.id}:${session.snoozeReminderAt}`;
    if (selected?.id === session.id) {
      if (!acknowledgedReminders.has(key)) { acknowledgedReminders.add(key); void useAppStore.getState().snoozeSession(session.id, null); }
      continue;
    }
    if (noticedReminders.has(key)) continue;
    noticedReminders.add(key);
    returned.push(session.id);
    if (!firstLoad && !isAuxiliarySession(session)) tabs = { ...tabs, [session.projectId]: openTabRule(tabs[session.projectId] ?? [], session.id, selected?.id ?? null) };
  }
  const seen = !!selected && state.unseenSessionIds.includes(selected.id);
  const unread = [...finished, ...returned];
  if (tabs === state.openTabsByProject && drafts === state.draftTabByProject && active === state.lastActiveTabByProject && !unread.length && !seen) return;
  useAppStore.setState((current) => ({
    openTabsByProject: tabs,
    draftTabByProject: drafts,
    lastActiveTabByProject: active,
    unseenSessionIds: unread.length || seen ? [...new Set([...current.unseenSessionIds, ...unread])].filter(id => id !== selected?.id) : current.unseenSessionIds,
  }));
});

// Browser-style history: every selection change records one bounded entry.
useAppStore.subscribe((state) => {
  const key = navKeyOf(currentNavEntry(state));
  if (key === lastNavKey) return;
  lastNavKey = key;
  if (!navApplying) recordNavigation();
});

/** Prompt-triggered browser context: a screenshot of the active tab attaches to the turn. */
async function captureBrowserAttachment(prompt: string, sessionId: string | null) {
  if (!sessionId || !promptWantsBrowserCapture(prompt)) return null;
  const browser = useAppStore.getState().browserBySession[sessionId];
  const active = browser?.tabs.find((tab) => tab.id === browser.activeTabId);
  if (!active?.url || !/^https?:\/\//i.test(active.url)) return null;
  try {
    const png = await client.browserCapture(sessionId, active.id);
    const [attachment] = await client.pastePromptAttachments(`session:${sessionId}`, [{ name: "browser.png", data: png }]);
    return attachment ?? null;
  } catch {
    return null;
  }
}

/** A closed thing can come back while its session (and project tab owner) still exists. */
function closedItemReopenable(state: AppStore, item: ClosedItem): boolean {
  const alive = (id: string | undefined) => !!id && state.sessions.some((session) => session.id === id);
  if (item.kind === "session") return alive(item.sessionId) && state.projects.some((project) => project.id === item.projectId);
  if (item.kind === "browser") return alive(item.sessionId);
  // Editor, review and side chat panes belong to their session; the others follow the selection.
  return item.pane.kind === "editor" || item.pane.kind === "review" || item.pane.kind === "sidechat" ? alive(item.pane.sessionId) : true;
}

/** Restores one closed thing where it was (ADR-102). */
function reopenItem(item: ClosedItem) {
  const store = useAppStore.getState();
  if (item.kind === "session") {
    const tabs = (store.openTabsByProject[item.projectId] ?? []).filter((id) => id !== item.sessionId);
    useAppStore.setState({ openTabsByProject: { ...store.openTabsByProject, [item.projectId]: insertAt(tabs, item.sessionId, item.index) } });
    void store.selectSession(item.sessionId);
    return;
  }
  if (item.kind === "browser") {
    void (async () => {
      if (!useAppStore.getState().browserBySession[item.sessionId]?.open && !(await useAppStore.getState().openBrowser(item.sessionId))) return;
      if (!(await useAppStore.getState().browserNewTab(item.sessionId, item.url))) return;
      if (useAppStore.getState().selectedSessionId === item.sessionId) useAppStore.getState().openDockPane("browser");
    })();
    return;
  }
  const closed = item.pane;
  if (closed.kind === "sidechat" && closed.sessionId) { void store.openSideChat(closed.sessionId); return; }
  useAppStore.setState((state) => {
    const existing = state.dockPanes.find((pane) => pane.kind === closed.kind && (
      closed.kind === "editor" ? pane.path === closed.path && pane.sessionId === closed.sessionId
        : closed.kind === "review" ? pane.sessionId === closed.sessionId && pane.review?.messageId === closed.review?.messageId : true));
    const pane: DockPane = existing ?? { ...closed, id: newId() };
    const panes = existing ? state.dockPanes : insertAt(state.dockPanes, pane, item.index);
    // Same bound as openDockPane: at most eight editor panes, dropping the oldest other one.
    const editors = panes.filter((entry) => entry.kind === "editor");
    const bounded = editors.length > 8 ? panes.filter((entry) => entry !== editors.find((editor) => editor !== pane)) : panes;
    return { dockPanes: bounded, dockOpen: true, dockActivePaneId: pane.id, dockWidth: clampDockWidth(state.dockWidth, state) };
  });
  const sessionId = useAppStore.getState().selectedSessionId;
  if (closed.kind === "terminal" && sessionId) useAppStore.getState().ensureTerminal(sessionId);
}

/**
 * Shows a side chat in the right dock beside its parent (ADR-049). Without a
 * remembered composer mode it starts read-only (planning) where supported.
 */
function showSideChat(parentSessionId: string, side: Session, quote?: string) {
  const owner = `session:${side.id}`;
  useAppStore.setState((state) => {
    const known = state.sessions.some((session) => session.id === side.id);
    // Composer modes are memory-only, so a side chat reopens read-only after a restart too.
    const composerContexts = !state.composerContexts[owner]
      ? { ...state.composerContexts, [owner]: { ...emptyComposerContext, planning: supportsPlanning(side.agent, side.model ?? null) } }
      : state.composerContexts;
    const existing = state.dockPanes.find((pane) => pane.kind === "sidechat" && pane.sessionId === parentSessionId);
    const pane: DockPane = existing ?? { id: newId(), kind: "sidechat", sessionId: parentSessionId };
    return { sessions: known ? state.sessions : [side, ...state.sessions], composerContexts,
      dockPanes: existing ? state.dockPanes : [...state.dockPanes, pane], dockOpen: true, dockActivePaneId: pane.id, dockWidth: clampDockWidth(state.dockWidth, state) };
  });
  const store = useAppStore.getState();
  if (quote) {
    const next = appendTranscriptQuote(store.composerDrafts[owner] ?? "", quote);
    if (next !== null) store.setComposerDraft(owner, next);
  }
  void store.ensureTranscript(side.id);
  requestAnimationFrame(() => focusComposer(owner, { preventScroll: true, end: true }));
}

function releaseUnusedQueueAttachments(files: import("@/client/types").PromptAttachment[], owner: string) {
  const state = useAppStore.getState();
  for (const file of files) {
    const used = Object.values(state.composerContexts).some(context => context.attachments.some(reference => reference.id === file.id)) || Object.values(state.promptQueues).some(queue => queue.items.some(item => item.context.attachments.some(reference => reference.id === file.id)));
    if (!used) void client.releasePromptAttachments(file.owner ?? owner, [file.id]).catch(() => undefined);
  }
}

/** Only native lifecycle snapshots may release the next explicitly queued send. */
export function observePromptQueue(session: Session) {
  let queue = useAppStore.getState().promptQueues[session.id];
  if (!queue || !useAppStore.getState().sessions.some(item => item.id === session.id)) return;
  const messageId = lastAssistantId(session);
  const head = queue.items[0];
  // An IPC rejection can precede delivery of its native admission snapshot.
  // Reconcile that late snapshot without ever rebasing/replaying the request.
  if (!queue.inFlight && head && observedQueuedAdmission(session, head)) {
    queue = { ...queue, items: queue.items.slice(1), waitingFor: messageId };
    const reconciled = queue;
    useAppStore.setState(state => ({ promptQueues: { ...state.promptQueues, [session.id]: reconciled } }));
  }
  if (activeSession(session)) {
    if (messageId && messageId !== queue.waitingFor) useAppStore.setState(state => ({ promptQueues: { ...state.promptQueues, [session.id]: { ...queue, waitingFor: messageId } } }));
    return;
  }
  if (!queue.waitingFor || messageId !== queue.waitingFor || session.messages.find(message => message.id === messageId)?.streaming) return;
  // A stop or a failure pauses the queue: the next request never starts on its own
  // after the person stopped the agent or the turn went wrong. Each says which it was.
  const failed = session.status !== "completed";
  const reason = session.status === "stopped" ? "queue.stopped" : "queue.failed";
  useAppStore.setState(state => ({ promptQueues: { ...state.promptQueues, [session.id]: { ...queue, waitingFor: null, paused: failed || queue.paused, reason: failed ? reason : queue.reason } } }));
  void advancePromptQueue(session.id);
}

/** Resolves once the store has seen the admitted turn start or finish (bounded to 5 s). */
function sessionMoved(sessionId: string, before: string | null) {
  const moved = () => {
    const session = useAppStore.getState().sessions.find(item => item.id === sessionId);
    return !session || activeSession(session) || lastAssistantId(session) !== before;
  };
  return new Promise<void>((resolve) => {
    if (moved()) { resolve(); return; }
    const finish = () => { clearTimeout(timer); stop(); resolve(); };
    const timer = setTimeout(finish, 5000);
    const stop = useAppStore.subscribe(() => { if (moved()) finish(); });
  });
}

async function advancePromptQueue(sessionId: string) {
  const state = useAppStore.getState();
  const queue = state.promptQueues[sessionId];
  const session = state.sessions.find(item => item.id === sessionId);
  const item = queue?.items[0];
  if (!queue || !session || !item || queue.paused || queue.inFlight || queue.waitingFor || activeSession(session)) return;
  if (state.modelChangesPending[sessionId] || !sameQueueBinding(session, item)) {
    useAppStore.setState(state => ({ promptQueues: { ...state.promptQueues, [sessionId]: { ...queue, paused: true, reason: "queue.contextChanged" } } }));
    return;
  }
  // Claim synchronously before IPC: repeated final snapshots cannot double-send.
  const predecessor = item.afterMessageId === undefined ? lastAssistantId(session) : item.afterMessageId;
  if (lastAssistantId(session) !== predecessor) {
    useAppStore.setState(state => ({ promptQueues: { ...state.promptQueues, [sessionId]: { ...queue, paused: true, reason: "queue.contextChanged" } } }));
    return;
  }
  useAppStore.setState(state => ({ promptQueues: { ...state.promptQueues, [sessionId]: { ...queue, inFlight: item.id, items: queue.items.map(entry => entry.id === item.id ? { ...entry, afterMessageId: predecessor } : entry) } } }));
  let succeeded = false;
  let response: Session | undefined;
  try {
    response = await client.sendPrompt({ sessionId, prompt: item.prompt, execution: item.execution, clientRequestId: item.clientRequestId, debugging: Boolean(item.context.debugging) && !item.context.planning, goal: item.context.goal, attachmentIds: item.context.attachments.map(file => file.id), attachmentOwner: `session:${sessionId}`, queuedAfter: { ...item.binding, messageId: predecessor } });
    succeeded = true;
  } catch (error) {
    useAppStore.setState({ error: formatUnknownError(error) });
  }
  const latest = useAppStore.getState();
  const live = latest.sessions.find(session => session.id === sessionId);
  const current = latest.promptQueues[sessionId];
  if (!live || !current || current.inFlight !== item.id) return;
  // Startup/persistence may fail after admission. Never replay an admitted turn.
  const admitted = succeeded || observedQueuedAdmission(live, { ...item, afterMessageId: predecessor });
  const waitingFor = activeSession(live) ? lastAssistantId(live) : response && lastAssistantId(live) === predecessor ? lastAssistantId(response) : null;
  const effectiveStatus = response && lastAssistantId(live) === predecessor ? response.status : live.status;
  const failed = !succeeded || effectiveStatus === "failed" || effectiveStatus === "stopped";
  useAppStore.setState(state => ({ promptQueues: { ...state.promptQueues, [sessionId]: { ...current, items: admitted ? current.items.filter(entry => entry.id !== item.id) : current.items, inFlight: undefined, waitingFor, paused: current.paused || failed, reason: failed ? "queue.failed" : current.reason } }, sessions: state.sessions.map(session => session.id === sessionId && response ? { ...session, goal: response.goal ?? null } : session) }));
  if (admitted && !succeeded) releaseUnusedQueueAttachments(item.context.attachments, `session:${sessionId}`);
  // A very fast turn may have settled while send_prompt was still returning.
  if (!waitingFor && live.status === "completed") void advancePromptQueue(sessionId);
}

let transcriptTick = 0;
const transcriptRequests = new Map<string, Promise<boolean>>();
/** Views that need a transcript while mounted (team helpers, historical reviews). */
const retainedTranscripts = new Map<string, number>();

/** Keeps a session's transcript loaded until the returned release is called. */
export function retainTranscript(sessionId: string) {
  retainedTranscripts.set(sessionId, (retainedTranscripts.get(sessionId) ?? 0) + 1);
  void useAppStore.getState().ensureTranscript(sessionId);
  return () => {
    const count = (retainedTranscripts.get(sessionId) ?? 1) - 1;
    if (count > 0) retainedTranscripts.set(sessionId, count); else retainedTranscripts.delete(sessionId);
    releaseTranscripts();
  };
}

/**
 * Drops transcripts outside the keep set; their metadata stays. `limit` is how
 * many recently opened ones stay beyond the selected, active and retained ones.
 */
function releaseTranscripts(limit?: number) {
  useAppStore.setState((state) => {
    const keep = transcriptsToKeep({
      loaded: state.loadedTranscripts, sessions: state.sessions, selectedSessionId: state.selectedSessionId,
      queued: Object.entries(state.promptQueues).filter(([, queue]) => queue.items.length).map(([id]) => id),
      retained: retainedTranscripts.keys(),
      limit,
    });
    const release = Object.keys(state.loadedTranscripts).filter((id) => !keep.has(id));
    if (!release.length) return {};
    const released = new Set(release);
    return {
      loadedTranscripts: Object.fromEntries(Object.entries(state.loadedTranscripts).filter(([id]) => !released.has(id))),
      sessions: state.sessions.map((session) => released.has(session.id)
        ? { ...session, messages: [], transcriptLength: session.messages.filter((message) => message.role !== "system").length }
        : session),
    };
  });
}

/**
 * Streamed deltas arrive in native batches of at most one per 16 ms (`output_batch.rs`).
 * The conversation on screen applies them at most ~30 times a second on an animation
 * frame; others, and everything while the document is hidden, every 250 ms or when
 * shown (ADR-180). Output keeps the exact native order and offsets.
 */
const outputBatch = (() => {
  const apply = (events: AgentEvent[]) => {
    const bySession = new Map<string, AgentEvent[]>();
    for (const event of events) {
      const queued = bySession.get(event.sessionId);
      if (queued) queued.push(event); else bySession.set(event.sessionId, [event]);
    }
    const gaps = new Set<string>();
    useAppStore.setState((state) => {
      let changed = false;
      const sessions = state.sessions.map((session) => {
        const queued = bySession.get(session.id);
        // An unloaded transcript is fetched whole when opened; partial text is never kept.
        if (!queued || state.loadedTranscripts[session.id] === undefined) return session;
        // A transcript loaded mid-stream can miss text emitted before it arrived.
        const messages = applyAgentOutputs(session.messages, queued, () => gaps.add(session.id));
        if (messages === session.messages) return session;
        changed = true;
        return { ...session, messages };
      });
      // Nothing applied (duplicates, unloaded transcripts): subscribers see no update.
      return changed ? { sessions } : {};
    });
    for (const id of gaps) void useAppStore.getState().ensureTranscript(id, true);
  };
  /** Sessions on screen: the open conversation and the panes of a split. */
  const isVisible = (sessionId: string) => {
    const state = useAppStore.getState();
    if (state.mainView !== "session") return false;
    return state.selectedSessionId === sessionId || splitLeaves(state.splitLayout.root).some((leaf) => leaf.sessionId === sessionId);
  };
  return new OutputScheduler<AgentEvent>({
    apply,
    isVisible,
    documentHidden: () => typeof document !== "undefined" && document.hidden,
    now: () => performance.now(),
    frame: (callback) => { const id = requestAnimationFrame(callback); return () => cancelAnimationFrame(id); },
    timer: (callback, ms) => { const id = setTimeout(callback, ms); return () => clearTimeout(id); },
  });
})();

/**
 * Clears what belongs to the computer being left before the next one bootstraps (ADR-133):
 * queues, tabs, dock panes, terminals and history name its sessions, and usage, models and
 * inbox data came from its CLIs. Window-bound state (computer use) stays with this Mac.
 */
export function resetComputerScope() {
  useAppStore.setState({
    editors: null, editorIcons: {},
    promptQueues: {}, composerContexts: {}, temporarySessionIds: [],
    usageResumeArmed: {}, usageLimitDismissed: {}, usageLimitResets: {},
    usageByProvider: {}, usageLoading: {}, modelsByProvider: {},
    dockPanes: [], dockActivePaneId: null, dockOpen: false, dockMaximized: false,
    terminalWorkspacesBySession: {}, runningTerminals: {}, splitLayout: initialSplit(),
    openTabsByProject: {}, lastActiveTabByProject: {}, draftTabByProject: {}, unseenSessionIds: [],
    closedItems: [], navHistory: [], navIndex: -1, messageJump: null, transcriptSearch: null,
    githubInbox: {}, prWatches: [], ciAutoFix: [], secrets: null, loginPrompts: null,
  });
  // Read once by `bindRealtime`; the transport already talks to the next computer.
  void useAppStore.getState().ciAutofixAction({ type: "status" });
  void useAppStore.getState().prWatchAction({ type: "status" });
}

export async function bindRealtime() {
  const unlisteners: (() => void)[] = [];
  try {
    unlisteners.push(await client.onActivityNotification((notice) => {
      // Sounds (Settings → Notifications, off by default) play once per new notice, never for the conversation in view.
      const before = useAppStore.getState();
      const cue = agentCueFor(notice.kind);
      if (cue && before.settings.agentSounds && before.sessions.some(session => session.id === notice.sessionId)
        && !before.activityNotifications.some(item => item.id === notice.id) && !isViewingNotice(before, notice, document.hasFocus())) playAgentCue(cue);
      useAppStore.setState((state) => {
        const pull = notice.kind === "pullRequest";
        if ((!pull && !state.sessions.some(session => session.id === notice.sessionId)) || state.activityNotifications.some(item => item.id === notice.id)) return {};
        if (!state.settingsOpen && (pull ? state.mainView === "pulls" : state.mainView === "session" && state.selectedSessionId === notice.sessionId) && document.hasFocus()) return {};
        return { activityNotifications: retainActivityNotifications([...state.activityNotifications, notice], state.settings.notifications) };
      });
    }));
    unlisteners.push(await client.onNotificationOpen((sessionId) => {
      // A pull request alert (ADR-050) sends `pull:owner/repo#n`, or `pull:` for a summary.
      if (sessionId.startsWith("pull:")) { openPullAlert(sessionId.slice(5) || undefined); return; }
      const store = useAppStore.getState();
      if (!store.sessions.some(session => session.id === sessionId)) return;
      store.setSettingsOpen(false);
      void store.selectSession(sessionId);
    }));
    if (typeof window !== "undefined" && typeof document !== "undefined") {
      window.addEventListener("focus", refreshPullsOnReturn);
      document.addEventListener("visibilitychange", refreshPullsOnReturn);
      unlisteners.push(() => { window.removeEventListener("focus", refreshPullsOnReturn); document.removeEventListener("visibilitychange", refreshPullsOnReturn); });
    }
    unlisteners.push(await client.onWindowSnap((event) => { void receiveWindowSnap(event); }));
    unlisteners.push(await client.onCiAutofixChanged(() => { void useAppStore.getState().ciAutofixAction({ type: "status" }); }));
    void useAppStore.getState().ciAutofixAction({ type: "status" });
    unlisteners.push(await client.onPrWatchChanged(() => { void useAppStore.getState().prWatchAction({ type: "status" }); }));
    // Settings changed by the phone or another window.
    unlisteners.push(await client.onSettingsChanged((saved) => {
      if (settingsSavesPending > 0) return;
      const current = useAppStore.getState().settings;
      const next = shareSettings(current, mergeSettings(saved));
      if (next !== current) useAppStore.setState({ settings: next });
    }));
    void useAppStore.getState().prWatchAction({ type: "status" });
    unlisteners.push(await client.onSessionUpdated((session) => {
      // Earlier deltas apply first; the snapshot then supersedes them by offset.
      outputBatch.flush();
      let reload = false;
      useAppStore.setState((state) => {
        const loaded = state.loadedTranscripts[session.id] !== undefined;
        // Native-created sessions (delegated tasks) arrive first as an event.
        if (!state.sessions.some((item) => item.id === session.id)) {
          return { sessions: [mergeSessionEvent({ ...session, messages: [] }, session, false).session, ...state.sessions] };
        }
        return {
          // Pins mutate only through serialized metadata acknowledgments. A lifecycle
          // snapshot captured before that acknowledgment must not undo it.
          sessions: state.sessions.map((item) => {
            if (item.id !== session.id) return item;
            const merged = mergeSessionEvent(item, session, loaded);
            reload = merged.reload;
            return merged.session;
          }),
        };
      });
      if (reload) void useAppStore.getState().ensureTranscript(session.id, true);
      // Events carry only the current turn: admission and queue checks read the merged transcript.
      const current = useAppStore.getState().sessions.find((item) => item.id === session.id);
      if (!current) return;
      const admission = directAdmissions.get(current.id);
      if (admission && observedPromptAdmission(current, admission.previousMessageId, admission.prompt)) {
        admission.clear();
        directAdmissions.delete(current.id);
      }
      observePromptQueue(current);
    }));
    unlisteners.push(() => outputBatch.cancel());
    // Text held back for a conversation that was not on screen is applied as it is shown.
    if (typeof document !== "undefined") {
      const showHeld = () => { if (!document.hidden) outputBatch.flush(); };
      document.addEventListener("visibilitychange", showHeld);
      unlisteners.push(() => document.removeEventListener("visibilitychange", showHeld));
    }
    unlisteners.push(useAppStore.subscribe((state, previous) => {
      if (state.selectedSessionId !== previous.selectedSessionId || state.mainView !== previous.mainView || state.splitLayout !== previous.splitLayout) outputBatch.flush();
    }));
    // Under system memory pressure only the transcripts in use stay; others reload when opened.
    unlisteners.push(await client.onMemoryPressure(() => releaseTranscripts(0)));
    unlisteners.push(await client.onAgentOutput((event) => {
      outputBatch.queue(event);
      const servers = scanLocalServers(event.chunk);
      if (servers.length) useAppStore.getState().noteLocalServers(event.sessionId, servers);
    }));
    unlisteners.push(await client.onPtyOutput((event) => {
      const servers = scanLocalServers(event.data);
      if (servers.length) useAppStore.getState().noteLocalServers(event.sessionId, servers);
    }));
    unlisteners.push(await client.onBrowserState((browser) => {
      useAppStore.setState((state) => ({ browserBySession: { ...state.browserBySession, [browser.sessionId]: browser } }));
    }));
    // An agent records the browser through the same recorder as the pane's button (ADR-112).
    unlisteners.push(await client.onBrowserRecordRequest((request) => {
      void (async () => {
        try {
          if (request.action === "stop") {
            const path = await stopRecording(request.sessionId, false);
            await client.browserRecordReply(request.requestId, { path, saved: Boolean(path) }, null);
            return;
          }
          const store = useAppStore.getState();
          const tabId = store.browserBySession[request.sessionId]?.activeTabId;
          if (!tabId) throw new Error("Open a page with browser_open first.");
          const { browserRecordingFps: fps, browserRecordingKeys: keys, browserRecordingClicks: clicks } = store.settings;
          await startRecording(request.sessionId, tabId, { fps, keys, clicks }, (_path, error) => {
            if (error) useAppStore.setState({ error: formatUnknownError(error) });
          });
          await client.browserRecordReply(request.requestId, { recording: true }, null);
        } catch (error) {
          await client.browserRecordReply(request.requestId, null, formatUnknownError(error)).catch(() => undefined);
        }
      })();
    }));
    unlisteners.push(await client.onComputerState((computer) => useAppStore.setState({ computer })));
    unlisteners.push(await client.onSecretState((secrets) => useAppStore.setState({ secrets })));
    unlisteners.push(await client.onLoginVaultState((loginPrompts) => useAppStore.setState({ loginPrompts })));
    // Pages an agent opens float over the conversation; a project script's preview opens in the browser (ADR-151).
    unlisteners.push(await client.onBrowserAgentPage((event) => useAppStore.getState().showAgentPage(event.sessionId, event.url)));
    unlisteners.push(await client.onProjectPreview((event) => void useAppStore.getState().openProjectPreview(event.sessionId, event.url)));
    unlisteners.push(await client.onAgentExit((event) => {
      outputBatch.flush();
      const store = useAppStore.getState();
      void store.refreshGitStatus();
      const provider = store.sessions.find((session) => session.id === event.sessionId)?.agent;
      // Not forced: the native 60 s cache bounds probes when many turns finish close together.
      // Shown in the sidebar rings, or already read for the composer's usage chips (ADR-120).
      if (provider && (store.settings.sidebarUsageProviders.includes(provider) || store.usageByProvider[provider]?.windows.length)) void store.refreshProviderUsage(provider);
    }));
    return () => unlisteners.forEach((unlisten) => unlisten());
  } catch (error) {
    unlisteners.forEach((unlisten) => unlisten());
    throw error;
  }
}

export const selectCurrentProject = (state: AppStore) =>
  state.projects.find((project) => project.id === state.selectedProjectId) ?? null;

export const selectCurrentSession = (state: AppStore) =>
  state.sessions.find((session) => session.id === state.selectedSessionId) ?? null;

/** True when two snapshots of a session differ at most in `messages`. */
export function sameSessionMeta(a: Session, b: Session) {
  if (a === b) return true;
  const left = a as unknown as Record<string, unknown>, right = b as unknown as Record<string, unknown>;
  const keys = Object.keys(left);
  if (keys.length !== Object.keys(right).length) return false;
  return keys.every((key) => key === "messages" || left[key] === right[key]);
}

let sessionsMeta: Session[] = [];
/**
 * Sessions for views that never read `messages` (sidebar, tabs, dock, composer):
 * the returned array keeps its identity while only streamed text changes, so
 * those views skip per-frame renders. Its `messages` may be stale; do not read them.
 */
export const selectSessionsMeta = (state: AppStore): Session[] => {
  const next = state.sessions;
  if (next.length === sessionsMeta.length && next.every((session, index) => sameSessionMeta(sessionsMeta[index], session))) return sessionsMeta;
  sessionsMeta = next;
  return next;
};

let listedFrom: Session[] | null = null;
let listed: Session[] = [];
/** Sessions shown in lists (sidebar, tabs, board, search): side chats live beside their parent (ADR-049) and snoozed ones under "Snoozed" (ADR-103). */
export const selectListedSessions = (state: AppStore): Session[] => {
  const meta = selectSessionsMeta(state);
  if (meta !== listedFrom) {
    listedFrom = meta;
    listed = meta.some((session) => isAuxiliarySession(session) || isSnoozed(session)) ? meta.filter((session) => !isAuxiliarySession(session) && !isSnoozed(session)) : meta;
  }
  return listed;
};

let snoozedFrom: Session[] | null = null;
let snoozed: Session[] = [];
/** Snoozed conversations (ADR-103), soonest back first; a turn that starts in one brings it back to the lists. */
export const selectSnoozedSessions = (state: AppStore): Session[] => {
  const meta = selectSessionsMeta(state);
  if (meta !== snoozedFrom) {
    snoozedFrom = meta;
    const next = meta.filter((session) => isSnoozed(session) && !isAuxiliarySession(session))
      .sort((a, b) => Date.parse(a.snoozedUntil ?? "") - Date.parse(b.snoozedUntil ?? ""));
    snoozed = next.length === snoozed.length && next.every((session, index) => session === snoozed[index]) ? snoozed : next;
  }
  return snoozed;
};

let currentMeta: Session | null = null;
/** The selected session for views that never read `messages`; see {@link selectSessionsMeta}. */
export const selectCurrentSessionMeta = (state: AppStore): Session | null => {
  const next = selectCurrentSession(state);
  if (next && currentMeta && sameSessionMeta(currentMeta, next)) return currentMeta;
  currentMeta = next;
  return next;
};

export type PromptLinkNotice = { id: number } & (
  | { kind: "opened"; hiddenCharacters: number; missingProject: string | null; provider: AgentProviderId | null; providerNote: "applied" | "unavailable" | "modelUnknown" | null; restore: { owner: string; text: string } | null }
  | { kind: "invalid"; reason: import("@/client/types").PromptLinkError }
  | { kind: "noProject" | "copied" | "copyEmpty" }
  | { kind: "copyFailed"; message: string }
);

export const selectProjectSessions = (state: AppStore) =>
  state.sessions.filter((session) => session.projectId === state.selectedProjectId);

export const selectEnabledAgents = (state: AppStore) =>
  state.agents.filter(
    (agent) => agent.installed && !state.settings.disabledProviders.includes(agent.id),
  );

let windowSnapSequence = 0;
/**
 * A global window snap joins the composer that is open: the selected session,
 * or the selected project's new-chat landing. Nothing is sent (ADR-054).
 */
async function receiveWindowSnap(event: WindowSnapEvent) {
  const notice = (kind: NonNullable<AppStore["windowSnapNotice"]>["kind"], app?: string) => useAppStore.setState({ windowSnapNotice: { id: ++windowSnapSequence, kind, app } });
  if (event.status !== "ready") { notice(event.status === "needsPermission" ? "permission" : event.status === "ownWindow" ? "own" : "failed"); return; }
  const state = useAppStore.getState();
  const owner = state.selectedSessionId ? `session:${state.selectedSessionId}` : state.selectedProjectId ? `project:${state.selectedProjectId}` : null;
  if (!owner) { void client.windowSnapAction({ type: "discard", nonce: event.nonce }); notice("noComposer"); return; }
  try {
    const attachments = await client.windowSnapAction({ type: "claim", nonce: event.nonce, owner });
    const current = useAppStore.getState();
    const context = composerContextForOwner(owner, current.composerContexts, current.sessions);
    try { current.setComposerContext(owner, { ...context, attachments: appendAttachments(context.attachments, attachments) }); }
    catch (error) { await client.releasePromptAttachments(owner, attachments.map((file) => file.id)); throw error; }
    notice("added", event.app);
  } catch (error) { useAppStore.setState({ error: formatUnknownError(error) }); }
}

// Usage limits: a resume is armed from the session's notice, or by itself when the
// automatic resume is on (ADR-063). One timer waits for the earliest reset (rechecking
// every minute and on wake, since timers stall while the Mac sleeps); a queue paused by
// the limit continues after the continue turn succeeds.
const RESUME_CANCELLED_KEY = "sirus.usageResumeCancelled";
function readResumeCancelled(): Record<string, number> {
  try {
    const value: unknown = JSON.parse(globalThis.localStorage?.getItem(RESUME_CANCELLED_KEY) ?? "{}");
    if (!value || typeof value !== "object") return {};
    return Object.fromEntries(Object.entries(value).filter((entry): entry is [string, number] => typeof entry[1] === "number").slice(0, 500));
  } catch { return {}; }
}
function writeResumeCancelled(value: Record<string, number>) {
  try { globalThis.localStorage?.setItem(RESUME_CANCELLED_KEY, JSON.stringify(value)); } catch { /* storage full or unavailable */ }
}
function limitReset(state: Pick<AppStore, "sessions" | "usageLimitResets">, sessionId: string): number | null {
  const session = state.sessions.find((item) => item.id === sessionId);
  return session?.usageLimit ? session.usageLimit.resetsAt ?? state.usageLimitResets[sessionId] ?? null : null;
}
/** Whether this session's usage-limit notice continues by itself at the reset. */
export function usageResumeActive(state: Pick<AppStore, "sessions" | "usageLimitResets" | "usageResumeArmed" | "usageResumeCancelled" | "settings">, sessionId: string): boolean {
  const session = state.sessions.find((item) => item.id === sessionId);
  const stoppedAt = session ? Date.parse(session.lastActivityAt) : Number.NaN;
  return resumeArmed({
    armed: Boolean(state.usageResumeArmed[sessionId]),
    auto: state.settings.usageAutoResume,
    limited: Boolean(session?.usageLimit),
    resetsAt: limitReset(state, sessionId),
    stoppedAt: Number.isFinite(stoppedAt) ? stoppedAt : Number.POSITIVE_INFINITY,
    cancelledReset: state.usageResumeCancelled[sessionId],
  });
}
const resumingAfterLimit = new Set<string>();
const queueAfterLimit = new Set<string>();
const resetLookups = new Set<string>();
let usageResumeTimer: ReturnType<typeof setTimeout> | undefined;
function scheduleUsageResume() {
  clearTimeout(usageResumeTimer);
  usageResumeTimer = undefined;
  const state = useAppStore.getState();
  const now = Date.now();
  let next = Number.POSITIVE_INFINITY;
  for (const session of state.sessions) {
    const id = session.id;
    if (!session.usageLimit || !usageResumeActive(state, id)) continue;
    const at = resumeAt(limitReset(state, id));
    if (at == null) continue;
    if (now >= at) void state.resumeAfterUsageLimit(id);
    else next = Math.min(next, at - now);
  }
  if (Number.isFinite(next)) usageResumeTimer = setTimeout(scheduleUsageResume, Math.min(next, RESUME_RECHECK_MS));
}
let usageResumeSignature = "";
useAppStore.subscribe((state, previous) => {
  if (state.sessions === previous.sessions && state.usageResumeArmed === previous.usageResumeArmed && state.usageLimitResets === previous.usageLimitResets
    && state.usageResumeCancelled === previous.usageResumeCancelled && state.settings.usageAutoResume === previous.settings.usageAutoResume) return;
  for (const id of queueAfterLimit) {
    const session = state.sessions.find((item) => item.id === id);
    if (session && activeSession(session)) continue;
    queueAfterLimit.delete(id);
    if (session?.status === "completed") state.resumePromptQueue(id);
  }
  // A new send clears the native limit; its notice state goes with it.
  const limited = new Set(state.sessions.filter((session) => session.usageLimit).map((session) => session.id));
  const stale = (record: Record<string, unknown>) => Object.keys(record).some((id) => !limited.has(id));
  if (state.sessions.length && stale(state.usageResumeCancelled)) {
    const kept = Object.fromEntries(Object.entries(state.usageResumeCancelled).filter(([id]) => limited.has(id)));
    writeResumeCancelled(kept);
    queueMicrotask(() => useAppStore.setState({ usageResumeCancelled: kept }));
  }
  if (stale(state.usageResumeArmed) || stale(state.usageLimitDismissed) || stale(state.usageLimitResets)) {
    const keep = <T,>(record: Record<string, T>) => Object.fromEntries(Object.entries(record).filter(([id]) => limited.has(id)));
    for (const id of resetLookups) if (!limited.has(id)) resetLookups.delete(id);
    queueMicrotask(() => useAppStore.setState((current) => ({ usageResumeArmed: keep(current.usageResumeArmed), usageLimitDismissed: keep(current.usageLimitDismissed), usageLimitResets: keep(current.usageLimitResets) })));
    return;
  }
  // Streaming deltas elsewhere change `sessions` every frame; re-arm only when a limited session's state moved.
  const signature = state.sessions.filter((session) => session.usageLimit || queueAfterLimit.has(session.id)).map((session) => `${session.id}:${session.status}:${session.usageLimit?.resetsAt ?? ""}`).join("|");
  const changed = signature !== usageResumeSignature || state.usageResumeArmed !== previous.usageResumeArmed || state.usageLimitResets !== previous.usageLimitResets
    || state.usageResumeCancelled !== previous.usageResumeCancelled || state.settings.usageAutoResume !== previous.settings.usageAutoResume;
  usageResumeSignature = signature;
  if (changed && (signature || usageResumeTimer)) scheduleUsageResume();
});
// After sleep or a while in the background, the timer may be late: check again at once.
if (typeof window !== "undefined") {
  const wake = () => { if (document.visibilityState !== "hidden" && (usageResumeTimer || usageResumeSignature)) scheduleUsageResume(); };
  window.addEventListener("focus", wake);
  document.addEventListener("visibilitychange", wake);
}

// A temporary session is deleted once it is no longer open in any pane and has
// settled; a running turn finishes first. Its worktree is kept (never removed
// without an explicit confirmation).
const deletingTemporary = new Set<string>();
useAppStore.subscribe((state, previous) => {
  if (!state.temporarySessionIds.length) return;
  if (state.selectedSessionId === previous.selectedSessionId && state.sessions === previous.sessions && state.splitLayout === previous.splitLayout && state.temporarySessionIds === previous.temporarySessionIds) return;
  const open = new Set([state.selectedSessionId, ...splitLeaves(state.splitLayout.root).map((leaf) => leaf.sessionId)]);
  for (const id of state.temporarySessionIds) {
    if (open.has(id) || deletingTemporary.has(id)) continue;
    const session = state.sessions.find((item) => item.id === id);
    const forget = () => useAppStore.setState((current) => ({ temporarySessionIds: current.temporarySessionIds.filter((item) => item !== id) }));
    if (!session) { queueMicrotask(forget); continue; }
    if (["starting", "running", "waiting"].includes(session.status)) continue;
    deletingTemporary.add(id);
    void state.deleteSession(id, false).finally(() => { deletingTemporary.delete(id); forget(); });
  }
});

// Project `+N -M` totals: re-read shortly after a turn settles (and on demand), never on a timer.
let projectDiffTimer: ReturnType<typeof setTimeout> | undefined;
export function scheduleProjectDiffs() {
  clearTimeout(projectDiffTimer);
  projectDiffTimer = setTimeout(() => void useAppStore.getState().refreshProjectDiffs(), 800);
}
useAppStore.subscribe((state, previous) => {
  if (state.sessions === previous.sessions) return;
  const before = new Map(previous.sessions.map((session) => [session.id, session.status]));
  const busy = (status: string | undefined) => status === "running" || status === "starting" || status === "waiting";
  if (state.sessions.some((session) => busy(before.get(session.id)) && !busy(session.status))) scheduleProjectDiffs();
});

// The active pane follows the selection; panes of deleted sessions close.
useAppStore.subscribe((state, previous) => {
  if (state.selectedSessionId === previous.selectedSessionId && state.sessions === previous.sessions) return;
  const layout = state.splitLayout;
  if (state.sessions !== previous.sessions && state.selectedSessionId === previous.selectedSessionId && splitLeaves(layout.root).length < 2) return;
  const next = splitSync(layout, state.selectedSessionId, new Set(state.sessions.map((session) => session.id)));
  if (next !== layout) useAppStore.setState({ splitLayout: next });
});

// Opening a session loads its transcript; leaving it lets the cache release older ones.
useAppStore.subscribe((state, previous) => {
  if (state.selectedSessionId === previous.selectedSessionId) return;
  if (state.selectedSessionId) void state.ensureTranscript(state.selectedSessionId);
  else releaseTranscripts();
});
