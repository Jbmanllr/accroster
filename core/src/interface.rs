//! The core's interface: what goes in ([`Command`], [`Event`]) and what comes
//! out ([`View`], [`Effect`]).
//!
//! The UI layer never decides anything. It:
//!
//! 1. renders the latest [`View`] (menu bar label, menu, Settings, About),
//! 2. forwards each click as the [`Command`] attached to what was clicked, and
//!    each OS notification as an [`Event`],
//! 3. performs each [`Effect`] it receives, literally.
//!
//! Commands are what the user asked for; events are what happened. Both are
//! handled the same way: the core updates its state and files, then returns
//! the effects to perform. The view is recomputed from state on demand, so it
//! is always complete and the UI can redraw everything from it.
//!
//! Later tickets add variants (switch CLI, add Profile, sleep/wake, …) here;
//! everything else in the UI layer stays a renderer.

use std::path::PathBuf;

/// A user intent, sent by the UI layer when something is clicked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Settings… in the menu.
    OpenSettings,
    /// About in the menu.
    OpenAbout,
    /// Quit in the menu.
    Quit,
    /// The Verbose logging switch in Settings. Turning it on lasts 24 hours.
    SetVerboseLogging(bool),
    /// Reveal logs in Finder, in Settings.
    RevealLogs,
    /// Add Profile… in the menu: create a Profile folder and start Claude
    /// Code's browser sign-in in it.
    AddProfile,
    /// "Waiting for sign-in… Cancel" in the menu.
    CancelSignIn,
    /// The launch-at-login checkbox, in the first-run offer or in Settings.
    SetLaunchAtLogin(bool),
    /// The CLI switching switch in Settings. On appends the marked block to
    /// `~/.zshrc` and installs the `claude` shim; off removes them and the
    /// pointer.
    SetCliSwitching(bool),
    /// A Profile row's click, or "Use for new CLI runs" in its submenu: make
    /// the Profile (by id) the CLI's Active Profile for new `claude` runs.
    UseForNewCliRuns(String),
    /// "Copy shell override" in a Profile's submenu (by id).
    CopyShellOverride(String),
    /// "Switch desktop app to this Profile…" in a Profile's submenu, with
    /// that Profile's id: the Guided switch.
    SwitchDesktopApp(String),
    /// "Switch anyway", when the busy guard couldn't tell.
    ConfirmDesktopSwitch,
    /// "Notify me when idle", when a desktop turn blocked the switch.
    NotifyWhenDesktopIdle,
    /// "Cancel switch".
    CancelDesktopSwitch,
    /// "Limit resets → claude.ai" in a Profile's hover submenu: open
    /// claude.ai Settings > Usage, where the user checks resets by hand.
    OpenLimitResets,
    /// A Discovered Profile's row: Add Profile… aimed at that pair (passing
    /// its email to the sign-in when known).
    SignInDiscovered(DiscoveredPair),
    /// Hide in a Discovered Profile's submenu.
    HideDiscovered(DiscoveredPair),
    /// Restore in Settings > Hidden (n)….
    UnhideDiscovered(DiscoveredPair),
    /// Rename… in a submenu: asks the UI for a nickname
    /// ([`Effect::AskNickname`]).
    Rename(NicknameTarget),
    /// The answer to [`Effect::AskNickname`]. `None`, empty or blank clears
    /// the nickname.
    SetNickname {
        target: NicknameTarget,
        nickname: Option<String>,
    },
    /// All Sessions… in the menu: show the All Sessions window and bring its
    /// list up to date.
    OpenAllSessions,
    /// The All Sessions search field changed. Empty shows every Session.
    SearchSessions(String),
    /// An item in the All Sessions sidebar was picked.
    FilterSessions(SessionFilter),
    /// A row in the All Sessions list was selected (by id), or the
    /// selection was cleared. The detail pane shows the selected Session.
    SelectSession(Option<String>),
    /// The All Sessions window closed: stop watching Claude's folders.
    CloseAllSessions,
    /// Moves a Profile (by id) to position `to` in the Profile order.
    MoveProfile { id: String, to: usize },
    /// "Sign in again…" on an expired or signed-out Profile (by id): Claude
    /// Code's browser sign-in in that same folder.
    SignInAgain(String),
    /// Remove… in a row's hover submenu: asks first
    /// ([`Effect::ConfirmRemoval`]).
    AskRemoveProfile(String),
    /// The removal confirmed: sign the Profile out, then delete its folder.
    RemoveProfile(String),
    /// A choice under the CLI's Active Profile's "Remove…": which Profile
    /// new CLI runs use next (`None` = Default, `~/.claude`). Asks first.
    AskRemoveProfileWithNextCli { id: String, next: Option<String> },
    /// That removal confirmed: point the CLI at `next` (or delete the
    /// pointer) first, then remove the Profile.
    RemoveProfileWithNextCli { id: String, next: Option<String> },
    /// "Mute notifications" in a Profile's hover submenu (by id): mutes
    /// (`true`) or unmutes its limit notifications.
    SetMuted { id: String, muted: bool },
    /// The thresholds in Settings, in whole percent (1–100). Out-of-range
    /// values are dropped; the rest are kept sorted, without repeats.
    SetThresholds(Vec<u32>),
    /// The reset notification switch in Settings.
    SetResetNotification(bool),
    /// Diagnostics… in the menu.
    OpenDiagnostics,
    /// "Copy report" in Diagnostics: field names and versions only.
    CopyDiagnosticsReport,
    /// "Resume <feature>" in Diagnostics, offered once the checks pass again.
    /// The only way a Safety pause ends.
    ResumeFeature(SafetyFeature),
}

/// A feature that writes Claude's files and can get a Safety pause
/// (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SafetyFeature {
    /// The pointer and row clicks.
    CliSwitching,
    /// The Profile links that share Transcripts and resume data
    /// (`projects`, `file-history`, `session-env`).
    TranscriptSharing,
}

/// A Discovered Profile's identity: an account uuid and an Organization uuid.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DiscoveredPair {
    pub account_uuid: String,
    pub organization_uuid: String,
}

/// What a nickname is given to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NicknameTarget {
    /// A Profile, by id.
    Profile(String),
    Discovered(DiscoveredPair),
}

impl Command {
    /// A fixed name for logs (never carries user data).
    pub fn name(&self) -> &'static str {
        match self {
            Command::OpenSettings => "open_settings",
            Command::OpenAbout => "open_about",
            Command::Quit => "quit",
            Command::SetVerboseLogging(_) => "set_verbose_logging",
            Command::RevealLogs => "reveal_logs",
            Command::AddProfile => "add_profile",
            Command::CancelSignIn => "cancel_sign_in",
            Command::SetLaunchAtLogin(_) => "set_launch_at_login",
            Command::SetCliSwitching(_) => "set_cli_switching",
            Command::UseForNewCliRuns(_) => "use_for_new_cli_runs",
            Command::CopyShellOverride(_) => "copy_shell_override",
            Command::SwitchDesktopApp(_) => "switch_desktop_app",
            Command::ConfirmDesktopSwitch => "confirm_desktop_switch",
            Command::NotifyWhenDesktopIdle => "notify_when_desktop_idle",
            Command::CancelDesktopSwitch => "cancel_desktop_switch",
            Command::OpenLimitResets => "open_limit_resets",
            Command::SignInDiscovered(_) => "sign_in_discovered",
            Command::HideDiscovered(_) => "hide_discovered",
            Command::UnhideDiscovered(_) => "unhide_discovered",
            Command::Rename(_) => "rename",
            Command::SetNickname { .. } => "set_nickname",
            Command::OpenDiagnostics => "open_diagnostics",
            Command::CopyDiagnosticsReport => "copy_diagnostics_report",
            Command::ResumeFeature(_) => "resume_feature",
            Command::OpenAllSessions => "open_all_sessions",
            Command::SearchSessions(_) => "search_sessions",
            Command::FilterSessions(_) => "filter_sessions",
            Command::SelectSession(_) => "select_session",
            Command::CloseAllSessions => "close_all_sessions",
            Command::MoveProfile { .. } => "move_profile",
            Command::SignInAgain(_) => "sign_in_again",
            Command::AskRemoveProfile(_) => "ask_remove_profile",
            Command::RemoveProfile(_) => "remove_profile",
            Command::AskRemoveProfileWithNextCli { .. } => "ask_remove_profile_with_next_cli",
            Command::RemoveProfileWithNextCli { .. } => "remove_profile_with_next_cli",
            Command::SetMuted { .. } => "set_muted",
            Command::SetThresholds(_) => "set_thresholds",
            Command::SetResetNotification(_) => "set_reset_notification",
        }
    }
}

/// Something that happened outside the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Time passed. The driver sends one every [`crate::driver::TICK_INTERVAL`];
    /// the core reads the actual time from its [`crate::Clock`].
    Tick,
    /// The status item's menu is about to open.
    MenuOpened,
    /// The Mac is going to sleep.
    WillSleep,
    /// The Mac woke from sleep.
    DidWake,
    /// The screen locked.
    ScreenLocked,
    /// The screen unlocked.
    ScreenUnlocked,
    /// The Mac gained (`true`) or lost (`false`) its network path.
    NetworkChanged { online: bool },
    /// Files changed under the folders of [`Effect::WatchSessionFiles`]
    /// (already debounced, about a second): the paths FSEvents reported.
    SessionFilesChanged(Vec<PathBuf>),
}

/// An action only the UI layer can perform. The UI does exactly this and
/// nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Bring the Settings window forward, rendering [`View::settings`].
    ShowSettings,
    /// Show the About panel, rendering [`View::about`].
    ShowAbout,
    /// Select this path in a Finder window.
    RevealInFinder(PathBuf),
    /// Open this URL in the default browser.
    OpenUrl(String),
    /// Open System Settings → Login Items, where only the user can turn
    /// launch at login back on.
    OpenLoginItemsSettings,
    /// Bring the Diagnostics window forward, rendering [`View::diagnostics`].
    ShowDiagnostics,
    /// Put this text on the general pasteboard.
    CopyToClipboard(String),
    /// Bring the Claude desktop app (`com.anthropic.claudefordesktop`)
    /// forward, opening it if it isn't running. Nothing ever quits or
    /// relaunches it.
    BringDesktopAppForward,
    /// Post a user notification.
    Notify { title: String, body: String },
    /// Post a user notification with one action button titled `action`;
    /// clicking it sends `command`. Nothing happens unless it is clicked.
    NotifyWithAction {
        title: String,
        body: String,
        action: String,
        command: Command,
    },
    /// Quit the app. The core has already flushed what it needed to.
    Terminate,
    /// Ask for a nickname for `name` (prefilled with `current`), then send
    /// [`Command::SetNickname`] with `target` and the answer. Cancel sends
    /// nothing.
    AskNickname {
        name: String,
        current: Option<String>,
        target: NicknameTarget,
    },
    /// Bring the All Sessions window forward, rendering
    /// [`View::all_sessions`].
    ShowAllSessions,
    /// Watch these folders (recursively, debounced about a second) and send
    /// [`Event::SessionFilesChanged`] for what changes, until
    /// [`Effect::StopWatchingSessionFiles`]. Replaces any earlier watch.
    WatchSessionFiles(Vec<PathBuf>),
    /// Stop the watch [`Effect::WatchSessionFiles`] started.
    StopWatchingSessionFiles,
    /// Ask before an irreversible action: an alert with `title` and
    /// `message`, a destructive confirm button and Cancel. Confirming sends
    /// `confirm`; Cancel sends nothing.
    ConfirmRemoval {
        title: String,
        message: String,
        confirm: Command,
    },
}

/// Everything the UI shows, recomputed after every command and event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub menu_bar: MenuBarView,
    pub menu: MenuView,
    pub settings: SettingsView,
    pub about: AboutView,
    pub all_sessions: AllSessionsView,
    /// The first-run window, shown while `Some` (until the user answers).
    pub first_run: Option<FirstRunView>,
    pub diagnostics: DiagnosticsView,
}

/// The Diagnostics window: every format check, the Claude versions, and the
/// paused features with their Resume buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsView {
    pub checks: Vec<CheckRow>,
    pub versions: ClaudeVersions,
    pub paused: Vec<PausedFeature>,
    /// What "Copy report" copies: check names, results, field names and
    /// versions; never ids, emails, paths or values.
    pub report: String,
}

/// One format check in Diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckRow {
    /// For example "Sidebar entries".
    pub name: String,
    pub status: CheckStatus,
    /// What was found, in field names (for example "unknown fields: foo").
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Pass,
    Fail,
    /// Nothing of that format was on the Mac to check.
    NeverSeen,
}

/// A feature under a Safety pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PausedFeature {
    pub feature: SafetyFeature,
    /// "CLI switching paused".
    pub title: String,
    /// Why, for example "settings.json sets CLAUDE_CONFIG_DIR".
    pub reason: String,
    /// `Resume <feature>`, present only once the checks pass again:
    /// [`Command::ResumeFeature`].
    pub resume: Option<Command>,
}

/// The installed Claude versions, read from files (nothing is spawned).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClaudeVersions {
    /// The Terminal's Claude Code: where `~/.local/bin/claude` links to.
    pub claude_code: Option<String>,
    /// The Claude Code versions bundled with the desktop app, oldest first.
    pub desktop_claude_code: Vec<String>,
    /// The desktop app's `CFBundleShortVersionString`.
    pub desktop: Option<String>,
}

/// The first-run window: the one-time launch-at-login offer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstRunView {
    /// The checkbox's initial state. Closing the window or clicking its
    /// button sends [`Command::SetLaunchAtLogin`] with the checkbox's state.
    pub launch_at_login: bool,
}

/// The launch-at-login toggle in Settings, as macOS currently has it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchAtLoginView {
    /// Whether the toggle is on: the app opens at login right now.
    pub on: bool,
    /// Whether the toggle can be clicked.
    pub enabled: bool,
    /// An explanatory line under the toggle, when there is something to say.
    pub note: Option<String>,
}

/// The status item in the menu bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuBarView {
    /// Short text shown in the menu bar (kept short for the notch): the CLI's
    /// Active Profile and its tightest limit, like `Perso 62%`.
    pub label: String,
    /// Drawn dimmed (the figure is stale).
    pub dimmed: bool,
}

/// The status item's menu, top to bottom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuView {
    pub items: Vec<MenuItem>,
}

/// One row of the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    /// A disabled line drawn as a warning (for example, a set-aside settings
    /// file).
    Warning(String),
    Separator,
    /// A clickable item; clicking sends `command`.
    Action {
        title: String,
        command: Command,
    },
    /// A clickable item with a check mark when `checked`; clicking sends
    /// `command`.
    Check {
        title: String,
        checked: bool,
        command: Command,
    },
    /// A disabled plain line (for example `email · Organization · plan` in a
    /// Profile's hover submenu).
    Detail(String),
    /// An item opening a nested submenu of `items`.
    Submenu {
        title: String,
        items: Vec<MenuItem>,
    },
    /// A signed-in Profile's row, drawn as a custom view with limit bars.
    Profile(ProfileRow),
    /// A Discovered Profile, drawn greyed. Clicking the row sends `command`.
    Discovered(DiscoveredRow),
}

/// A Discovered Profile's row in the menu: an account the Mac has evidence of
/// that has no Profile yet. It never shows limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredRow {
    /// `Sign in to <name>…`, where the name is the nickname, else
    /// `email · Organization`, else "Unknown Login".
    pub title: String,
    /// A second line for an Unknown Login: Session count, last activity and
    /// "Desktop's last account" when it applies.
    pub subtitle: Option<String>,
    /// Sent when the row is clicked: [`Command::SignInDiscovered`].
    pub command: Command,
    /// The hover submenu: details (uuids, first 8 characters), Rename…, Hide.
    pub submenu: Vec<MenuItem>,
}

/// One Profile's row in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    /// The nickname, else `email · Organization`.
    pub title: String,
    /// Badges after the name, "CLI" first, then "Desktop": which clients use
    /// this Profile.
    pub badges: Vec<String>,
    /// What clicking the row sends, if it does anything.
    pub command: Option<Command>,
    /// Session limit, Weekly limit, then each model-specific weekly limit, in
    /// that order; empty until the first Limit reading.
    pub limits: Vec<LimitBar>,
    /// A short line under the bars (for example "as of 14:02").
    pub note: Option<String>,
    /// Drawn dimmed: the reading is not current (stale, unreadable, or the
    /// Login expired).
    pub dimmed: bool,
    /// The most-headroom marker: of the non-stale signed-in Profiles (two or
    /// more), this one has the lowest tightest limit.
    pub most_headroom: bool,
    /// Banked reset and Session refill lines, when Claude Code reports them
    /// (for example "Session refill available"). Never redeemable from here.
    pub offers: Vec<String>,
    /// The hover submenu.
    pub submenu: Vec<MenuItem>,
}

/// One limit, drawn as an `NSLevelIndicator` (continuous capacity, warning at
/// 70, critical at 90) with its percentage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitBar {
    /// "Session", "Weekly", or the model's display name.
    pub label: String,
    /// Utilization, whole percent (0–100, rounded down like Claude Code's
    /// `/usage`).
    pub percent: u32,
    /// The limit's reset time has passed since the reading: drawn as
    /// "Reset" at 0%.
    pub reset: bool,
}

/// The Settings window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsView {
    /// Whether the Verbose logging switch is on.
    pub verbose_logging: bool,
    /// The explanatory line under the switch.
    pub verbose_logging_note: String,
    pub launch_at_login: LaunchAtLoginView,
    pub cli_switching: CliSwitchingView,
    /// Profiles > Hidden (n)…: each hidden Discovered Profile still on the
    /// Mac.
    pub hidden_discovered: Vec<HiddenDiscovered>,
    pub notifications: NotificationsView,
}

/// Settings > Notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationsView {
    /// The thresholds every limit of every signed-in Profile warns at, in
    /// whole percent, lowest first ([`Command::SetThresholds`]).
    pub thresholds: Vec<u32>,
    /// Whether a limit that warned notifies again when it resets
    /// ([`Command::SetResetNotification`]). Off by default.
    pub reset_notification: bool,
}

/// One entry of Settings > Hidden (n)….
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenDiscovered {
    /// The nickname, else `email · Organization`, else "Unknown Login" with
    /// the account's first 8 characters.
    pub title: String,
    /// Sent by its Restore item: [`Command::UnhideDiscovered`].
    pub restore: Command,
}

/// The All Sessions window: every Session on the Mac, across Profiles and
/// the CLI, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllSessionsView {
    /// The Sessions matching [`AllSessionsView::search`], newest first.
    pub rows: Vec<SessionRow>,
    /// The search field's text.
    pub search: String,
    /// Whether the list is still filling in (the index is being built).
    pub loading: bool,
    /// The sidebar, top to bottom: All Sessions and Running, Projects,
    /// Sidebars (one per Profile, then CLI only).
    pub sidebar: Vec<SidebarGroup>,
    /// The sidebar item picked; [`AllSessionsView::rows`] are its Sessions.
    pub filter: SessionFilter,
    /// The selected Session, when it is in [`AllSessionsView::rows`].
    pub detail: Option<SessionDetail>,
}

/// Which Sessions the All Sessions list shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum SessionFilter {
    /// Every Session, except those archived in every Profile that has them.
    #[default]
    All,
    /// Sessions with a running dot.
    Running,
    /// Sessions whose working folder is this absolute path.
    Project(String),
    /// Sessions with an entry in this Profile's Sidebar (archived or not).
    Sidebar(DiscoveredPair),
    /// Sessions with no Sidebar entry in any Profile.
    CliOnly,
}

/// A heading (none for the first group) and its items.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarGroup {
    pub title: Option<String>,
    pub items: Vec<SidebarItem>,
}

/// One pickable sidebar item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarItem {
    /// For example "All Sessions", "app", "Perso", "Work (not signed in)".
    pub title: String,
    /// What picking it sends in [`Command::FilterSessions`].
    pub filter: SessionFilter,
}

/// The All Sessions detail pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionDetail {
    pub id: String,
    pub title: String,
    /// The working folder, `~` for the home folder.
    pub folder: Option<String>,
    pub branch: Option<String>,
    /// For example "Last used in the CLI, 5 min ago".
    pub last_used: String,
    /// Real prompts in the Transcript; `None` once it is deleted.
    pub turns: Option<u64>,
    pub first_prompt: Option<String>,
    pub dot: Option<RunningDot>,
    /// One line per Profile, in sidebar order.
    pub sidebars: Vec<SidebarPresence>,
    /// Why nothing can be opened, when the Transcript was deleted.
    pub deleted: Option<String>,
}

/// Whether one Profile's Sidebar has an entry for the Session (✓ or –).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarPresence {
    /// The Profile, as its sidebar item names it.
    pub profile: String,
    pub has_entry: bool,
    /// The entry is archived in this Profile.
    pub archived: bool,
    /// This is the desktop app's Active Profile ("desktop now").
    pub desktop_now: bool,
}

/// One Session in the All Sessions list, drawn on two lines.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRow {
    /// The Session's Transcript id.
    pub id: String,
    /// First line: the title, else the first prompt in quotes.
    pub title: String,
    /// Second line: `project · last activity` (for example
    /// `app · 5 min ago`).
    pub subtitle: String,
    /// A running dot before the title, when the Session runs.
    pub dot: Option<RunningDot>,
    /// The Transcript is gone and only Sidebar entries remain: drawn greyed,
    /// with nothing to open.
    pub deleted: bool,
}

/// Where a running Session runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunningDot {
    /// Green: a Claude Code process outside the desktop app (Terminal, an
    /// IDE, …), in any status.
    Terminal,
    /// Orange: a desktop app turn that is busy or waiting.
    Desktop,
}

/// The CLI switching switch in Settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliSwitchingView {
    /// Whether CLI switching is on (the shim is installed).
    pub on: bool,
    /// The exact `PATH` line the `~/.zshrc` block holds, shown before the
    /// user opts in.
    pub line: String,
    /// What the last change left for the user to do, if anything (for
    /// example the lines of an edited block to remove by hand).
    pub note: Option<String>,
}

/// The About panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AboutView {
    pub name: String,
    pub version: String,
    pub tagline: String,
    pub disclaimer: String,
}
