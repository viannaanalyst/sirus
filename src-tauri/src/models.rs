use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Idle,
    Starting,
    Running,
    Waiting,
    Completed,
    Failed,
    Stopped,
}

impl SessionStatus {
    pub fn is_active(&self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Waiting)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum AgentProviderId {
    Codex,
    Claude,
    #[serde(rename = "opencode")]
    OpenCode,
    Cursor,
    Grok,
    Antigravity,
    Droid,
    Pi,
    Devin,
    Hermes,
}

impl AgentProviderId {
    pub fn key(&self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::Claude => "claude",
            Self::OpenCode => "opencode",
            Self::Cursor => "cursor",
            Self::Grok => "grok",
            Self::Antigravity => "antigravity",
            Self::Droid => "droid",
            Self::Pi => "pi",
            Self::Devin => "devin",
            Self::Hermes => "hermes",
        }
    }

    /// Model-facing title; keep aligned with `provider-registry.ts` names.
    pub fn title(&self) -> &'static str {
        match self {
            Self::Codex => "Codex",
            Self::Claude => "Claude Code",
            Self::OpenCode => "OpenCode",
            Self::Cursor => "Cursor",
            Self::Grok => "Grok",
            Self::Antigravity => "Antigravity",
            Self::Droid => "Droid",
            Self::Pi => "Pi",
            Self::Devin => "Devin",
            Self::Hermes => "Hermes",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MessageRole {
    User,
    Agent,
    System,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ChangeKind {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub added_at: String,
    pub last_opened_at: String,
    /// Display-only folder colour, emoji or logo (ADR-059); empty means the plain folder.
    #[serde(default, skip_serializing_if = "ProjectLook::is_empty")]
    pub look: ProjectLook,
    /// The pixel speech-bubble avatar the owner customised (ADR-175); absent means the
    /// defaults derived from the project name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<ProjectAppearance>,
    /// Owner-saved Setup and On finish shell scripts (ADR-078); omitted when empty.
    #[serde(
        default,
        skip_serializing_if = "crate::project_scripts::ProjectScripts::is_empty"
    )]
    pub scripts: crate::project_scripts::ProjectScripts,
}

/// A project's avatar choices (ADR-175, v2). Colour is `#rrggbb`; `eyes` and `accessory` are names
/// from `sidebar::EYES` and `sidebar::ACCESSORIES`. Rows saved before v2 still load: old eye names
/// become their v2 equivalent, `mouth` and `cheeks` are ignored (unknown fields are skipped), and a
/// missing `accessory` means the default taken from the project's name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectAppearance {
    pub color: String,
    #[serde(deserialize_with = "crate::sidebar::deserialize_eyes")]
    pub eyes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accessory: Option<String>,
}

/// A folder in the project switcher that groups projects (2026-10-09). Display only:
/// no directory is created or moved. Its icon follows the same rules as a project's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectFolder {
    pub id: String,
    pub name: String,
    pub look: ProjectLook,
    /// Projects in the folder, in their switcher order. A project is in at most one folder.
    pub project_ids: Vec<String>,
    pub collapsed: bool,
}

/// How a project's sidebar icon looks: one of a logo, an emoji, an Astro icon or a line icon;
/// the colour tints it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProjectLook {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub emoji: Option<String>,
    /// `data:image/png;base64,…` of a 96×96 PNG made natively from a picked image.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    /// One of the Astro cosmic icons (ADR-059, ADR-116), drawn in the project colour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub astro: Option<ProjectAstroIcon>,
    /// One of the fixed line icons (`project_look::ICONS`), drawn in the project colour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

/// An Astro icon as a project icon: one of `project_look::ASTRO_ICONS` in one of `ASTRO_STYLES`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProjectAstroIcon {
    pub icon: String,
    pub style: String,
}

impl ProjectLook {
    pub fn is_empty(&self) -> bool {
        self.color.is_none()
            && self.emoji.is_none()
            && self.logo.is_none()
            && self.astro.is_none()
            && self.icon.is_none()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Worktree {
    pub path: String,
    pub branch: String,
    pub isolated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub id: String,
    pub session_id: String,
    pub role: MessageRole,
    pub content: String,
    pub created_at: String,
    pub streaming: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<crate::activity::TurnActivity>,
    /// Instructions the person sent into this running reply (steering); `offset`
    /// is the reply's UTF-16 length when each arrived, so it renders in place.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steers: Vec<Steer>,
    /// User messages: what was attached; images keep a thumbnail file (ADR-071).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MessageAttachment>,
    /// Agent replies: sessions started or delegated during the turn (ADR-111).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub launched: Vec<String>,
    /// Agent replies: HTML visuals the agent published with `sirus_show_visual` (ADR-123).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub visuals: Vec<MessageVisual>,
    /// User messages: the sender's id for this send (a UUID), so a delivery that arrives
    /// twice starts one turn (idempotent sends).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_request_id: Option<String>,
    /// User messages: this send started a fresh provider session (ADR-181).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotation: Option<ContextRotation>,
}

/// A self-contained HTML visual shown as a card in an agent reply (ADR-123). It only ever
/// runs in the sandboxed `sirus-preview://` frame; a later call with the same title in the
/// same turn replaces it and bumps `revision`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageVisual {
    pub id: String,
    pub title: String,
    pub html: String,
    /// A native visual's JavaScript program (`visual_show`), drawn by Sirus Native; `html` then
    /// only holds a placeholder for the web app.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub created_at: String,
    #[serde(default)]
    pub revision: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageAttachment {
    /// The sent snapshot, so "Send again" can resend it while the app keeps it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    /// `file` or `folder`.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// File size in bytes (files only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    /// Legacy or fallback only: `data:image/jpeg;base64,…`, at most 1280 px on its longer
    /// side. Sent images keep it as a file instead (`has_thumbnail`, `thumbnails.rs`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thumbnail: Option<String>,
    /// The JPEG copy (at most 1280 px) is `thumbnails/<id>.jpg` in the app data folder.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub has_thumbnail: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Steer {
    pub text: String,
    pub at: String,
    pub offset: usize,
    /// Screenshots and files steered with the text.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<MessageAttachment>,
}

/// Exact vendor identity bound to the owning native session and validated workspace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NativeThread {
    #[serde(default = "default_account_id")]
    pub provider_account_id: String,
    pub thread_id: String,
    pub session_id: String,
    pub project_id: String,
    pub cwd: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputOption {
    pub label: String,
    pub description: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputQuestion {
    pub id: String,
    pub header: String,
    pub question: String,
    #[serde(default)]
    pub is_other: bool,
    #[serde(default)]
    pub is_secret: bool,
    pub options: Option<Vec<InputOption>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProposedFileChange {
    pub path: String,
    pub kind: String,
    pub diff: String,
    pub move_path: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum PendingRequestKind {
    Command {
        command: String,
        cwd: Option<String>,
        reason: Option<String>,
    },
    FileChange {
        reason: Option<String>,
        changes: Vec<ProposedFileChange>,
    },
    UserInput {
        questions: Vec<InputQuestion>,
    },
    Tool {
        name: String,
        input: serde_json::Value,
        reason: Option<String>,
        /// What made Claude ask (`rule`, `hook` or `check`), so a full-access card can say why.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        asked_by: Option<String>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRequest {
    pub request_id: String,
    pub generation: String,
    pub turn_id: String,
    pub item_id: String,
    pub kind: PendingRequestKind,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ApprovalDecision {
    Accept,
    Decline,
    Cancel,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase", deny_unknown_fields)]
pub enum AgentResponse {
    Approval {
        decision: ApprovalDecision,
    },
    UserInput {
        answers: std::collections::BTreeMap<String, Vec<String>>,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RespondAgentRequest {
    pub session_id: String,
    pub generation: String,
    pub request_id: String,
    pub turn_id: String,
    pub response: AgentResponse,
}

/// The provider refused the turn because the account's usage limit is spent.
/// `resets_at` (UTC milliseconds) comes from the provider's own rate-limit report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageLimit {
    #[serde(default)]
    pub resets_at: Option<i64>,
}

/// Tokens the conversation occupies and the model's window, as the provider reports them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContextUsage {
    pub used: u64,
    #[serde(default)]
    pub window: Option<u64>,
    /// When Claude's prompt cache for this conversation expires (Unix ms): the last main
    /// request's start plus its cache lifetime (1 hour on a subscription, 5 minutes with an
    /// API key). Absent when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_expires_at: Option<i64>,
    /// The smallest reading seen: what the provider's own prompt and tools cost. A compaction's
    /// boundary reports the rebuilt conversation without it, so the reading after a compaction
    /// adds this back until the next reply reports the real figure.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub floor: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal: Option<String>,
    #[serde(default)]
    pub pinned_message_ids: Vec<String>,
    #[serde(default)]
    pub fork_origin: Option<ForkOrigin>,
    #[serde(default)]
    pub import_origin: Option<ImportOrigin>,
    /// This conversation continued on another computer (ADR-133).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_to: Option<SessionMoveNote>,
    /// This conversation was moved here from another computer (ADR-133).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub moved_from: Option<SessionMoveNote>,
    /// Pending turn-level handoff: the target provider receives a recap of the
    /// source conversation on the first send, then this is consumed.
    #[serde(default)]
    pub handoff: Option<HandoffOrigin>,
    #[serde(default)]
    pub account_bindings: std::collections::HashMap<AgentProviderId, String>,
    /// Latest context-window reading the provider reported (ADR-057); not inferred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_usage: Option<ContextUsage>,
    /// Set when the last turn hit the provider's usage limit; cleared by the next send.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage_limit: Option<UsageLimit>,
    /// When the person last pressed Stop (ADR-101). Until they send again, nothing the app
    /// runs on its own (PR watch, CI auto-fix, delegated results) starts a turn here.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_by_user_at: Option<String>,
    /// Automatic continuations after a crash in a row (ADR-118); at most three, cleared when
    /// a turn ends normally or another prompt is sent.
    #[serde(default, skip_serializing_if = "crate::models::is_zero")]
    pub auto_continuations: u32,
    /// RFC 3339 instant the conversation returns from snooze (ADR-103); it is hidden from
    /// session lists and tabs until then, and the native timer brings it back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<String>,
    /// When a snooze reminder fired (ADR-103): the conversation reads as unread until it
    /// is opened, which clears this through `snooze_session` with no time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snooze_reminder_at: Option<String>,
    pub id: String,
    pub title: String,
    pub project_id: String,
    pub agent: AgentProviderId,
    #[serde(default = "default_account_id")]
    pub provider_account_id: String,
    pub status: SessionStatus,
    pub created_at: String,
    pub last_activity_at: String,
    pub worktree: Worktree,
    /// Each transcript has its own file and loader (ADR-047); memory keeps only
    /// loaded ones (ADR-099), and an unloaded one is read when it is used.
    #[serde(default, skip_serializing_if = "crate::persist::omit_transcripts")]
    pub messages: crate::transcript_store::Messages,
    pub last_error: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub native_thread: Option<NativeThread>,
    #[serde(default)]
    pub execution: ExecutionOptions,
    // Pending callbacks belong to a live process only; persisted/UI input cannot restore them.
    #[serde(default, skip_deserializing)]
    pub pending_requests: Vec<PendingRequest>,
    /// Coordinator sessions own their team plan and progress (ADR-043).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<crate::team::Team>,
    /// Worker sessions point back to their coordinator task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_worker: Option<crate::team::TeamWorker>,
    /// A side chat points back to the main session it answers about (ADR-049).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side_chat: Option<crate::side_chat::SideChatOrigin>,
    /// An "Ask" conversation about a GitHub pull request or issue (ADR-050): hidden from
    /// session lists and always read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pull_ask: Option<crate::pull_ask::PullAskOrigin>,
    /// The session that delegated this one with `sirus_delegate` or started it with
    /// `sirus_create_session` (ADR-111): its breadcrumb, and whether the parent hears back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegated_by: Option<crate::delegation::DelegatedBy>,
    /// Project script runs in this session's worktree and whether Setup is still due (ADR-078).
    #[serde(
        default,
        skip_serializing_if = "crate::project_scripts::SessionScripts::is_empty"
    )]
    pub scripts: crate::project_scripts::SessionScripts,
}

impl Session {
    /// A side chat (ADR-049) or a pull request Ask conversation (ADR-050): shown only
    /// beside what it answers about, never in session lists.
    pub fn is_auxiliary(&self) -> bool {
        self.side_chat.is_some() || self.pull_ask.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkOrigin {
    #[serde(default)]
    pub seeded_native_thread_id: Option<String>,
    pub source_session_id: String,
    pub source_message_id: String,
    pub source_title: String,
    pub inherited_message_count: usize,
}

/// Recap a handed-off provider receives on its first turn. `request` is the
/// last user message at the chosen response, shown on the composer card.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HandoffOrigin {
    pub from: AgentProviderId,
    pub brief: String,
    pub request: String,
    #[serde(default)]
    pub pending: bool,
    /// The recap went out with a prompt the provider has not taken yet (ADR-101). A turn that
    /// ends before taking it gives the recap back; one that took it spends it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delivering: bool,
    /// A context rotation, not a provider switch: the same provider continues in a fresh
    /// session seeded with a code-built brief (ADR-181).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rotated: bool,
}

/// A user message that began a fresh provider session because the context was nearly full
/// (ADR-181): the transcript shows a marker above it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ContextRotation {
    pub used: u64,
    pub window: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SessionWorkspacePref {
    Ask,
    Checkout,
    Worktree,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ThemePref {
    Dark,
    Light,
    System,
    /// Retained only for migrating older state; incoming saves reject it.
    Translucent,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum UiFont {
    Inter,
    Geist,
    DmSans,
    PlexSans,
    Humanist,
    Helvetica,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MonoFont {
    PlexMono,
    Jetbrains,
    Fira,
    GeistMono,
    Source,
    Roboto,
    Ubuntu,
    SfMono,
    Menlo,
    Cascadia,
    Hack,
    Consolas,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DockIcon {
    Default,
    SmokedGlass,
    White,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DensityPref {
    Compact,
    Default,
    Comfortable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum WorktreeLocationPref {
    Automatic,
    Custom,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum ComposerLineSpeed {
    Slow,
    Smooth,
    Fast,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SidebarProjectSortOrder {
    /// Retired "Recent activity" (`updated_at`) values load as Manual.
    #[serde(alias = "updated_at")]
    Manual,
    CreatedAt,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SidebarThreadSortOrder {
    UpdatedAt,
    CreatedAt,
}

/// What the sidebar shows when it is not hidden (ADR-139): the icon rail or the
/// conversation list. `sidebar_collapsed` still hides either. The removed Project style
/// loads as the list.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SidebarMode {
    #[default]
    Rail,
    #[serde(alias = "project")]
    List,
}

/// The conversation list's order (ADR-139): flat by last activity, or grouped by project.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SidebarListOrder {
    #[default]
    Recent,
    Project,
}

/// Chat behavior (ADR-149): where a plain click on a web link opens, the conversation's
/// Sirus browser or the system browser.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum LinkOpenTarget {
    #[default]
    App,
    System,
}

/// Closed choices for the global window snap shortcut.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WindowSnapShortcut {
    #[default]
    ControlOptionCommandS,
    OptionShiftS,
    ControlShiftS,
}

/// Release notes of an installed update (What's new, ADR-140): bounded, shown once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateNotes {
    pub version: String,
    pub notes: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
    /// What's new was shown (or dismissed) for this version.
    #[serde(default)]
    pub seen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AppSettings {
    pub notifications: crate::notifications::Preferences,
    pub usage_providers: Vec<AgentProviderId>,
    /// Providers whose quota ring shows in the sidebar rail (at most two).
    pub sidebar_usage_providers: Vec<AgentProviderId>,
    /// Messages sent while a Codex/Claude reply runs steer it instead of waiting in the queue.
    pub steer_while_running: bool,
    /// Owner-authorized CI auto-fix: failing PR checks start an automatic fix turn,
    /// then Sirus commits and pushes the session branch (ADR-064).
    pub ci_auto_fix: bool,
    /// Owner switch for PR watch (ADR-079): sessions may watch their own pull
    /// request and get an automatic turn on new checks, reviews or conflicts.
    pub pr_watch: bool,
    /// Chat behavior: Enter while dictating stops and sends instead of only stopping.
    pub dictation_enter_sends: bool,
    /// Chat behavior: a finished turn folds its steps into the "Worked for" line.
    pub fold_finished_turns: bool,
    /// The "Working" panel beside the composer, shown while two or more sessions work (ADR-094).
    #[serde(default = "default_show_working_panel")]
    pub show_working_panel: bool,
    /// Chat behavior: macOS spelling and autocorrect in the composers (on by default).
    #[serde(default = "default_true")]
    pub composer_autocorrect: bool,
    /// Chat behavior: the project's conversations as tabs in the title bar.
    #[serde(default = "default_true")]
    pub show_session_tabs: bool,
    /// Chat behavior: after a crash, conversations cut mid-turn continue on their own (ADR-118).
    #[serde(default = "default_true")]
    pub continue_interrupted_sessions: bool,
    /// Chat behavior (beta, off by default): an idle Claude conversation with a large context
    /// compacts shortly before its prompt cache expires (ADR-157).
    pub compact_idle_before_cache_expiry: bool,
    /// Chat behavior (off by default): when a Claude or Codex conversation's reported context
    /// reaches 80% of its window, the next send starts a fresh provider session seeded with a
    /// brief built by code, with no model call (ADR-181).
    #[serde(default)]
    pub rotate_at_context_limit: bool,
    /// Sounds (off by default): a soft chime when an agent finishes a turn or needs approval or an answer.
    /// The window plays it, and never for the conversation in view while the window is focused.
    #[serde(default)]
    pub agent_sounds: bool,
    /// Providers → Advanced: how many finished conversations keep their agent running for quick
    /// follow-ups (ADR-108). 16 means all of them (the default).
    #[serde(default = "default_keep_idle_agents")]
    pub keep_idle_agents: u32,
    /// Browser recordings (ADR-112): frames per second (30 or 60) and whether key presses and
    /// clicks are drawn over the video.
    #[serde(default = "default_recording_fps")]
    pub browser_recording_fps: u32,
    #[serde(default = "default_true")]
    pub browser_recording_keys: bool,
    #[serde(default = "default_true")]
    pub browser_recording_clicks: bool,
    /// Browser (ADR-151): pages an agent opens or navigates float over the conversation.
    #[serde(default = "default_true")]
    pub browser_show_agent_pages: bool,
    /// Chat behavior: GitHub pull request/issue links in replies open the Pull requests page.
    pub github_links_in_app: bool,
    /// Chat behavior: web links in chat and terminals open in the Sirus browser or the system one.
    pub link_open_target: LinkOpenTarget,
    /// Chat behavior: diffs wrap long lines by default.
    pub diff_word_wrap: bool,
    /// Chat behavior: ask before closing a terminal tab.
    pub confirm_terminal_close: bool,
    /// System-wide shortcut that snaps the frontmost app window into the open composer (ADR-054).
    pub window_snap_enabled: bool,
    pub window_snap_shortcut: WindowSnapShortcut,
    /// Pinned pull requests/issues in the review inbox, as `owner/repo#number` (ADR-050).
    pub github_pins: Vec<String>,
    /// Unread tracking on the Pull requests page: `owner/repo#number` → the RFC 3339 `updatedAt`
    /// last seen (at most 500), and the time before which everything counts as seen.
    pub github_seen: std::collections::BTreeMap<String, String>,
    pub github_seen_floor: Option<String>,
    /// Pull request alerts already given (ADR-050): `opened:owner/repo#n` or
    /// `review:owner/repo#n`, newest first, at most 200, so a restart does not repeat them.
    pub github_notified: Vec<String>,
    /// Rail item order and hidden items (closed IDs; Home cannot be hidden).
    pub rail_item_order: Vec<String>,
    pub hidden_rail_items: Vec<String>,
    /// Project IDs pinned as rail shortcuts.
    pub rail_project_shortcuts: Vec<String>,
    pub custom_shortcuts: std::collections::HashMap<String, String>,
    pub default_agent: AgentProviderId,
    pub open_last_project: bool,
    /// The chat background image saved in `backgrounds/` (ADR-089), if any.
    pub chat_background: Option<String>,
    /// How the image is drawn: none, dither, ascii, halftone, scanlines or haze.
    pub chat_background_effect: String,
    /// Where it shows: `empty` conversations only, or `all` of them.
    pub chat_background_show_on: String,
    /// Image strength before a conversation has messages, in percent.
    pub chat_background_empty_visibility: u8,
    /// Image strength once the conversation has messages, in percent.
    pub chat_background_session_visibility: u8,
    pub worktree_base_path: Option<String>,
    pub default_session_workspace: SessionWorkspacePref,
    pub confirm_close_running: bool,
    pub restore_previous_sessions: bool,
    pub check_for_updates: bool,
    pub disabled_providers: Vec<AgentProviderId>,
    pub provider_paths: std::collections::HashMap<AgentProviderId, String>,
    pub theme: ThemePref,
    pub dark_window_translucent: bool,
    pub light_window_translucent: bool,
    pub dark_window_opacity: u32,
    pub light_window_opacity: u32,
    pub dark_sidebar_translucent: bool,
    pub light_sidebar_translucent: bool,
    pub dark_sidebar_opacity: u32,
    pub light_sidebar_opacity: u32,
    pub translucent_opacity: u32,
    pub system_ui_font: bool,
    pub ui_font: UiFont,
    pub code_font: MonoFont,
    pub code_font_size: u32,
    pub terminal_font: MonoFont,
    pub font_smoothing: bool,
    pub dock_icon: DockIcon,
    /// Imported VS Code colour themes (ADR-185), bounded and validated; the two ids are the
    /// ones in use for the light and dark palettes (none means Default (Sirus)).
    pub vscode_themes: Vec<crate::vscode_theme::VsCodeTheme>,
    pub light_vscode_theme: Option<String>,
    pub dark_vscode_theme: Option<String>,
    pub density: DensityPref,
    pub animations: bool,
    pub composer_line_speed: ComposerLineSpeed,
    /// Deprecated compatibility preference; native appearance uses the typed controls above.
    pub glass: bool,
    pub pointer_glow: bool,
    pub reduce_motion: bool,
    pub ui_font_size: u32,
    pub git_auto_fetch: bool,
    pub git_show_untracked: bool,
    pub git_confirm_destructive: bool,
    pub worktree_location: WorktreeLocationPref,
    pub worktree_branch_pattern: String,
    pub terminal_use_system_shell: bool,
    pub terminal_font_size: u32,
    pub terminal_cursor_style: String,
    pub terminal_scrollback: u32,
    pub developer_logs: bool,
    /// Offers the app-owned computer-use MCP server to Codex/Claude/OpenCode turns (ADR-038).
    pub computer_use_enabled: bool,
    /// Lists the `sirus_*` session tools on the per-session MCP bridge (ADR-076).
    pub agents_manage_sessions: bool,
    pub experimental: bool,
    #[serde(default)]
    pub disabled_models: Vec<String>,
    pub disabled_skills: Vec<String>,
    #[serde(default)]
    pub favorite_models: Vec<String>,
    #[serde(default)]
    pub default_model: Option<String>,
    #[serde(default)]
    pub model_execution: std::collections::HashMap<String, ExecutionOptions>,
    #[serde(default)]
    pub sidebar_collapsed: bool,
    /// Icon rail or conversation list (ADR-139); older settings load as the rail.
    pub sidebar_mode: SidebarMode,
    pub sidebar_list_order: SidebarListOrder,
    /// Projects whose folder is open in the list's By project order (ADR-139, round 2);
    /// at most `SIDEBAR_OPEN_FOLDERS_LIMIT` owned project ids. `None` until the person opens
    /// or closes one: then the selected project's folder shows open.
    #[serde(default)]
    pub sidebar_open_folders: Option<Vec<String>>,
    pub sidebar_project_order: Vec<String>,
    pub sidebar_project_sort_order: SidebarProjectSortOrder,
    pub sidebar_thread_sort_order: SidebarThreadSortOrder,
    /// Working, waiting and unread sessions float above the thread order (ADR-121).
    #[serde(default)]
    pub sidebar_attention_first: bool,
    pub pinned_project_ids: Vec<String>,
    pub project_folders: Vec<ProjectFolder>,
    pub pinned_session_ids: Vec<String>,
    /// Remote MCP servers every supported agent receives, per project or for all (ADR-137).
    #[serde(default)]
    pub connectors: Vec<crate::connectors::Connector>,
    #[serde(default)]
    pub environment_panel_default_open: bool,
    pub show_environment_usage: bool,
    pub show_environment_repository: bool,
    pub show_environment_editor: bool,
    pub show_environment_pull_request: bool,
    pub show_environment_pinned: bool,
    pub show_environment_notepad: bool,
    pub show_environment_instructions: bool,
    #[serde(default = "default_true")]
    pub enable_provider_update_checks: bool,
    /// A turn stopped by the provider's usage limit continues by itself at the reset
    /// (ADR-063); off by default, each notice can cancel it.
    #[serde(default)]
    pub usage_auto_resume: bool,
    /// In-app updates (ADR-140): release builds look for a new version on their own.
    #[serde(default = "default_true")]
    pub app_update_auto_check: bool,
    /// An update found on its own is downloaded and installed in the background and waits
    /// for a restart (it also starts the next time the app opens).
    #[serde(default = "default_true")]
    pub app_update_auto_download: bool,
    /// The first launch of a new version shows its notes once (What's new).
    #[serde(default = "default_true")]
    pub app_update_whats_new: bool,
    /// The notes of the last installed update, kept for What's new and Settings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_update_notes: Option<AppUpdateNotes>,
    #[serde(default = "default_locale")]
    pub locale: String,
    /// New conversations: the approval mode they start on when the provider offers it
    /// (ADR-130). Full access, as before; a provider without it steps down, never up.
    pub default_approval: ApprovalMode,
    /// Source control (ADR-130): fast-forward the project checkout's default branch from
    /// origin while it has no local changes.
    pub git_auto_pull: bool,
    /// Source control (ADR-130): new worktrees start from `origin/<default branch>` after a
    /// fetch instead of the local branch (on by default).
    pub worktrees_from_origin: bool,
    /// Agent behavior (ADR-130): how reply text appears while it streams.
    pub response_streaming: ResponseStreaming,
    /// Agent behavior (ADR-130): agents may use the in-app browser tools.
    pub agent_browser_access: bool,
    /// Custom persistent browser profiles (ADR-150); Default and Private are built in.
    pub browser_profiles: Vec<crate::browser_profiles::BrowserProfile>,
    /// The profile new browser tabs open in: `default`, `private` or a custom profile's id.
    #[serde(default = "crate::browser_profiles::default_profile_id")]
    pub default_browser_profile: String,
}

/// How a streaming reply shows its text (ADR-130).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResponseStreaming {
    /// Nothing until the turn ends.
    Turn,
    /// Each finished paragraph or code block.
    Paragraph,
    /// Word by word as it arrives (the behavior before the setting).
    #[default]
    Token,
}

fn default_locale() -> String {
    "pt-BR".into()
}

pub(crate) fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn default_true() -> bool {
    true
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            notifications: Default::default(),
            custom_shortcuts: std::collections::HashMap::new(),
            usage_providers: vec![AgentProviderId::Codex],
            sidebar_usage_providers: Vec::new(),
            steer_while_running: false,
            ci_auto_fix: false,
            pr_watch: true,
            dictation_enter_sends: false,
            fold_finished_turns: true,
            show_working_panel: true,
            composer_autocorrect: true,
            show_session_tabs: true,
            continue_interrupted_sessions: true,
            compact_idle_before_cache_expiry: false,
            rotate_at_context_limit: false,
            agent_sounds: false,
            keep_idle_agents: 16,
            browser_recording_fps: 30,
            browser_recording_keys: true,
            browser_recording_clicks: true,
            browser_show_agent_pages: true,
            github_links_in_app: true,
            link_open_target: LinkOpenTarget::App,
            diff_word_wrap: false,
            confirm_terminal_close: true,
            window_snap_enabled: false,
            window_snap_shortcut: WindowSnapShortcut::default(),
            github_pins: Vec::new(),
            github_seen: std::collections::BTreeMap::new(),
            github_seen_floor: None,
            github_notified: Vec::new(),
            rail_item_order: Vec::new(),
            hidden_rail_items: Vec::new(),
            rail_project_shortcuts: Vec::new(),
            default_agent: AgentProviderId::Codex,
            open_last_project: true,
            chat_background: None,
            chat_background_effect: "none".into(),
            chat_background_show_on: "all".into(),
            chat_background_empty_visibility: 24,
            chat_background_session_visibility: 24,
            worktree_base_path: None,
            default_session_workspace: SessionWorkspacePref::Ask,
            confirm_close_running: true,
            restore_previous_sessions: true,
            check_for_updates: false,
            disabled_providers: Vec::new(),
            provider_paths: std::collections::HashMap::new(),
            theme: ThemePref::Dark,
            dark_window_translucent: false,
            light_window_translucent: false,
            dark_window_opacity: 85,
            light_window_opacity: 85,
            dark_sidebar_translucent: false,
            light_sidebar_translucent: false,
            dark_sidebar_opacity: 72,
            light_sidebar_opacity: 38,
            translucent_opacity: 85,
            system_ui_font: true,
            ui_font: UiFont::Inter,
            code_font: MonoFont::SfMono,
            code_font_size: 13,
            terminal_font: MonoFont::SfMono,
            font_smoothing: true,
            dock_icon: DockIcon::Default,
            vscode_themes: Vec::new(),
            light_vscode_theme: None,
            dark_vscode_theme: None,
            density: DensityPref::Default,
            animations: true,
            composer_line_speed: ComposerLineSpeed::Slow,
            glass: false,
            pointer_glow: true,
            reduce_motion: false,
            ui_font_size: 13,
            git_auto_fetch: false,
            git_show_untracked: true,
            git_confirm_destructive: true,
            worktree_location: WorktreeLocationPref::Automatic,
            worktree_branch_pattern: "sirus/{session-name}".into(),
            terminal_use_system_shell: true,
            terminal_font_size: 13,
            terminal_cursor_style: "block".into(),
            terminal_scrollback: 2000,
            developer_logs: false,
            computer_use_enabled: false,
            agents_manage_sessions: false,
            experimental: false,
            disabled_models: Vec::new(),
            disabled_skills: Vec::new(),
            favorite_models: Vec::new(),
            default_model: None,
            model_execution: Default::default(),
            sidebar_collapsed: false,
            sidebar_mode: SidebarMode::Rail,
            sidebar_list_order: SidebarListOrder::Recent,
            sidebar_open_folders: None,
            sidebar_project_order: Vec::new(),
            sidebar_project_sort_order: SidebarProjectSortOrder::Manual,
            sidebar_thread_sort_order: SidebarThreadSortOrder::CreatedAt,
            sidebar_attention_first: false,
            pinned_project_ids: Vec::new(),
            project_folders: Vec::new(),
            pinned_session_ids: Vec::new(),
            connectors: Vec::new(),
            environment_panel_default_open: false,
            show_environment_usage: true,
            show_environment_repository: true,
            show_environment_editor: true,
            show_environment_pull_request: true,
            show_environment_pinned: true,
            show_environment_notepad: true,
            show_environment_instructions: true,
            enable_provider_update_checks: true,
            usage_auto_resume: false,
            app_update_auto_check: true,
            app_update_auto_download: true,
            app_update_whats_new: true,
            app_update_notes: None,
            locale: default_locale(),
            default_approval: ApprovalMode::Full,
            git_auto_pull: false,
            worktrees_from_origin: true,
            response_streaming: ResponseStreaming::Token,
            agent_browser_access: true,
            browser_profiles: Vec::new(),
            default_browser_profile: crate::browser_profiles::default_profile_id(),
        }
    }
}

/// Customizable rail items (Settings is fixed at the bottom).
/// The Kanban board was retired on 2026-10-11 (ADR-110); saved settings drop its id.
pub const RAIL_ITEMS: [&str; 2] = ["home", "pulls"];

/// Most unread markers kept for the Pull requests page; the renderer drops the oldest.
pub const GITHUB_SEEN_LIMIT: usize = 500;

/// Most pull request alerts remembered so a restart does not repeat them (ADR-050).
pub const GITHUB_NOTIFIED_LIMIT: usize = 200;

/// `opened:owner/repo#n` or `review:owner/repo#n`.
fn valid_github_notified(entry: &str) -> bool {
    entry
        .strip_prefix("opened:")
        .or_else(|| entry.strip_prefix("review:"))
        .is_some_and(valid_github_pin)
}

/// A strict RFC 3339 time as GitHub and `toISOString` write it.
fn valid_seen_time(value: &str) -> bool {
    value.len() <= 40
        && value.is_ascii()
        && value.as_bytes().get(10) == Some(&b'T')
        && chrono::DateTime::parse_from_rfc3339(value).is_ok()
}

/// `owner/repo#number`, with the same repository charset as GitHub web URLs.
pub(crate) fn valid_github_pin(pin: &str) -> bool {
    let Some((repo, number)) = pin.split_once('#') else {
        return false;
    };
    pin.len() <= 220
        && number
            .parse::<u32>()
            .is_ok_and(|n| n > 0 && n <= 10_000_000)
        && crate::pull_requests::repository(&format!("https://github.com/{repo}")).is_some()
}

impl AppSettings {
    pub fn validate_controls(&self) -> crate::error::Result<()> {
        crate::skills::validate_disabled(&self.disabled_skills)?;
        if self.app_update_notes.as_ref().is_some_and(|notes| {
            notes.version.is_empty()
                || notes.version.len() > 64
                || notes.notes.chars().count() > 4000
                || notes.date.as_ref().is_some_and(|date| date.len() > 64)
        }) {
            return Err(crate::error::Error::new(
                "settings",
                "Update notes are not valid.",
            ));
        }
        crate::connectors::validate(&self.connectors)?;
        crate::browser_profiles::validate(&self.browser_profiles, &self.default_browser_profile)?;
        use crate::error::Error;
        // One entry per provider at most (ten since Hermes).
        if self.usage_providers.len() > 10
            || self
                .usage_providers
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.usage_providers.len()
        {
            return Err(Error::new(
                "invalid_settings",
                "Invalid usage provider selection.",
            ));
        }
        if ![30, 60].contains(&self.browser_recording_fps) {
            return Err(Error::new(
                "invalid_settings",
                "Recordings run at 30 or 60 frames per second.",
            ));
        }
        if ![0, 1, 2, 3, 5, 16].contains(&self.keep_idle_agents) {
            return Err(Error::new(
                "invalid_settings",
                "Invalid number of agents to keep ready.",
            ));
        }
        if self
            .chat_background
            .as_deref()
            .is_some_and(|name| !crate::chat_background::valid_name(name))
            || !crate::chat_background::EFFECTS.contains(&self.chat_background_effect.as_str())
            || !["empty", "all"].contains(&self.chat_background_show_on.as_str())
            || self.chat_background_empty_visibility > 100
            || self.chat_background_session_visibility > 100
        {
            return Err(Error::new("invalid_settings", "Invalid chat background."));
        }
        crate::vscode_theme::validate_stored(
            &self.vscode_themes,
            self.light_vscode_theme.as_deref(),
            self.dark_vscode_theme.as_deref(),
        )?;
        if self.sidebar_usage_providers.len() > 2
            || self
                .sidebar_usage_providers
                .iter()
                .any(|id| !crate::provider_usage::reports_usage(id))
            || (self.sidebar_usage_providers.len() == 2
                && self.sidebar_usage_providers[0] == self.sidebar_usage_providers[1])
        {
            return Err(Error::new(
                "invalid_settings",
                "Choose at most two providers with usage for the sidebar.",
            ));
        }
        if self.github_pins.len() > 200 || self.github_pins.iter().any(|pin| !valid_github_pin(pin))
        {
            return Err(Error::new(
                "invalid_settings",
                "Invalid pinned pull requests.",
            ));
        }
        if self.github_seen.len() > GITHUB_SEEN_LIMIT
            || self
                .github_seen
                .iter()
                .any(|(key, at)| !valid_github_pin(key) || !valid_seen_time(at))
            || self
                .github_seen_floor
                .as_deref()
                .is_some_and(|at| !valid_seen_time(at))
        {
            return Err(Error::new(
                "invalid_settings",
                "Invalid read pull requests.",
            ));
        }
        if self.github_notified.len() > GITHUB_NOTIFIED_LIMIT
            || self
                .github_notified
                .iter()
                .any(|entry| !valid_github_notified(entry))
            || self
                .github_notified
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.github_notified.len()
        {
            return Err(Error::new(
                "invalid_settings",
                "Invalid pull request alerts.",
            ));
        }
        let unique = |items: &[String]| {
            items.iter().collect::<std::collections::HashSet<_>>().len() == items.len()
        };
        if self.rail_item_order.len() > RAIL_ITEMS.len()
            || self.hidden_rail_items.len() > RAIL_ITEMS.len()
            || !unique(&self.rail_item_order)
            || !unique(&self.hidden_rail_items)
            || self
                .rail_item_order
                .iter()
                .chain(&self.hidden_rail_items)
                .any(|item| !RAIL_ITEMS.contains(&item.as_str()))
            || self.hidden_rail_items.iter().any(|item| item == "home")
            || self.rail_project_shortcuts.len() > 12
            || !unique(&self.rail_project_shortcuts)
            || self
                .rail_project_shortcuts
                .iter()
                .any(|id| id.is_empty() || id.len() > 128 || id.chars().any(char::is_control))
        {
            return Err(Error::new(
                "invalid_settings",
                "Invalid rail customization.",
            ));
        }
        crate::appearance::validate(self)?;
        if self.model_execution.len() > 512
            || self.model_execution.iter().any(|(key, option)| {
                key.len() > 256
                    || key.chars().any(char::is_control)
                    || !key.contains("::")
                    || option.approval.is_some()
                    || option
                        .effort
                        .as_deref()
                        .is_some_and(|effort| !crate::execution::EFFORTS.contains(&effort))
            })
        {
            return Err(Error::new(
                "invalid_settings",
                "Invalid model execution preferences.",
            ));
        }
        // Provider executable overrides: an absolute path (or a bare command name kept from
        // older data), never relative segments or control characters.
        if self.provider_paths.values().any(|path| {
            path.trim().is_empty()
                || path.len() > 4096
                || path.chars().any(char::is_control)
                || !(std::path::Path::new(path).is_absolute() || !path.contains('/'))
        }) {
            return Err(Error::new(
                "invalid_settings",
                "Invalid provider executable path.",
            ));
        }
        const BINDINGS: [(&str, &str); 19] = [
            ("new-session", "meta+n"),
            ("palette", "meta+k"),
            ("open-project", "meta+o"),
            ("settings", "meta+,"),
            ("toggle-sidebar", "meta+s"),
            ("toggle-context", "meta+\\"),
            ("open-terminal", "meta+`"),
            ("stop-agent", "meta+."),
            ("back", "meta+["),
            ("forward", "meta+]"),
            ("open-files", "meta+alt+o"),
            ("open-browser", "meta+alt+b"),
            ("toggle-environment", "meta+alt+e"),
            ("find-in-conversation", "meta+f"),
            ("search-conversations", "meta+shift+f"),
            ("toggle-side-chat", "meta+alt+s"),
            ("send-new-thread", "meta+alt+enter"),
            ("cycle-effort", "shift+tab"),
            ("reopen-closed", "meta+shift+t"),
        ];
        for (id, combo) in &self.custom_shortcuts {
            let mut parts = combo.split('+').collect::<Vec<_>>();
            let key = parts.pop().unwrap_or_default();
            let canonical = ["meta", "alt", "shift"]
                .into_iter()
                .filter(|part| parts.contains(part))
                .chain([key])
                .collect::<Vec<_>>()
                .join("+");
            // The composer's effort cycle may use Tab with any modifier and no ⌘.
            let tab = id == "cycle-effort" && key == "tab" && !parts.is_empty();
            if !BINDINGS.iter().any(|(known, _)| known == id)
                || combo.len() > 40
                || (!tab && !parts.contains(&"meta"))
                || canonical != *combo
                || parts
                    .iter()
                    .any(|part| !["meta", "alt", "shift"].contains(part))
                || (key == "enter" && parts.len() == 1)
                || (key != "enter"
                    && !tab
                    && (key.len() != 1
                        || !key.chars().all(|c| {
                            c.is_ascii_lowercase() || c.is_ascii_digit() || ",.;/\\[]`'".contains(c)
                        })))
                || ([
                    "q", "w", "h", "m", "c", "v", "x", "a", "z", "r", "l", "t", "=", "-",
                ]
                .contains(&key)
                    // ⌘⇧T reopens what was closed, like a browser (ADR-102).
                    && canonical != "meta+shift+t")
            {
                return Err(Error::new(
                    "invalid_settings",
                    "Unsupported shortcut combination.",
                ));
            }
        }
        let mut used = std::collections::HashSet::new();
        for (id, fallback) in BINDINGS {
            if !used.insert(
                self.custom_shortcuts
                    .get(id)
                    .map(String::as_str)
                    .unwrap_or(fallback),
            ) {
                return Err(Error::new(
                    "invalid_settings",
                    "Shortcut combinations must be unique.",
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;
    #[test]
    fn github_pins_are_bounded_owner_repo_numbers() {
        for good in ["owner/repo#1", "my-org/a.b_c#9999"] {
            assert!(valid_github_pin(good), "{good}");
        }
        for bad in [
            "owner/repo",
            "owner/repo#0",
            "owner#1",
            "o/../x#1",
            "o/r#1#2",
            "o/r#-1",
            "https://x#1",
        ] {
            assert!(!valid_github_pin(bad), "{bad}");
        }
        let settings = AppSettings {
            github_pins: vec!["owner/repo#1".into(); 201],
            ..AppSettings::default()
        };
        assert!(settings.validate_controls().is_err());
    }

    #[test]
    fn github_seen_is_bounded_with_valid_keys_and_times() {
        let with = |entries: &[(&str, &str)], floor: Option<&str>| AppSettings {
            github_seen: entries
                .iter()
                .map(|(key, at)| (key.to_string(), at.to_string()))
                .collect(),
            github_seen_floor: floor.map(str::to_string),
            ..AppSettings::default()
        };
        let parsed = serde_json::from_str::<AppSettings>("{}").unwrap();
        assert!(parsed.github_seen.is_empty() && parsed.github_seen_floor.is_none());
        assert!(with(
            &[
                ("owner/repo#1", "2026-10-17T12:00:00Z"),
                ("my-org/a.b_c#9", "2026-10-17T12:00:00.123+02:00"),
            ],
            Some("2026-10-01T00:00:00.000Z"),
        )
        .validate_controls()
        .is_ok());
        for (key, at) in [
            ("owner/repo", "2026-10-17T12:00:00Z"),
            ("o/../x#1", "2026-10-17T12:00:00Z"),
            ("owner/repo#1", "yesterday"),
            ("owner/repo#1", "2026-10-17 12:00:00Z"),
            ("owner/repo#1", ""),
        ] {
            assert!(
                with(&[(key, at)], None).validate_controls().is_err(),
                "{key} {at}"
            );
        }
        assert!(with(&[], Some("2026-10-01")).validate_controls().is_err());
        let keys: Vec<String> = (1..=GITHUB_SEEN_LIMIT + 1)
            .map(|n| format!("owner/repo#{n}"))
            .collect();
        let entries: Vec<(&str, &str)> = keys
            .iter()
            .map(|key| (key.as_str(), "2026-10-17T12:00:00Z"))
            .collect();
        assert!(with(&entries, None).validate_controls().is_err());
        assert!(with(&entries[1..], None).validate_controls().is_ok());
    }

    #[test]
    fn github_notified_is_bounded_unique_and_closed() {
        let with = |entries: Vec<String>| AppSettings {
            github_notified: entries,
            ..AppSettings::default()
        };
        assert!(serde_json::from_str::<AppSettings>("{}")
            .unwrap()
            .github_notified
            .is_empty());
        assert!(with(vec![
            "opened:owner/repo#1".into(),
            "review:owner/repo#1".into()
        ])
        .validate_controls()
        .is_ok());
        for bad in [
            "owner/repo#1",
            "merged:owner/repo#1",
            "opened:owner/repo",
            "review:o/../x#1",
            "opened:",
        ] {
            assert!(with(vec![bad.into()]).validate_controls().is_err(), "{bad}");
        }
        assert!(with(vec!["opened:owner/repo#1".into(); 2])
            .validate_controls()
            .is_err());
        let many: Vec<String> = (1..=GITHUB_NOTIFIED_LIMIT + 1)
            .map(|n| format!("opened:owner/repo#{n}"))
            .collect();
        assert!(with(many.clone()).validate_controls().is_err());
        assert!(with(many[1..].to_vec()).validate_controls().is_ok());
    }

    #[test]
    fn sidebar_usage_accepts_at_most_two_providers_that_report_usage() {
        let with = |ids: Vec<AgentProviderId>| AppSettings {
            sidebar_usage_providers: ids,
            ..AppSettings::default()
        };
        assert!(serde_json::from_str::<AppSettings>("{}")
            .unwrap()
            .sidebar_usage_providers
            .is_empty());
        assert!(with(vec![AgentProviderId::Codex, AgentProviderId::Claude])
            .validate_controls()
            .is_ok());
        assert!(with(vec![
            AgentProviderId::Codex,
            AgentProviderId::Claude,
            AgentProviderId::Cursor
        ])
        .validate_controls()
        .is_err());
        assert!(with(vec![AgentProviderId::Codex, AgentProviderId::Codex])
            .validate_controls()
            .is_err());
        assert!(with(vec![AgentProviderId::Grok])
            .validate_controls()
            .is_err());
    }

    #[test]
    fn general_preferences_have_legacy_defaults_and_closed_sort_modes() {
        let legacy: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(
            legacy.sidebar_project_sort_order,
            SidebarProjectSortOrder::Manual
        );
        let retired: AppSettings =
            serde_json::from_str(r#"{"sidebarProjectSortOrder":"updated_at"}"#).unwrap();
        assert_eq!(
            retired.sidebar_project_sort_order,
            SidebarProjectSortOrder::Manual
        );
        assert_eq!(
            legacy.sidebar_thread_sort_order,
            SidebarThreadSortOrder::CreatedAt
        );
        assert!(!legacy.sidebar_attention_first);
        let attention: AppSettings =
            serde_json::from_str(r#"{"sidebarAttentionFirst":true}"#).unwrap();
        assert!(attention.sidebar_attention_first);
        assert_eq!(
            serde_json::to_value(&attention).unwrap()["sidebarAttentionFirst"],
            true
        );
        assert!(
            legacy.show_environment_usage
                && legacy.show_environment_repository
                && legacy.show_environment_editor
                && legacy.show_environment_pull_request
                && legacy.show_environment_pinned
                && legacy.show_environment_notepad
                && legacy.show_environment_instructions
        );
        for (field, allowed) in [
            ("sidebarProjectSortOrder", vec!["manual", "created_at"]),
            ("sidebarThreadSortOrder", vec!["updated_at", "created_at"]),
        ] {
            for mode in allowed {
                let settings: AppSettings =
                    serde_json::from_value(serde_json::json!({field: mode})).unwrap();
                assert_eq!(serde_json::to_value(settings).unwrap()[field], mode);
            }
            for invalid in [
                serde_json::json!("random"),
                serde_json::json!(100),
                serde_json::Value::Null,
            ] {
                assert!(
                    serde_json::from_value::<AppSettings>(serde_json::json!({field: invalid}))
                        .is_err()
                );
            }
        }
        assert!(serde_json::from_value::<AppSettings>(
            serde_json::json!({"sidebarThreadSortOrder": "manual"})
        )
        .is_err());
        for field in [
            "showEnvironmentUsage",
            "showEnvironmentRepository",
            "showEnvironmentEditor",
            "showEnvironmentPullRequest",
            "showEnvironmentPinned",
            "showEnvironmentNotepad",
            "showEnvironmentInstructions",
        ] {
            let settings: AppSettings =
                serde_json::from_value(serde_json::json!({field: false})).unwrap();
            assert_eq!(serde_json::to_value(settings).unwrap()[field], false);
            assert!(
                serde_json::from_value::<AppSettings>(serde_json::json!({field: "false"})).is_err()
            );
        }
    }
    #[test]
    fn composer_line_speed_is_backward_compatible_and_closed() {
        let legacy: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.composer_line_speed, ComposerLineSpeed::Slow);
        for speed in ["slow", "smooth", "fast"] {
            let settings: AppSettings = serde_json::from_value(serde_json::json!({
                "composerLineSpeed": speed
            }))
            .unwrap();
            let saved = serde_json::to_value(settings).unwrap();
            assert_eq!(saved["composerLineSpeed"], speed);
        }
        for invalid in [
            serde_json::json!("turbo"),
            serde_json::json!(100),
            serde_json::Value::Null,
        ] {
            assert!(serde_json::from_value::<AppSettings>(serde_json::json!({
                "composerLineSpeed": invalid
            }))
            .is_err());
        }
    }
    #[test]
    fn sidebar_mode_and_list_order_are_backward_compatible_and_closed() {
        let legacy: AppSettings = serde_json::from_str(r#"{"sidebarCollapsed":true}"#).unwrap();
        assert!(legacy.sidebar_collapsed);
        assert_eq!(legacy.sidebar_mode, SidebarMode::Rail);
        assert_eq!(legacy.sidebar_list_order, SidebarListOrder::Recent);
        assert_eq!(legacy.link_open_target, LinkOpenTarget::App);
        assert!(legacy.sidebar_open_folders.is_none());
        // The removed Project style loads as the list.
        let project: AppSettings = serde_json::from_str(r#"{"sidebarMode":"project"}"#).unwrap();
        assert_eq!(project.sidebar_mode, SidebarMode::List);
        for (field, allowed) in [
            ("sidebarMode", &["rail", "list"][..]),
            ("sidebarListOrder", &["recent", "project"][..]),
            ("linkOpenTarget", &["app", "system"][..]),
        ] {
            for &value in allowed {
                let settings: AppSettings =
                    serde_json::from_value(serde_json::json!({ field: value })).unwrap();
                assert_eq!(serde_json::to_value(settings).unwrap()[field], value);
            }
            for invalid in [
                serde_json::json!("hidden"),
                serde_json::json!(1),
                serde_json::Value::Null,
            ] {
                assert!(serde_json::from_value::<AppSettings>(
                    serde_json::json!({ field: invalid })
                )
                .is_err());
            }
        }
    }
    #[test]
    fn mac_settings_from_the_phone_load_old_files_and_stay_bounded() {
        let legacy: AppSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(legacy.default_approval, ApprovalMode::Full);
        assert!(!legacy.git_auto_pull && legacy.worktrees_from_origin);
        assert_eq!(legacy.response_streaming, ResponseStreaming::Token);
        assert!(legacy.agent_browser_access);
        let saved: AppSettings = serde_json::from_value(serde_json::json!({
            "defaultApproval": "auto",
            "responseStreaming": "paragraph",
        }))
        .unwrap();
        let value = serde_json::to_value(&saved).unwrap();
        assert_eq!(value["defaultApproval"], "auto");
        assert_eq!(value["responseStreaming"], "paragraph");
        assert!(saved.validate_controls().is_ok());
        assert!(serde_json::from_value::<AppSettings>(
            serde_json::json!({ "responseStreaming": "fast" })
        )
        .is_err());
    }
    #[test]
    fn settings_cannot_store_global_approval_grants() {
        let mut settings = AppSettings::default();
        settings
            .model_execution
            .insert("codex::model".into(), ExecutionOptions::default());
        assert!(settings.validate_controls().is_ok());
        for mode in [ApprovalMode::Ask, ApprovalMode::Auto, ApprovalMode::Full] {
            settings
                .model_execution
                .get_mut("codex::model")
                .unwrap()
                .approval = Some(mode);
            assert!(settings.validate_controls().is_err());
        }
    }
    #[test]
    fn shortcut_controls_reject_collisions_reserved_keys_and_unknown_actions() {
        let mut settings = AppSettings::default();
        for (id, combo) in [
            ("toggle-sidebar", "meta+n"),
            ("palette", "meta+q"),
            ("palette", "meta+meta+b"),
            ("unknown", "meta+shift+b"),
            ("send-new-thread", "meta+enter"),
            ("send-new-thread", "meta+alt+return"),
        ] {
            settings.custom_shortcuts.clear();
            settings.custom_shortcuts.insert(id.into(), combo.into());
            assert!(settings.validate_controls().is_err());
        }
        settings.custom_shortcuts.clear();
        settings
            .custom_shortcuts
            .insert("toggle-sidebar".into(), "meta+shift+b".into());
        assert!(settings.validate_controls().is_ok());
        for (id, combo) in [
            ("forward", "meta+shift+]"),
            ("open-files", "meta+shift+o"),
            ("open-browser", "meta+shift+b"),
            ("toggle-environment", "meta+shift+e"),
            ("find-in-conversation", "meta+alt+f"),
            ("search-conversations", "meta+alt+g"),
            ("toggle-side-chat", "meta+shift+s"),
            ("send-new-thread", "meta+shift+enter"),
            ("reopen-closed", "meta+alt+y"),
        ] {
            settings.custom_shortcuts.clear();
            settings.custom_shortcuts.insert(id.into(), combo.into());
            assert!(settings.validate_controls().is_ok(), "{id}");
        }
        settings.custom_shortcuts.clear();
        settings
            .custom_shortcuts
            .insert("reopen-closed".into(), "meta+alt+t".into());
        assert!(
            settings.validate_controls().is_err(),
            "only meta+shift+t may use the reserved T key"
        );
        settings.custom_shortcuts.clear();
        settings
            .custom_shortcuts
            .insert("palette".into(), "meta+shift+t".into());
        assert!(
            settings.validate_controls().is_err(),
            "the reopen default stays unique"
        );
        settings.ui_font_size = 100;
        assert!(settings.validate_controls().is_err());
    }
    #[test]
    fn provider_paths_must_be_absolute_or_bare_names() {
        let mut settings = AppSettings::default();
        for (path, ok) in [
            ("/opt/homebrew/bin/claude", true),
            ("claude", true),
            ("bin/claude", false),
            ("../claude", false),
            ("", false),
            ("/bin/claude\n", false),
        ] {
            settings.provider_paths.clear();
            settings
                .provider_paths
                .insert(AgentProviderId::Claude, path.into());
            assert_eq!(settings.validate_controls().is_ok(), ok, "{path:?}");
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AppData {
    pub projects: Vec<Project>,
    pub sessions: Vec<Session>,
    pub settings: AppSettings,
    #[serde(default)]
    pub provider_accounts: Vec<ProviderAccount>,
    #[serde(default)]
    pub selected_provider_accounts: std::collections::HashMap<AgentProviderId, String>,
    #[serde(default)]
    pub composer_drafts: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub context_texts: std::collections::HashMap<String, String>,
    /// Per-session CI auto-fix progress and explicit per-PR opt-outs (ADR-064).
    #[serde(default)]
    pub ci_auto_fix: Vec<crate::ci_autofix::FixState>,
    /// Per-session pull request watches and what their agents were told (ADR-079).
    #[serde(default)]
    pub pr_watches: Vec<crate::pr_watch::PrWatch>,
    /// Guided reviews of pull requests and session changes (ADR-125); one row each in `state.db`.
    #[serde(default)]
    pub review_guides: Vec<crate::review_guides::ReviewGuide>,
}

impl AppData {
    /// Whether a provider may start work: off in Settings → Providers keeps it out everywhere.
    pub fn provider_enabled(&self, provider: &AgentProviderId) -> bool {
        !self.settings.disabled_providers.contains(provider)
    }
}

pub fn default_account_id() -> String {
    "default".into()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAccount {
    pub id: String,
    pub provider: AgentProviderId,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitIdentity {
    pub is_repo: bool,
    pub root: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChange {
    pub path: String,
    pub kind: ChangeKind,
    pub additions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitWorktree {
    pub path: String,
    pub head: Option<String>,
    pub branch: Option<String>,
    pub detached: bool,
    pub bare: bool,
    pub locked: Option<String>,
    pub prunable: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitStatus {
    pub identity: GitIdentity,
    pub dirty: bool,
    pub ahead: u32,
    pub behind: u32,
    pub changes: Vec<FileChange>,
    /// More untracked files exist than `changes` lists (capped at `git::MAX_UNTRACKED`).
    #[serde(default)]
    pub untracked_truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub path: String,
    pub is_dir: bool,
}

/// One matching line from the Files pane text search (ADR-095). `line` and
/// `column` are 1-based (column in characters); `text` is a trimmed excerpt.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TextSearchMatch {
    pub line: u32,
    pub column: u32,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSearchFile {
    /// Relative to the workspace root, `/`-separated.
    pub path: String,
    pub matches: Vec<TextSearchMatch>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextSearchResult {
    pub files: Vec<TextSearchFile>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum WorkspaceEntryKind {
    File,
    Directory,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentInstall {
    pub id: AgentProviderId,
    pub name: String,
    pub binary: String,
    pub installed: bool,
    pub path: Option<String>,
    pub version: Option<String>,
    /// Every install found on this machine, for Settings → Providers to choose from.
    #[serde(default)]
    pub candidates: Vec<ExecutableCandidate>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExecutableCandidate {
    pub path: String,
    pub version: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppearanceSupport {
    pub translucency: bool,
    pub dock_icon: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    pub appearance_support: AppearanceSupport,
    pub git_detected: bool,
    pub git_path: Option<String>,
    pub git_version: Option<String>,
    pub shell: String,
    pub data_dir: String,
    pub worktree_root: String,
    #[serde(default)]
    pub capabilities: HostCapabilities,
    /// The machine's name (`gethostname`, without `.local`), shown on the phone instead of
    /// "Mac" when the host is a server (ADR-131).
    #[serde(default)]
    pub host_name: String,
}

/// What this host can do (ADR-131): the headless Linux host has no window, so the
/// embedded browser, computer use, dictation and native pickers are the Mac app's.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HostCapabilities {
    /// `macos`, `linux`, … (`std::env::consts::OS`).
    pub platform: String,
    /// No window: the headless `sirus-host`.
    pub headless: bool,
    pub browser: bool,
    pub computer_use: bool,
    pub dictation: bool,
    /// Native file and folder pickers.
    pub pickers: bool,
}

impl HostCapabilities {
    pub fn current() -> Self {
        Self {
            platform: std::env::consts::OS.into(),
            headless: true,
            browser: false,
            computer_use: false,
            dictation: false,
            pickers: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub ok: bool,
    pub version: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    pub session_id: String,
    pub message_id: String,
    pub offset: usize,
    pub message: Option<Message>,
    pub stream: String,
    pub chunk: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentExitEvent {
    pub session_id: String,
    pub code: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PtyOutputEvent {
    pub session_id: String,
    pub terminal_id: String,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitCommitResult {
    pub hash: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitPushResult {
    pub branch: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorInstall {
    pub id: String,
    pub name: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ImportOrigin {
    pub provider: AgentProviderId,
    pub conversation_id: String,
}

/// One side of a conversation moved between computers (ADR-133): the other computer's name,
/// the branch that carried the work, and when.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SessionMoveNote {
    pub computer: String,
    pub branch: String,
    pub at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    pub name: String,
    pub current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUpdate {
    pub provider: AgentProviderId,
    pub installed: bool,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub update_available: bool,
    /// False when this installation has no fixed one-click update command.
    pub update_supported: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUpdateResult {
    pub provider: AgentProviderId,
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorAppIcon {
    pub id: String,
    /// Base64 PNG of the real macOS app icon; empty when unavailable.
    pub png: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFileSnapshot {
    pub path: String,
    pub content: String,
    pub size: u64,
    pub binary: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateSessionRequest {
    pub project_id: String,
    pub title: Option<String>,
    pub agent: AgentProviderId,
    pub isolated_worktree: bool,
    #[serde(default)]
    pub model: Option<String>,
    /// Work in this session's folder (same project) instead of the checkout or a new worktree.
    #[serde(default)]
    pub share_worktree_of: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApprovalMode {
    Ask,
    Auto,
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(default, deny_unknown_fields)]
#[derive(Default)]
pub struct ExecutionOptions {
    pub effort: Option<String>,
    pub fast: bool,
    pub planning: bool,
    pub approval: Option<ApprovalMode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendPromptRequest {
    #[serde(default)]
    pub queued_after: Option<QueuedPromptContext>,
    #[serde(default)]
    pub debugging: bool,
    #[serde(default)]
    pub goal: Option<String>,
    pub session_id: String,
    #[serde(default)]
    pub attachment_ids: Vec<String>,
    #[serde(default)]
    pub attachment_owner: String,
    pub prompt: String,
    #[serde(default)]
    pub execution: ExecutionOptions,
    /// Team planning turn: read-only, wrapped with coordinator instructions (ADR-043).
    #[serde(default)]
    pub team: bool,
    /// One UUID per user send, reused on every retry of it: a send whose id is already
    /// on a recent user message returns the session as it is instead of a second turn.
    #[serde(default)]
    pub client_request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct QueuedPromptContext {
    pub message_id: Option<String>,
    pub agent: AgentProviderId,
    pub model: Option<String>,
    pub provider_account_id: String,
    pub worktree_path: String,
}

/// Devices pane (ADR-128): what the pane needs to know before it can stream a simulator.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    /// macOS with Xcode's `simctl`.
    pub xcode: bool,
    /// The Node.js version found (`v24.1.0`), if any.
    pub node_version: Option<String>,
    /// Node.js 22 or newer, with npm beside it.
    pub node_ok: bool,
    pub tools: DeviceTools,
    pub hub: DeviceHubState,
    pub devices: Vec<SimDevice>,
    /// The loopback proxy the desktop webview streams through, once it runs.
    pub proxy: Option<DeviceProxy>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceTools {
    pub hub_package: String,
    pub hub_version: String,
    pub installed: bool,
    pub installing: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceHubState {
    pub running: bool,
    pub starting: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SimDevice {
    pub udid: String,
    pub name: String,
    /// `iOS 18.2`.
    pub runtime: String,
    pub booted: bool,
    pub family: SimFamily,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SimFamily {
    Phone,
    Tablet,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceProxy {
    pub port: u16,
    pub token: String,
}

/// Every finished conversation keeps its agent (`agent_pool::POOL_LIMIT`).
fn default_recording_fps() -> u32 {
    30
}

fn default_keep_idle_agents() -> u32 {
    16
}

fn default_show_working_panel() -> bool {
    true
}
