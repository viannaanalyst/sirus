import { LocalTransport } from "./local-transport";
import { socketUrl, socketUrlFor } from "./remote-pairing";
import { RemoteTransport, type RemoteConnection } from "./remote-transport";
import { SwitchingTransport } from "./switching-transport";
import { ACTIVE_ENVIRONMENT_KEY, HOME_ENVIRONMENT, chooseActive, readEnvironments } from "@/lib/environments";
import { eventFromThisMac, runsOnThisMac } from "@/lib/computers";
import { readIncoming, readPullOutcome } from "@/lib/git-incoming";
import type { Transport } from "./transport";
import type {
  AgentEvent,
  AgentExitEvent,
  AgentInstall,
  AgentProviderId,
  AppData,
  AppSettings,
  CreateSessionRequest,
  FileEntry,
  GitIdentity,
  GitStatus,
  HostInfo,
  ProbeResult,
  Project,
  ProviderModelList,
  PtyOutputEvent,
  SendPromptRequest,
  Session,
  VsCodeTheme,
  VsCodeThemeExtension,
} from "./types";

type SharedListener = { handlers: Set<(payload: unknown) => void>; ready: Promise<() => void> };

export class SirusClient {
  private readonly transport: Transport;
  private readonly shared = new Map<string, SharedListener>();
  constructor(transport: Transport) { this.transport = transport; }

  /**
   * One native listener per event, fanned out to every handler. Each native
   * listener receives its own serialized payload, so several terminals listening
   * separately would multiply the cost of every output chunk.
   */
  private listenShared<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    let entry = this.shared.get(event);
    if (!entry) {
      const handlers = new Set<(payload: unknown) => void>();
      const created: SharedListener = {
        handlers,
        ready: this.transport.listen<unknown>(event, (payload) => { for (const each of [...handlers]) each(payload); }),
      };
      created.ready.catch(() => { if (this.shared.get(event) === created) this.shared.delete(event); });
      this.shared.set(event, created);
      entry = created;
    }
    const current = entry;
    const own = handler as (payload: unknown) => void;
    current.handlers.add(own);
    return current.ready.then(() => () => {
      current.handlers.delete(own);
      if (current.handlers.size || this.shared.get(event) !== current) return;
      this.shared.delete(event);
      void current.ready.then((unlisten) => unlisten());
    }, (error: unknown) => {
      current.handlers.delete(own);
      throw error;
    });
  }

  loadState() {
    return this.transport.invoke<AppData>("load_state");
  }

  async skillsCatalog(owner: import("./types").SkillOwner) {
    const result = await this.transport.invoke<import("./types").SkillActionResponse>("skill_action", { action: { type: "catalog", owner } });
    if (result.type !== "catalog") throw new Error("Unexpected skill catalog response");
    return result.catalog;
  }
  async skillPreview(owner: import("./types").SkillOwner, sourceId: string) {
    const result = await this.transport.invoke<import("./types").SkillActionResponse>("skill_action", { action: { type: "preview", owner, sourceId } });
    if (result.type !== "preview") throw new Error("Unexpected skill preview response");
    return result.document;
  }

  /** MCP manager (ADR-075): list, add or remove servers in the provider CLIs' own config files. */
  connectorAction(action: import("./types").ConnectorAction) {
    return this.transport.invoke<import("./types").ConnectorStatus[]>("connector_action", { action });
  }

  mcpAction(action: import("./types").McpAction) {
    return this.transport.invoke<import("./types").McpActionResponse>("mcp_action", { action });
  }

  providerUsage(provider: AgentProviderId, refresh = false, accountId?: string) {
    return this.transport.invoke<import("./types").ProviderUsage>("provider_usage", { provider, refresh, accountId });
  }

  /** Usage page (ADR-141): token history from the provider CLIs' local logs over the past `days` (1–90). */
  usageHistory(days: number) {
    return this.transport.invoke<import("./types").UsageHistory>("usage_history", { days });
  }

  /** Usage page prices: the named models' USD per million tokens from the daily public price table. */
  usagePrices(models: string[]) {
    return this.transport.invoke<import("./types").UsagePrices>("usage_prices", { models });
  }

  createProviderAccount(provider: AgentProviderId, name: string) {
    return this.transport.invoke<import("./types").ProviderAccount>("create_provider_account", { provider, name });
  }
  selectProviderAccount(provider: AgentProviderId, accountId: string) {
    return this.transport.invoke<void>("select_provider_account", { provider, accountId });
  }
  renameProviderAccount(provider: AgentProviderId, accountId: string, name: string) {
    return this.transport.invoke<import("./types").ProviderAccount>("rename_provider_account", { provider, accountId, name });
  }
  loginProviderAccount(provider: AgentProviderId, accountId: string) {
    return this.transport.invoke<void>("login_provider_account", { provider, accountId });
  }
  cancelProviderAccountLogin(provider: AgentProviderId, accountId: string) {
    return this.transport.invoke<void>("cancel_provider_account_login", { provider, accountId });
  }

  consumeCodexReset(offer: string) {
    return this.transport.invoke<import("./types").CodexResetResult>("consume_codex_reset", { offer, confirm: true });
  }

  providerUpdates(refresh = false) {
    return this.transport.invoke<import("./types").ProviderUpdate[]>("provider_updates", { refresh });
  }

  updateProviders(providers: AgentProviderId[]) {
    return this.transport.invoke<import("./types").ProviderUpdateResult[]>("update_providers", { providers, confirm: true });
  }

  saveSettings(settings: AppSettings) {
    return this.transport.invoke<AppSettings>("save_settings", { settings });
  }

  saveComposerDraft(key: string, value: string) {
    return this.transport.invoke<void>("save_composer_draft", { key, value });
  }

  saveContextText(key: string, value: string, flush = false) {
    return this.transport.invoke<void>("save_context_text", { key, value, flush });
  }

  pastePromptAttachments(owner: string, files: { name: string; data: string }[], expectedText?: string) {
    return this.transport.invoke<import("./types").PromptAttachment[]>("paste_prompt_attachments", { owner, files, expectedText: expectedText ?? null });
  }
  /** Where a Finder drag is over the window (CSS px), and when it drops; paths stay native. */
  onFileDrop(handler: (event: { kind: "over" | "drop" | "leave"; x?: number; y?: number; count?: number }) => void) {
    return this.listenShared<{ kind: "over" | "drop" | "leave"; x?: number; y?: number; count?: number }>("file-drop", handler);
  }
  /** Attaches the files and folders of the drop that just happened (taken once, natively). */
  dropPromptAttachments(owner: string) {
    return this.transport.invoke<import("./types").PromptAttachment[]>("drop_prompt_attachments", { owner });
  }
  releasePromptAttachments(owner: string, ids: string[]) {
    return this.transport.invoke<void>("release_prompt_attachments", { owner, ids });
  }
  pickPromptAttachments(owner: string, folder?: boolean) {
    return this.transport.invoke<import("./types").PromptAttachment[]>("pick_prompt_attachments", { owner, folder });
  }
  capturePromptWindow(owner: string) {
    return this.transport.invoke<import("./types").PromptAttachment[]>("capture_prompt_window", { owner });
  }
  /** A composer image at full size, for the viewer and the annotator (the chip shows a small copy). */
  attachmentImage(owner: string, id: string) {
    return this.transport.invoke<string>("attachment_image", { owner, id });
  }

  /** Where a sent image's thumbnail loads from, by attachment id (no base64 over IPC). */
  mediaUrl(token: string): string | undefined {
    return this.transport.mediaUrl?.(token);
  }

  /** A token for a video inside the session's workspace, with its size and type. */
  mediaPreview(sessionId: string, path: string) {
    return this.transport.invoke<{ token: string; size: number; mime: string }>("media_preview", { sessionId, path });
  }

  mediaRelease(token: string) {
    return this.transport.invoke<void>("media_release", { token });
  }

  thumbnailUrl(id: string): string | undefined {
    return this.transport.thumbnailUrl?.(id);
  }

  /** `plain` asks for the typed text and table view of a Word or Excel file instead of the Office renderer. */
  attachmentPreview(owner: string, id: string, plain = false) {
    return this.transport.invoke<import("./types").DocumentPreview>("attachment_preview", { owner, id, plain });
  }
  /** A document in a session's workspace (a project file or one an agent wrote), under the same bounds. */
  workspaceDocument(sessionId: string, path: string, plain = false) {
    return this.transport.invoke<import("./types").DocumentPreview>("workspace_document", { sessionId, path, plain });
  }

  pickFolder() {
    return this.transport.invoke<string | null>("pick_folder");
  }

  addProject(path: string) {
    return this.transport.invoke<Project>("add_project", { path });
  }

  /** New folder `<parent>/<slug of name>` with `git init`, README.md and a first commit, added as a project. */
  createProject(name: string, parent: string) {
    return this.transport.invoke<Project>("create_project", { name, parent });
  }

  /** Clones a GitHub `owner/repo`, `https://` or SSH address into `<parent>/<folder or repo name>`, added as a project. */
  cloneProject(source: string, parent: string, folder?: string | null) {
    return this.transport.invoke<Project>("clone_project", { source, parent, folder: folder?.trim() || null });
  }

  /** Moving a conversation (ADR-133), source side: pushes its work as `sirus/move-<id>` and returns it. */
  prepareSessionMove(sessionId: string, target: string) {
    return this.transport.invoke<import("./types").SessionMoveExport>("prepare_session_move", { sessionId, target, confirm: true });
  }

  /** Target side: the project whose GitHub origin is `remoteUrl`, if this computer has it. */
  findProjectByRemote(remoteUrl: string) {
    return this.transport.invoke<Project | null>("find_project_by_remote", { remoteUrl });
  }

  /** Target side: a session on the moved branch with the copied conversation and a recap. */
  importSession(request: import("./types").ImportSessionRequest) {
    return this.transport.invoke<Session>("import_session", { request });
  }

  /** Source side, once the target has it: notes where the conversation went. */
  markSessionMoved(sessionId: string, target: string, branch: string) {
    return this.transport.invoke<void>("mark_session_moved", { sessionId, target, branch });
  }

  removeProject(projectId: string) {
    return this.transport.invoke<void>("remove_project", { projectId });
  }

  openProject(projectId: string) {
    return this.transport.invoke<Project>("open_project", { projectId });
  }

  /** Native image picker; the picked image as a small square PNG data URL, or `null` when cancelled. */
  pickLookImage() {
    return this.transport.invoke<string | null>("pick_look_image");
  }

  projectScriptsAction(action: import("./types").ProjectScriptsAction) {
    return this.transport.invoke<Project | null>("project_scripts_action", { action });
  }

  scanLeftoverWorktrees() {
    return this.transport.invoke<import("./types").WorktreeLeftoverScan>("worktree_cleanup_action", { action: { type: "scan" } });
  }

  cleanLeftoverWorktrees() {
    return this.transport.invoke<import("./types").WorktreeCleanupResult>("worktree_cleanup_action", { action: { type: "clean", confirm: true } });
  }

  /** Saves a project's avatar choices, or clears them with `null` (ADR-175). */
  setProjectAppearance(projectId: string, appearance: import("./types").ProjectAppearance | null) {
    return this.transport.invoke<Project>("set_project_appearance", { projectId, appearance });
  }

  renameProject(projectId: string, name: string) {
    return this.transport.invoke<Project>("rename_project", { projectId, name });
  }

  gitIdentity(path: string) {
    return this.transport.invoke<GitIdentity>("git_identity", { path });
  }

  /** Uncommitted `+N -M` of every project checkout, for the sidebar. */
  projectDiffStats() {
    return this.transport.invoke<{ projectId: string; additions: number; deletions: number }[]>("project_diff_stats");
  }

  gitStatus(sessionId: string) {
    return this.transport.invoke<GitStatus>("git_status", { sessionId });
  }

  gitDiff(sessionId: string, path: string) {
    return this.transport.invoke<string>("git_diff", { sessionId, path });
  }

  listBranches(projectId: string) {
    return this.transport.invoke<import("./types").BranchInfo[]>("list_branches", { projectId });
  }

  checkoutBranch(projectId: string, branch: string) {
    return this.transport.invoke<void>("checkout_branch", { projectId, branch, confirm: true });
  }

  createBranch(projectId: string, branch: string) {
    return this.transport.invoke<void>("create_branch", { projectId, branch, confirm: true });
  }

  /** Saves Markdown of an owned conversation where the person picks in the native dialog (ADR-060); false when cancelled. */
  exportConversation(sessionId: string, markdown: string) {
    return this.transport.invoke<boolean>("export_conversation", { sessionId, markdown });
  }

  /** One session's full transcript, loaded when a view needs it (ADR-048). */
  async loadTranscript(sessionId: string) {
    const result = await this.transport.invoke<import("./types").TranscriptResponse>("transcript_action", { action: { type: "load", sessionId } });
    if (result.type !== "transcript" || result.sessionId !== sessionId) throw new Error("Unexpected transcript response");
    return result.messages;
  }

  /** Bounded candidate messages for the all-conversations search; matching stays in the renderer. */
  async searchTranscripts(query: string) {
    const result = await this.transport.invoke<import("./types").TranscriptResponse>("transcript_action", { action: { type: "search", query } });
    if (result.type !== "candidates") throw new Error("Unexpected transcript search response");
    return result;
  }

  /** A command row's full output when it is stored apart (ADR-147); `stepId` is the activity row's id. */
  async toolOutput(sessionId: string, messageId: string, stepId: string) {
    const result = await this.transport.invoke<import("./types").TranscriptResponse>("transcript_action", { action: { type: "toolOutput", sessionId, messageId, stepId } });
    if (result.type !== "toolOutput") throw new Error("Unexpected tool output response");
    return result.text;
  }

  /** A reply's turn review with the diff texts transcripts leave out (ADR-164). */
  async turnReview(sessionId: string, messageId: string) {
    const result = await this.transport.invoke<import("./types").TranscriptResponse>("transcript_action", { action: { type: "turnReview", sessionId, messageId } });
    if (result.type !== "turnReview") throw new Error("Unexpected turn review response");
    return result.review;
  }

  /** The newest `limit` messages of a transcript, for a first paint over the network (ADR-164). */
  async loadTranscriptPage(sessionId: string, limit: number) {
    const result = await this.transport.invoke<import("./types").TranscriptResponse>("transcript_action", { action: { type: "load", sessionId, limit } });
    if (result.type !== "transcript" || result.sessionId !== sessionId) throw new Error("Unexpected transcript response");
    return result;
  }

  /** The active computer is reached over the network (the phone's web app, or another computer from the desktop). */
  get overNetwork(): boolean {
    const transport = this.transport instanceof SwitchingTransport ? this.transport.active : this.transport;
    return transport instanceof RemoteTransport;
  }

  async gitWorkspaceHistory(sessionId: string, from: string, skip: number) {
    const result = await this.transport.invoke<import("./types").GitWorkspaceResponse>("git_workspace_action", { action: { type: "history", sessionId, from, skip } });
    if (result.type !== "history") throw new Error("Unexpected Git history response");
    return { entries: result.entries, truncated: result.truncated };
  }

  /** Explicit fast-forward-only pull of the session branch. */
  async gitPull(sessionId: string) {
    const result = await this.transport.invoke<import("./types").GitWorkspaceResponse>("git_workspace_action", { action: { type: "pull", sessionId, confirm: true } });
    if (result.type !== "pulled") throw new Error("Unexpected Git pull response");
    return result;
  }

  /** What the session's workspace branch can pull, after a background fetch the native cache allows (ADR-148). */
  async gitIncoming(sessionId: string) {
    return readIncoming(await this.transport.invoke<unknown>("git_incoming", { sessionId }));
  }

  /** The header's Pull: fast-forward only from the branch's upstream (ADR-148). */
  async gitPullFastForward(sessionId: string) {
    const outcome = readPullOutcome(await this.transport.invoke<unknown>("git_pull_ff", { sessionId }));
    if (!outcome) throw new Error("Unexpected Git pull response");
    return outcome;
  }

  /** Pushes the session branch and opens a pull request; resolves to its GitHub URL. */
  async createPullRequest(sessionId: string, title: string, body: string, draft: boolean) {
    const result = await this.transport.invoke<import("./types").PullRequestResponse>("pull_request_action", { action: { type: "create", sessionId, title, body, draft, confirm: true } });
    if (result.type !== "created") throw new Error("Unexpected pull request response");
    return result.url;
  }

  async gitWorkspace(action: Exclude<import("./types").GitWorkspaceAction, { type: "diff" | "history" | "pull" }>) {
    const result = await this.transport.invoke<import("./types").GitWorkspaceResponse>("git_workspace_action", { action });
    if (result.type !== "snapshot") throw new Error("Unexpected Git workspace response");
    return result.snapshot;
  }

  async gitWorkspaceDiff(sessionId: string, path: string, staged: boolean, expectedIndex: string) {
    const result = await this.transport.invoke<import("./types").GitWorkspaceResponse>("git_workspace_action", { action: { type: "diff", sessionId, path, staged, expectedIndex } });
    if (result.type !== "diff") throw new Error("Unexpected Git diff response");
    return result.diff;
  }

  async gitCommitTitle(sessionId: string, expectedIndex: string, requestId: string): Promise<import("./types").CommitTitleResult | null> {
    const action: import("./types").CommitTitleAction = { type: "generate", sessionId, expectedIndex, requestId };
    const response = await this.transport.invoke<unknown>("commit_title_action", { action });
    if (isCancelledTitle(response)) return null;
    if (!response || typeof response !== "object") throw new Error("Unexpected commit title response");
    const value = response as Record<string, unknown>;
    if (Object.keys(value).length !== 4 || value.type !== "title" || typeof value.title !== "string" || !value.title.trim() || value.title.trim() !== value.title || Array.from(value.title).length > 72 || /[\p{Cc}\p{Zl}\p{Zp}]/u.test(value.title) || (value.provider !== "codex" && value.provider !== "claude") || typeof value.partial !== "boolean") throw new Error("Unexpected commit title response");
    return value as import("./types").CommitTitleResult;
  }
  async cancelCommitTitle(sessionId: string, requestId: string): Promise<void> {
    const action: import("./types").CommitTitleAction = { type: "cancel", sessionId, requestId };
    const response = await this.transport.invoke<unknown>("commit_title_action", { action });
    if (!isCancelledTitle(response)) throw new Error("Unexpected commit title cancellation response");
  }

  gitCommit(sessionId: string, message: string, expectedIndex?: string) {
    return this.transport.invoke<import("./types").GitCommitResult>("git_commit", { sessionId, message, expectedIndex });
  }

  gitPush(sessionId: string) {
    return this.transport.invoke<import("./types").GitPushResult>("git_push", { sessionId });
  }

  projectRemoteUrl(projectId: string) {
    return this.transport.invoke<string | null>("project_remote_url", { projectId });
  }

  sessionPullRequest(sessionId: string) {
    return this.transport.invoke<import("./types").PullRequestSnapshot>("session_pull_request", { sessionId });
  }

  /** GitHub CLI accounts: list, switch, sign out and the device-flow sign-in (ADR-188). */
  githubAccountAction(action: import("./types").GitHubAccountAction) {
    return this.transport.invoke<import("./types").GitHubAccountResponse>("github_account_action", { action });
  }

  openExternalUrl(url: string) {
    return this.transport.invoke<void>("open_external_url", { url });
  }

  detectEditors() {
    return this.transport.invoke<import("./types").EditorInstall[]>("detect_editors");
  }

  editorAppIcons() {
    return this.transport.invoke<import("./types").EditorAppIcon[]>("editor_app_icons");
  }

  openInEditor(sessionId: string, editor: import("./types").EditorId, path?: string | null) {
    return this.transport.invoke<void>("open_in_editor", { sessionId, editor, path: path ?? null });
  }

  readTextFile(sessionId: string, path: string) {
    return this.transport.invoke<import("./types").TextFileSnapshot>("read_text_file", { sessionId, path });
  }

  writeTextFile(sessionId: string, path: string, content: string) {
    return this.transport.invoke<void>("write_text_file", { sessionId, path, content });
  }

  listWorktrees(projectId: string) {
    return this.transport.invoke<import("./types").GitWorktree[]>("list_worktrees", { projectId });
  }

  listDir(sessionId: string, path?: string) {
    return this.transport.invoke<FileEntry[]>("list_dir", { sessionId, path });
  }

  /** Literal text search in the session's workspace, bounded on the native side (ADR-095). */
  searchWorkspaceText(sessionId: string, query: string, caseSensitive = false) {
    return this.transport.invoke<import("./types").TextSearchResult>("search_workspace_text", { sessionId, query, caseSensitive });
  }

  /** Moves one session-owned entry to the system Trash (recoverable). */
  trashWorkspaceEntry(sessionId: string, path: string): Promise<void> {
    return this.transport.invoke<void>("trash_workspace_entry", { sessionId, path });
  }

  createWorkspaceEntry(sessionId: string, kind: import("./types").WorkspaceEntryKind, name: string, parentPath?: string): Promise<FileEntry> {
    return this.transport.invoke<FileEntry>("create_workspace_entry", { sessionId, kind, name, parentPath });
  }

  /** Registers agent-written HTML for the transcript; returns the id served on `sirus-preview://localhost/<id>` (ADR-072). */
  htmlPreview(html: string) {
    return this.transport.invoke<string>("html_preview", { html });
  }

  workspaceFiles(owner: import("./types").SkillOwner, query: string) {
    return this.transport.invoke<import("./types").WorkspaceFiles>("workspace_files", { owner, query });
  }

  detectAgents() {
    return this.transport.invoke<AgentInstall[]>("detect_agents");
  }

  hostInfo() {
    return this.transport.invoke<HostInfo>("host_info");
  }

  probeProvider(id: AgentProviderId, path?: string | null) {
    return this.transport.invoke<ProbeResult>("probe_provider", { id, path });
  }

  pickExecutable() {
    return this.transport.invoke<string | null>("pick_executable");
  }

  openPath(path: string) {
    return this.transport.invoke<void>("open_path", { path });
  }

  createSession(request: CreateSessionRequest) {
    return this.transport.invoke<Session>("create_session", { request });
  }

  renameSession(sessionId: string, title: string) {
    return this.transport.invoke<Session>("rename_session", { sessionId, title });
  }

  /** Snoozes until `until` (RFC 3339), or returns the session now and clears its reminder with null (ADR-103). */
  snoozeSession(sessionId: string, until: string | null) {
    return this.transport.invoke<Session>("snooze_session", { sessionId, until });
  }

  forkSession(sessionId: string, messageId: string) {
    return this.transport.invoke<Session>("fork_session", { sessionId, messageId });
  }

  /** `newWorktree` moves the handoff into a new isolated worktree seeded from the current checkout. */
  handoffSession(sessionId: string, messageId: string, agent: import("./types").AgentProviderId, model: string | null, newWorktree = false) {
    return this.transport.invoke<Session>("handoff_session", { sessionId, messageId, agent, model, newWorktree });
  }

  /** Stops a localhost server only when Sirus Code started it; resolves to the processes signalled. */
  stopLocalServer(sessionId: string, url: string) {
    return this.transport.invoke<number>("local_server_action", { action: { type: "stop", sessionId, url } });
  }

  /** Moves a stopped Claude or Codex session to another signed-in account of its provider (ADR-063 amendment). */
  moveSessionAccount(sessionId: string, accountId: string) {
    return this.transport.invoke<import("./types").AccountMove>("move_session_account", { request: { sessionId, accountId } });
  }

  dismissHandoff(sessionId: string) {
    return this.transport.invoke<Session>("dismiss_handoff", { sessionId });
  }

  /** Sends an instruction into the session's running Codex/Claude reply (ADR-062). */
  steerTurn(sessionId: string, text: string) {
    return this.transport.invoke<void>("steer_turn", { sessionId, text });
  }

  /** A message for one running Claude subagent, read before its next step (ADR-113). */
  steerSubagent(sessionId: string, taskId: string, text: string) {
    return this.transport.invoke<void>("steer_subagent", { sessionId, taskId, text });
  }

  /** Moves a running foreground Claude subagent to the background (ADR-111). */
  backgroundSubagent(sessionId: string, taskId: string, toolUseId: string) {
    return this.transport.invoke<void>("background_subagent", { sessionId, taskId, toolUseId });
  }

  /** Stops one subagent of the session's running Claude reply (ADR-097, ADR-111). */
  stopBackgroundTask(sessionId: string, taskId: string) {
    return this.transport.invoke<void>("stop_background_task", { sessionId, taskId });
  }

  /** Reverses a settled turn's retained diffs, file by file, after a check (ADR-061). */
  undoTurnChanges(sessionId: string, messageId: string, paths?: string[]) {
    return this.transport.invoke<import("./types").UndoTurnOutcome>("undo_turn_changes", { request: { sessionId, messageId, paths, confirm: true } });
  }

  /** Read-only: why undoing this turn would be refused right now, or null (ADR-187). */
  checkTurnUndo(sessionId: string, messageId: string, paths?: string[]) {
    return this.transport.invoke<import("./types").UndoBlock | null>("check_turn_undo", { request: { sessionId, messageId, paths } });
  }

  keepTurnChanges(sessionId: string, messageId: string) {
    return this.transport.invoke<import("./types").TurnReview>("keep_turn_changes", { sessionId, messageId });
  }

  setMessagePinned(sessionId: string, messageId: string, pinned: boolean) {
    return this.transport.invoke<string[]>("set_message_pinned", { sessionId, messageId, pinned });
  }

  setSessionAgent(sessionId: string, agent: AgentProviderId) {
    return this.transport.invoke<Session>("set_session_agent", { sessionId, agent });
  }

  setSessionModel(sessionId: string, agent: AgentProviderId, model?: string | null) {
    return this.transport.invoke<Session>("set_session_model", { sessionId, agent, model });
  }

  listProviderModels(id: AgentProviderId, refresh = false) {
    return this.transport.invoke<ProviderModelList>("list_provider_models", { id, refresh });
  }

  deleteSession(sessionId: string, removeWorktree: boolean, confirm: boolean) {
    return this.transport.invoke<void>("delete_session", {
      sessionId,
      removeWorktree,
      confirm,
    });
  }

  sendPrompt(request: SendPromptRequest) {
    return this.transport.invoke<Session>("send_prompt", { request });
  }

  /** Follow (or stop following) a session's workspace on disk; resolves to the path its events carry (ADR-109). */
  workspaceWatch(sessionId: string, watch: boolean) {
    return this.transport.invoke<string>("workspace_watch", { sessionId, watch });
  }

  /** Files changed in a followed workspace (batched natively). */
  onWorkspaceChanged(handler: (path: string) => void) {
    return this.transport.listen<{ path: string }>("workspace-changed", (event) => handler(event.path));
  }

  /** Sends the conversation's last message again, with its attachments while the app keeps them. */
  retryLastTurn(sessionId: string) {
    return this.transport.invoke<unknown>("retry_last_turn", { sessionId });
  }

  /** Picks or clears the chat background image (ADR-089); returns the saved file name. */
  chatBackgroundAction(type: "pick" | "clear") {
    return this.transport.invoke<string | null>("chat_background_action", { action: { type } });
  }

  /** Searches Open VSX for colour themes (ADR-185); the back end alone talks to the network. */
  searchVsCodeThemes(query: string, sort: "downloadCount" | "rating" | "timestamp" | "relevance" = "downloadCount") {
    return this.transport.invoke<VsCodeThemeExtension[]>("vscode_themes_search", { query, sort });
  }

  /** Downloads, verifies and reads the themes of one Open VSX extension. */
  fetchVsCodeThemes(namespace: string, name: string) {
    return this.transport.invoke<VsCodeTheme[]>("vscode_themes_fetch", { namespace, name });
  }

  /** Picks a `.vsix` or `.json` theme on this Mac; null when cancelled. */
  importVsCodeThemeFile() {
    return this.transport.invoke<VsCodeTheme[] | null>("vscode_themes_import_local");
  }

  chatBackgroundImage(name: string) {
    if (isRemoteUi) return Promise.resolve(null);
    return this.transport.invoke<string | null>("chat_background_image", { name });
  }

  /** The first ⌘Q armed quitting; a second press within `ms` quits (ADR-090). */
  onQuitArmed(handler: (ms: number) => void) {
    return this.transport.listen<number>("quit-armed", handler);
  }

  /** In-app updates of this Mac's app (ADR-140): status, check, install or restart. */
  appUpdateAction(action: import("./types").AppUpdateAction) {
    return this.transport.invoke<import("./types").AppUpdateStatus>("app_update_action", { action });
  }

  onAppUpdate(handler: (status: import("./types").AppUpdateStatus) => void) {
    return this.transport.listen<import("./types").AppUpdateStatus>("app-update", handler);
  }

  /** The first ready paint: shows the hidden main window; resolves once it is on screen (ADR-098). */
  windowReady() {
    // A browser has no Mac window to show; the computer would refuse it (ADR-080).
    if (isRemoteUi) return Promise.resolve();
    return this.transport.invoke<void>("window_ready");
  }

  ciAutofixAction(action: import("./types").CiFixAction) {
    return this.transport.invoke<import("./types").CiFixState[]>("ci_autofix_action", { action });
  }

  prWatchAction(action: import("./types").PrWatchAction) {
    return this.transport.invoke<import("./types").PrWatch[]>("pr_watch_action", { action });
  }

  /** Guided reviews (ADR-125): one guide per pull request or session. */
  reviewGuideAction(action: import("./types").ReviewGuideAction) {
    return this.transport.invoke<import("./types").ReviewGuide | null>("review_guide_action", { action });
  }

  /** An agent wrote a guide, or one was deleted; `key` is `reviewGuideKey(target)`. */
  onReviewGuidesChanged(handler: (key: string) => void) {
    return this.transport.listen<{ key: string }>("review-guides-changed", (event) => handler(event.key));
  }

  onPrWatchChanged(handler: () => void) {
    return this.transport.listen<unknown>("pr-watch-changed", () => handler());
  }

  /** Settings saved by another window or the phone. */
  onSettingsChanged(handler: (settings: AppSettings) => void) {
    return this.transport.listen<AppSettings>("settings-changed", handler);
  }

  onCiAutofixChanged(handler: () => void) {
    return this.transport.listen<unknown>("ci-autofix-changed", () => handler());
  }

  /** A site's favicon by host only (never the full URL), fetched and cached natively (ADR-149). */
  siteFavicon(host: string) {
    return this.transport.invoke<string | null>("site_favicon", { host });
  }

  pullRequestAction(action: import("./types").PullRequestAction) {
    return this.transport.invoke<import("./types").PullRequestResponse>("pull_request_action", { action });
  }

  sideChatAction(action: import("./types").SideChatAction) {
    return this.transport.invoke<Session>("side_chat_action", { action });
  }
  pullAskAction(action: import("./types").PullAskAction) {
    return this.transport.invoke<Session>("pull_ask_action", { action });
  }

  teamAction(action: import("./types").TeamAction) {
    return this.transport.invoke<import("./types").TeamActionResponse>("team_action", { action });
  }

  respondAgentRequest(request: import("./types").RespondAgentRequest) {
    return this.transport.invoke<void>("respond_agent_request", { request });
  }

  stopAgent(sessionId: string) {
    return this.transport.invoke<void>("stop_agent", { sessionId });
  }

  /** Running sessions' process trees with CPU and memory; asked only while the panel needs it (ADR-144). */
  agentResources() {
    return this.transport.invoke<import("./types").ResourceSnapshot>("agent_resources");
  }

  agentResourceAction(action: import("./types").ResourceAction) {
    return this.transport.invoke<void>("agent_resource_action", { action });
  }

  startTerminal(sessionId: string, terminalId: string, cols: number, rows: number) {
    return this.transport.invoke<void>("start_terminal", { sessionId, terminalId, cols, rows });
  }

  writeTerminal(sessionId: string, terminalId: string, data: string) {
    return this.transport.invoke<void>("write_terminal", { sessionId, terminalId, data });
  }

  resizeTerminal(sessionId: string, terminalId: string, cols: number, rows: number) {
    return this.transport.invoke<void>("resize_terminal", { sessionId, terminalId, cols, rows });
  }

  stopTerminal(sessionId: string, terminalId: string) {
    return this.transport.invoke<void>("stop_terminal", { sessionId, terminalId });
  }

  /** One subagent's own messages, thinking and tools (ADR-146); `key` is its launching call, else its row id. */
  subagentFeed(sessionId: string, key: string) {
    return this.transport.invoke<import("./types").SubagentFeed>("subagent_feed", { sessionId, key });
  }

  onSubagentFeed(handler: (event: import("./types").SubagentFeedEvent) => void) {
    return this.transport.listen<import("./types").SubagentFeedEvent>("subagent-feed", handler);
  }

  onAgentOutput(handler: (event: AgentEvent) => void) {
    return this.transport.listen<AgentEvent>("agent-output", handler);
  }

  onSessionUpdated(handler: (session: Session) => void) {
    return this.transport.listen<Session>("session-updated", handler);
  }

  /** macOS reported memory pressure: drop what can be reloaded. */
  onMemoryPressure(handler: () => void) {
    return this.transport.listen<unknown>("memory-pressure", () => handler());
  }

  notificationAction(action: import("./types").NotificationAction) {
    return this.transport.invoke<import("./types").NotificationPermission>("notification_action", { action });
  }
  onActivityNotification(handler: (notice: import("./types").ActivityNotification) => void) {
    return this.transport.listen<import("./types").ActivityNotification>("notification-activity", handler);
  }
  onNotificationOpen(handler: (sessionId: string) => void) {
    return this.transport.listen<string>("notification-open", handler);
  }

  onAgentExit(handler: (event: AgentExitEvent) => void) {
    return this.transport.listen<AgentExitEvent>("agent-exit", handler);
  }

  onPtyOutput(handler: (event: PtyOutputEvent) => void) {
    return this.listenShared<PtyOutputEvent>("pty-output", handler);
  }

  onPtyExit(handler: (event: PtyOutputEvent) => void) {
    return this.listenShared<PtyOutputEvent>("pty-exit", handler);
  }

  dictationStatus() {
    if (isRemoteUi) return Promise.resolve<import("./types").DictationStatus>("unsupported");
    return this.transport.invoke<import("./types").DictationStatus>("dictation_status");
  }

  startDictation(locale: string) {
    return this.transport.invoke<void>("start_dictation", { locale });
  }

  /** Gives this Mac's alert key to another of the owner's computers (ADR-132 addendum); desktop only. */
  sharePushIdentity(baseUrl: string, token: string) {
    return this.transport.invoke<void>("share_push_identity", { baseUrl, token });
  }

  /** Whether the computer can transcribe a phone's recording (its speech recognition is allowed). */
  transcriptionStatus() {
    return this.transport.invoke<"available" | "unsupported">("transcription_status");
  }

  /** A phone's recording (AAC in MP4, base64) transcribed by the computer. */
  transcribeAudio(locale: string, audio: string) {
    return this.transport.invoke<string | null>("transcribe_audio", { locale, audio });
  }

  stopDictation(cancel = true) {
    return this.transport.invoke<string | null>("stop_dictation", { cancel });
  }

  browserOpen(sessionId: string) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_open", { sessionId });
  }

  browserClose(sessionId: string) {
    return this.transport.invoke<void>("browser_close", { sessionId });
  }

  browserState(sessionId: string) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_state", { sessionId });
  }

  /** A tab in a browser profile (`default`, `private` or a custom id); omitted, the Settings default (ADR-150). */
  browserNewTab(sessionId: string, url: string | null, profile?: string | null) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_new_tab", { sessionId, url, profile: profile ?? null });
  }

  browserCloseTab(sessionId: string, tabId: string) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_close_tab", { sessionId, tabId });
  }

  browserSelectTab(sessionId: string, tabId: string) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_select_tab", { sessionId, tabId });
  }

  browserNavigate(sessionId: string, tabId: string, url: string) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_navigate", { sessionId, tabId, url });
  }

  browserReload(sessionId: string, tabId: string) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_reload", { sessionId, tabId });
  }

  browserBack(sessionId: string, tabId: string) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_back", { sessionId, tabId });
  }

  browserForward(sessionId: string, tabId: string) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_forward", { sessionId, tabId });
  }

  browserSetBounds(sessionId: string, bounds: import("./types").BrowserBounds | null) {
    return this.transport.invoke<void>("browser_set_bounds", { sessionId, bounds });
  }

  browserAnnotateStart(sessionId: string, tabId: string) {
    return this.transport.invoke<void>("browser_annotate_start", { sessionId, tabId });
  }

  browserAnnotateFinish(sessionId: string, tabId: string) {
    return this.transport.invoke<import("./types").BrowserAnnotation[]>("browser_annotate_finish", { sessionId, tabId });
  }

  /** Finishes annotating and returns the marked elements with a screenshot of the page (ADR-151). */
  browserAnnotateCapture(sessionId: string, tabId: string) {
    return this.transport.invoke<import("./types").BrowserAnnotationCapture>("browser_annotate_capture", { sessionId, tabId });
  }

  /** Listening local servers worth opening for the session, read on demand (ADR-151). */
  localServersList(sessionId: string) {
    return this.transport.invoke<import("./types").LocalServer[]>("local_servers_list", { sessionId });
  }

  onBrowserAgentPage(handler: (event: import("./types").BrowserPageEvent) => void) {
    return this.transport.listen<import("./types").BrowserPageEvent>("browser-agent-page", handler);
  }

  onProjectPreview(handler: (event: import("./types").BrowserPageEvent) => void) {
    return this.transport.listen<import("./types").BrowserPageEvent>("project-preview", handler);
  }

  browserAnnotateCancel(sessionId: string, tabId: string) {
    return this.transport.invoke<void>("browser_annotate_cancel", { sessionId, tabId });
  }

  browserCopyLink(sessionId: string, tabId: string) {
    return this.transport.invoke<void>("browser_copy_link", { sessionId, tabId });
  }

  browserCapture(sessionId: string, tabId: string) {
    return this.transport.invoke<string>("browser_capture", { sessionId, tabId });
  }

  /** Device mode (ADR-111): the page's zoom, so a device's CSS width fits the pane. */
  browserSetZoom(sessionId: string, zoom: number) {
    return this.transport.invoke<void>("browser_set_zoom", { sessionId, zoom });
  }

  /** Device mode (`null` follows the pane), shared with agents (ADR-111, ADR-112). */
  browserSetDevice(sessionId: string, device: import("./types").BrowserDevice | null) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_set_device", { sessionId, device });
  }

  browserSetUserZoom(sessionId: string, zoom: number) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_set_user_zoom", { sessionId, zoom });
  }

  browserSetAppearance(sessionId: string, mode: "system" | "light" | "dark") {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_set_appearance", { sessionId, mode });
  }

  browserHardReload(sessionId: string, tabId: string) {
    return this.transport.invoke<void>("browser_hard_reload", { sessionId, tabId });
  }

  browserDevtools(sessionId: string, tabId: string) {
    return this.transport.invoke<void>("browser_devtools", { sessionId, tabId });
  }

  /** Answers the page's open JavaScript dialog from the pane (ADR-150). */
  browserDialogReply(sessionId: string, tabId: string, accept: boolean, text: string | null) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_dialog_reply", { sessionId, tabId, accept, text });
  }

  /** Deletes a custom browser profile's data and closes its tabs (ADR-150). */
  browserProfileDelete(profileId: string) {
    return this.transport.invoke<void>("browser_profile_delete", { profileId });
  }

  /** Whether custom browser profiles work on this Mac (macOS 14 and later) (ADR-150). */
  browserProfilesSupported() {
    return this.transport.invoke<boolean>("browser_profiles_supported", {});
  }

  /** Cookie import (ADR-152): the sources the owner can import from. */
  browserCookieSources() {
    return this.transport.invoke<import("./types").CookieImportSource[]>("browser_cookie_sources", {});
  }

  browserCookieDomains(sourceId: string) {
    return this.transport.invoke<import("./types").CookieDomain[]>("browser_cookie_domains", { sourceId });
  }

  /** Opens Full Disk Access; the address is fixed in the back end. */
  browserOpenFullDiskAccess() {
    return this.transport.invoke<void>("browser_open_full_disk_access", {});
  }

  browserCookieImport(profileId: string, sourceId: string, domains: string[]) {
    return this.transport.invoke<import("./types").CookieImportReport>("browser_cookie_import", { profileId, sourceId, domains });
  }

  /** Clears cookies or the cache of every session's browser (one shared store). */
  browserClearData(kind: "cookies" | "cache") {
    return this.transport.invoke<void>("browser_clear_data", { kind });
  }

  /** Key presses and clicks since the last read (JSON), for recordings (ADR-112). */
  browserInputEvents(sessionId: string, tabId: string) {
    return this.transport.invoke<string>("browser_input_events", { sessionId, tabId });
  }

  onBrowserRecordRequest(handler: (request: import("./types").BrowserRecordRequest) => void) {
    return this.transport.listen<import("./types").BrowserRecordRequest>("browser-record-request", handler);
  }

  browserRecordReply(requestId: string, result: unknown, error: string | null) {
    return this.transport.invoke<void>("browser_record_reply", { requestId, result, error });
  }

  /** Floats the active tab in an always-on-top window, or puts it back (ADR-111). */
  browserPictureInPicture(sessionId: string, open: boolean) {
    return this.transport.invoke<import("./types").BrowserSessionState>("browser_picture_in_picture", { sessionId, open });
  }

  /** One JPEG frame of the page (base64) for recording (ADR-111). */
  browserFrame(sessionId: string, tabId: string, maxWidth: number) {
    return this.transport.invoke<string>("browser_frame", { sessionId, tabId, maxWidth });
  }

  browserRecordingStart(extension: "mp4" | "webm") {
    return this.transport.invoke<string>("browser_recording_start", { extension });
  }

  browserRecordingAppend(id: string, chunk: string) {
    return this.transport.invoke<void>("browser_recording_append", { id, chunk });
  }

  /** Closes the recording file and shows it in Finder; `null` when nothing was recorded. */
  browserRecordingFinish(id: string, reveal: boolean) {
    return this.transport.invoke<string | null>("browser_recording_finish", { id, reveal });
  }


  computerAction(action: import("./types").ComputerAction) {
    return this.transport.invoke<import("./types").ComputerSnapshot>("computer_action", { action });
  }

  windowSnapAction(action: import("./types").WindowSnapAction) {
    return this.transport.invoke<import("./types").PromptAttachment[]>("window_snap_action", { action });
  }

  onWindowSnap(handler: (event: import("./types").WindowSnapEvent) => void) {
    return this.transport.listen<import("./types").WindowSnapEvent>("window-snap", handler);
  }

  /** The prompt link waiting since the last call (ADR-122), or null. */
  takePromptLink() {
    if (isRemoteUi) return Promise.resolve(null);
    return this.transport.invoke<import("./types").PromptLinkOutcome | null>("take_prompt_link");
  }

  buildPromptLink(input: { projectId: string | null; prompt: string; provider: AgentProviderId | null; model: string | null }) {
    return this.transport.invoke<string>("build_prompt_link", input);
  }

  /** A prompt link is waiting; take it with `takePromptLink`. */
  onPromptLink(handler: () => void) {
    return this.transport.listen<unknown>("prompt-link", () => handler());
  }

  secretAction(action: import("./types").SecretAction) {
    return this.transport.invoke<import("./types").SecretSnapshot>("secret_action", { action });
  }

  // Browser login vault (ADR-186). Passwords go in with `loginVaultSave` and never come back.
  loginVaultList() {
    return this.transport.invoke<import("./types").LoginSummary[]>("login_vault_list", {});
  }

  loginVaultSave(input: import("./types").LoginInput) {
    return this.transport.invoke<import("./types").LoginSummary>("login_vault_save", { input });
  }

  loginVaultDelete(id: string) {
    return this.transport.invoke<void>("login_vault_delete", { id });
  }

  loginVaultSetAgentUse(id: string, enabled: boolean) {
    return this.transport.invoke<import("./types").LoginSummary>("login_vault_set_agent_use", { id, enabled });
  }

  loginVaultMatches(url: string) {
    return this.transport.invoke<import("./types").LoginSummary[]>("login_vault_matches", { url });
  }

  loginVaultPrompts() {
    return this.transport.invoke<import("./types").LoginPrompts>("login_vault_prompts", {});
  }

  loginVaultAnswerOffer(id: string, save: boolean, agentUse: boolean) {
    return this.transport.invoke<void>("login_vault_answer_offer", { id, save, agentUse });
  }

  loginVaultAnswerApproval(id: string, allow: boolean) {
    return this.transport.invoke<void>("login_vault_answer_approval", { id, allow });
  }

  loginVaultAutofill(sessionId: string, loginId: string, part?: "login" | "totp") {
    return this.transport.invoke<void>("login_vault_autofill", { sessionId, loginId, part });
  }

  onLoginVaultState(handler: (state: import("./types").LoginPrompts) => void) {
    return this.transport.listen<import("./types").LoginPrompts>("login-vault-state", handler);
  }

  onSecretState(handler: (state: import("./types").SecretSnapshot) => void) {
    return this.transport.listen<import("./types").SecretSnapshot>("secret-state", handler);
  }

  onComputerState(handler: (state: import("./types").ComputerSnapshot) => void) {
    return this.transport.listen<import("./types").ComputerSnapshot>("computer-state", handler);
  }

  onBrowserState(handler: (state: import("./types").BrowserSessionState) => void) {
    return this.transport.listen<import("./types").BrowserSessionState>("browser-state", handler);
  }

  remoteAction(action: import("./types").RemoteAction) {
    return this.transport.invoke<import("./types").RemoteStatus>("remote_action", { action });
  }

  onRemoteChanged(handler: () => void) {
    return this.transport.listen<unknown>("remote-changed", () => handler());
  }

  /** Devices pane (ADR-128): checks, consented install, hub leases and simulator actions. */
  deviceAction<T = unknown>(action: import("./types").DeviceAction) {
    return this.transport.invoke<T>("device_action", { action });
  }

  deviceStatus() {
    return this.deviceAction<import("./types").DeviceStatus>({ type: "status" });
  }

  onDeviceChanged(handler: () => void) {
    return this.transport.listen<unknown>("device-changed", () => handler());
  }

  deviceHubAccess(proxy: import("./types").DeviceProxy | null) {
    return this.transport.deviceHubAccess?.(proxy) ?? null;
  }

  /** An image a reply links to on the Mac, as a data URL (ADR-085). */
  replyImage(sessionId: string, path: string) {
    return this.transport.invoke<string>("reply_image", { sessionId, path });
  }

  /** A `sirus-media` token for an image or video a reply links to, shown inline (ADR-119). */
  replyMedia(sessionId: string, path: string) {
    return this.transport.invoke<{ token: string; size: number; mime: string }>("reply_media", { sessionId, path });
  }

  remotePair() {
    return this.transport.invoke<import("./types").RemotePairing>("remote_pair");
  }

}

/** True when this UI was served to another device by the Mac's remote access (ADR-080). */
export const isRemoteUi = typeof window !== "undefined" && !("__TAURI_INTERNALS__" in window);

const remoteListeners = new Set<(state: RemoteConnection, previous: RemoteConnection) => void>();
const environmentListeners = new Set<(id: string, state: RemoteConnection, previous: RemoteConnection) => void>();
const switchListeners = new Set<(id: string) => void>();
let remoteState: RemoteConnection = "idle";

/** Whether the UI served to another device can reach the Mac right now. */
export function remoteReachable(): boolean {
  return !isRemoteUi || remoteState === "open";
}

/** Follows the active computer's connection; only the UI served to another device has one. */
export function onRemoteConnection(handler: (state: RemoteConnection, previous: RemoteConnection) => void): () => void {
  remoteListeners.add(handler);
  return () => { remoteListeners.delete(handler); };
}

/** Follows every known computer's connection (ADR-132). */
export function onEnvironmentConnection(handler: (id: string, state: RemoteConnection, previous: RemoteConnection) => void): () => void {
  environmentListeners.add(handler);
  return () => { environmentListeners.delete(handler); };
}

/** Called right after the phone switched to another computer, before it reloads. */
export function onEnvironmentSwitch(handler: (id: string) => void): () => void {
  switchListeners.add(handler);
  return () => { switchListeners.delete(handler); };
}

/**
 * One transport per computer the phone knows (ADR-132), made on first use. The token is read
 * each time from the phone's list, so a computer paired again uses its new token.
 */
const transports = new Map<string, RemoteTransport>();
let activeEnvironment = HOME_ENVIRONMENT;

function environmentsHere() {
  return readEnvironments(window.localStorage, window.location.origin);
}

/** The transport of one computer; `null` when it is not known on this phone (or is this Mac, ADR-133). */
export function environmentTransport(id: string): RemoteTransport | null {
  const existing = transports.get(id);
  if (existing) return existing;
  if (!isRemoteUi && id === HOME_ENVIRONMENT) return null;
  const environment = environmentsHere().find((item) => item.id === id);
  if (!environment) return null;
  const home = id === HOME_ENVIRONMENT;
  const transport = new RemoteTransport({
    url: environment.baseUrl ? socketUrlFor(environment.baseUrl) : socketUrl(location),
    baseUrl: home ? "" : environment.baseUrl,
    token: () => environmentsHere().find((item) => item.id === id)?.token || null,
    onConnection: (state, previous) => {
      for (const handler of [...environmentListeners]) handler(id, state, previous);
      if (id !== activeEnvironment) return;
      remoteState = state;
      for (const handler of [...remoteListeners]) handler(state, previous);
    },
  });
  transports.set(id, transport);
  return transport;
}

/** A client for a computer that is not the active one: the Home's summaries use it. */
const environmentClients = new Map<string, SirusClient>();
export function environmentClient(id: string): SirusClient | null {
  const transport = environmentTransport(id);
  if (!transport) return null;
  let made = environmentClients.get(id);
  if (!made) { made = new SirusClient(transport); environmentClients.set(id, made); }
  return made;
}

/** Drops a computer's transport (forgotten, or paired again with a new token). */
export function forgetEnvironmentTransport(id: string) {
  transports.get(id)?.dispose();
  transports.delete(id);
  environmentClients.delete(id);
}

export function activeEnvironmentId(): string {
  return activeEnvironment;
}

/** This Mac's own IPC in the desktop app; the desktop's home computer (ADR-133). */
const localTransport = isRemoteUi ? null : new LocalTransport();
/** This Mac whatever computer the window shows: remote access, its own conversations' summary. */
export const localClient: SirusClient | null = localTransport ? new SirusClient(localTransport) : null;

const switching = isRemoteUi
  ? (() => {
    activeEnvironment = chooseActive(environmentsHere(), window.localStorage.getItem(ACTIVE_ENVIRONMENT_KEY));
    return new SwitchingTransport(environmentTransport(activeEnvironment) ?? environmentTransport(HOME_ENVIRONMENT)!);
  })()
  // The desktop always starts on this Mac; window-bound commands and events stay on it.
  : localTransport ? new SwitchingTransport(localTransport, { transport: localTransport, command: runsOnThisMac, event: eventFromThisMac }) : null;

/**
 * Makes another computer the active one: `client` and every listener move to it. The caller
 * reloads the store (`bootstrap`) afterwards.
 */
export function switchEnvironment(id: string): boolean {
  const transport = !isRemoteUi && id === HOME_ENVIRONMENT ? localTransport : environmentTransport(id);
  if (!switching || !transport) return false;
  if (id === activeEnvironment && switching.active === transport) return true;
  const previous = remoteState;
  activeEnvironment = id;
  // The desktop opens on this Mac every time; only the phone remembers its computer.
  if (isRemoteUi) window.localStorage.setItem(ACTIVE_ENVIRONMENT_KEY, id);
  switching.use(transport);
  for (const handler of [...switchListeners]) handler(id);
  remoteState = transport instanceof RemoteTransport ? transport.connection : "open";
  for (const handler of [...remoteListeners]) handler(remoteState, previous);
  return true;
}

export const client = new SirusClient(switching ?? new LocalTransport());

/** Reconnects the UI served to another device at once, e.g. when the app returns to the front. */
export function wakeRemote() {
  for (const transport of transports.values()) transport.wake();
}

function isCancelledTitle(value: unknown): boolean {
  return !!value && typeof value === "object" && Object.keys(value).length === 1 && (value as Record<string, unknown>).type === "cancelled";
}
