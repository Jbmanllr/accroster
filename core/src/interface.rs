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
    SignInDiscovered(ProfileKey),
    /// Hide in a Discovered Profile's submenu.
    HideDiscovered(ProfileKey),
    /// Restore in Settings > Hidden (n)….
    UnhideDiscovered(ProfileKey),
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
    /// A double-click in All Sessions, or a Session in the menu's Recent
    /// Sessions: opens the Session (by Transcript id) in the client it was
    /// last used in. When that would need a choice (Claude's Active Profile
    /// has no entry for it), shows it in All Sessions instead.
    OpenSession(String),
    /// "Resume in Terminal" / "Show in Terminal" for a Session (by id).
    ResumeInTerminal(String),
    /// "Open in Claude" / "Open in Claude as <Profile>" for a Session (by
    /// id), in the desktop app's Active Profile.
    OpenInClaude(String),
    /// "Switch Claude to <Profile>, then open…": a Guided switch to Profile
    /// `profile` (by id), then opening Session `session` (by Transcript id)
    /// from that Profile's entry.
    SwitchClaudeThenOpen { profile: String, session: String },
    /// The Sidebar sync mode in Settings.
    SetSidebarSync(SidebarSyncMode),
    /// "Sync now" in Settings: copy every Session's Sidebar entry into every
    /// other Profile's sidebar.
    SyncSidebarNow,
    /// "Include SSH Sessions" in Settings (on by default).
    SetIncludeSshSessions(bool),
    /// "Uninstall…" in Settings: asks first ([`Effect::ConfirmUninstall`]).
    AskUninstall,
    /// The uninstall confirmed, or its Retry: unhook the CLI, remove every
    /// Profile, unregister launch at login, delete the app's folder, then
    /// [`Effect::TrashAppAndQuit`]. Each step is skipped when already done;
    /// a failed sign-out stops it before the app's folder.
    Uninstall,
}

/// How Sidebar sync runs (ADR 0003). Saved in `settings.json` as
/// `"off"` / `"manual"`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SidebarSyncMode {
    /// Sync never writes (the default).
    #[default]
    Off,
    /// Sync runs on "Sync now".
    Manual,
    /// Sync also runs a few seconds after each change to the desktop's
    /// Active Profile folder, and before a Guided switch.
    Automatic,
}

/// A feature that writes Claude's files and can get a Safety pause
/// (ADR 0004).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum SafetyFeature {
    /// The pointer and row clicks.
    CliSwitching,
    /// Creating or repairing the links from Profile folders to
    /// `~/.claude` (a replaced `projects`, `file-history` or `session-env`
    /// link pauses them).
    Links,
    /// Writing a Profile's `.claude.json`: seeding it from `~/.claude.json`,
    /// and putting its Login back after another Login's sign-in there was
    /// undone.
    Seeding,
    /// Copying Sidebar entries between Profiles.
    SidebarSync,
}

impl SafetyFeature {
    pub const ALL: [SafetyFeature; 4] = [
        SafetyFeature::CliSwitching,
        SafetyFeature::Links,
        SafetyFeature::Seeding,
        SafetyFeature::SidebarSync,
    ];

    /// How the menu, notifications and Diagnostics name it ("Resume
    /// <name>", "<name> paused").
    pub fn name(self) -> &'static str {
        match self {
            SafetyFeature::CliSwitching => "CLI switching",
            SafetyFeature::Links => "Profile links",
            SafetyFeature::Seeding => "Seeding .claude.json",
            SafetyFeature::SidebarSync => "Sidebar sync",
        }
    }

    /// Its key in `state/safety.json` and the log.
    pub(crate) fn key(self) -> &'static str {
        match self {
            SafetyFeature::CliSwitching => "cliSwitching",
            SafetyFeature::Links => "links",
            SafetyFeature::Seeding => "seeding",
            SafetyFeature::SidebarSync => "sidebarSync",
        }
    }
}

/// What tells Profiles apart: a Login's account uuid and an Organization
/// uuid. A Discovered Profile is known by its key alone; a Sidebar folder is
/// `claude-code-sessions/<account>/<org>/`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ProfileKey {
    pub account_uuid: String,
    pub organization_uuid: String,
}

/// What a nickname is given to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NicknameTarget {
    /// A Profile, by id.
    Profile(String),
    Discovered(ProfileKey),
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
            Command::SetSidebarSync(_) => "set_sidebar_sync",
            Command::SyncSidebarNow => "sync_sidebar_now",
            Command::SetIncludeSshSessions(_) => "set_include_ssh_sessions",
            Command::AskUninstall => "ask_uninstall",
            Command::Uninstall => "uninstall",
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
            Command::OpenSession(_) => "open_session",
            Command::ResumeInTerminal(_) => "resume_in_terminal",
            Command::OpenInClaude(_) => "open_in_claude",
            Command::SwitchClaudeThenOpen { .. } => "switch_claude_then_open",
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
    /// Files changed under the folders of [`Effect::WatchSidebarFolders`]
    /// (already debounced, about a second): the paths FSEvents reported.
    SidebarFilesChanged(Vec<PathBuf>),
    /// A notification's action button was clicked, maybe one posted before
    /// a relaunch: the core checks it still applies.
    NotificationActionClicked(NotificationAction),
}

/// What a notification's action button does, as the notification carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotificationAction {
    /// Make this Profile the CLI's Active Profile, if it still exists and
    /// CLI switching is on and free to change.
    SwitchCli { profile: String },
}

const SWITCH_CLI: &str = "switch-cli:";

impl NotificationAction {
    /// The text the UI stores in the notification.
    pub fn encode(&self) -> String {
        match self {
            NotificationAction::SwitchCli { profile } => format!("{SWITCH_CLI}{profile}"),
        }
    }

    /// Reads back [`NotificationAction::encode`]'s text; `None` for anything
    /// else (a notification from another version).
    pub fn decode(text: &str) -> Option<NotificationAction> {
        let profile = text.strip_prefix(SWITCH_CLI)?;
        (!profile.is_empty()).then(|| NotificationAction::SwitchCli {
            profile: profile.to_owned(),
        })
    }
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
    /// clicking it sends [`Event::NotificationActionClicked`] with
    /// `on_click`. Nothing happens unless it is clicked. The UI keeps
    /// [`NotificationAction::encode`] in the notification itself, so a click
    /// after a relaunch still arrives.
    NotifyWithAction {
        title: String,
        body: String,
        action: String,
        on_click: NotificationAction,
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
    /// Watch these folders (recursively, debounced about a second) and send
    /// [`Event::SidebarFilesChanged`] for what changes, until
    /// [`Effect::StopWatchingSidebarFolders`]. Independent of the All
    /// Sessions watch; replaces any earlier Sidebar watch.
    WatchSidebarFolders(Vec<PathBuf>),
    /// Stop the watch [`Effect::WatchSidebarFolders`] started.
    StopWatchingSidebarFolders,
    /// Open a new Terminal window and run this shell command line in it (it
    /// is already quoted for `sh`).
    RunInTerminal(String),
    /// Bring the Terminal window whose tab runs on this terminal device
    /// (`ttys004`) forward; without one, just bring Terminal forward.
    ShowInTerminal { tty: Option<String> },
    /// Ask before an irreversible action: an alert with `title` and
    /// `message`, a destructive confirm button and Cancel. Confirming sends
    /// `confirm`; Cancel sends nothing.
    ConfirmRemoval {
        title: String,
        message: String,
        confirm: Command,
    },
    /// Ask before uninstalling: an alert with `title` and `message`, a
    /// destructive "Uninstall" button and Cancel. Confirming sends
    /// `confirm`; Cancel sends nothing.
    ConfirmUninstall {
        title: String,
        message: String,
        confirm: Command,
    },
    /// The last uninstall step: move the running app's bundle to the Trash,
    /// then quit. The core has already removed everything else it owns.
    TrashAppAndQuit,
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
    /// Active Profile and its tightest limit, like `Perso 62%`. `None` when
    /// there is no Profile to show: the app's icon stands alone.
    pub label: Option<String>,
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
    /// The status line at the top of the menu: what a Guided switch or a
    /// Sidebar sync is doing, or how it ended. Disabled; the actions that
    /// answer it follow as [`MenuItem::Action`]s.
    Status {
        text: String,
        tone: StatusTone,
    },
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

/// How a [`MenuItem::Status`] line is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// Under way ("Syncing… 3 of 10").
    Working,
    /// Waits on the user: drawn prominently ("Can't tell whether Claude is
    /// busy…", "Sign out in Claude, then sign in as …").
    NeedsYou,
    /// Ended well ("Claude now uses …").
    Done,
    /// Ended without doing what was asked ("Gave up waiting after 30 min…").
    Failed,
}

/// A Discovered Profile's row in the menu: an account the Mac has evidence of
/// that has no Profile yet. It never shows limits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredRow {
    /// `Sign in to <name>…`, where the name is the nickname, else the
    /// Login's Profile name, else "Unknown Login".
    pub title: String,
    /// A second line for an Unknown Login: Session count, last activity and
    /// "Desktop's last Login" when it applies.
    pub subtitle: Option<String>,
    /// Sent when the row is clicked: [`Command::SignInDiscovered`].
    pub command: Command,
    /// The hover submenu: details (uuids, first 8 characters), Rename…, Hide.
    pub submenu: Vec<MenuItem>,
}

/// One Profile's row in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    /// The nickname, else the Profile name (`<full name> · <email>`, with
    /// the Organization when it isn't the personal default).
    pub title: String,
    /// Badges after the name, CLI first, then Desktop: which clients use
    /// this Profile.
    pub badges: Vec<Badge>,
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
    /// The most-headroom marker text ([`MOST_HEADROOM`]), drawn after the
    /// badges: of the non-stale signed-in Profiles (two or more), this one
    /// has the lowest tightest limit.
    pub marker: Option<String>,
    /// Drawn as a muted "default login" after the badges: CLI switching is
    /// off and the default `~/.claude` folder is signed in to this Profile's
    /// Login and Organization, so the CLI's limits are this row's.
    pub default_login: bool,
    /// Banked reset and Session refill lines, when Claude Code reports them
    /// (for example "Session refill available"). Never redeemable from here.
    pub offers: Vec<String>,
    /// The hover submenu.
    pub submenu: Vec<MenuItem>,
}

/// The most-headroom marker's text. It can move to another row while the
/// menu is open, so the UI keeps room for it on every row.
pub const MOST_HEADROOM: &str = "most headroom";

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
    /// When the limit resets, drawn after the percentage: "resets in 1 h 40
    /// min" for the Session limit, "resets Thu 09:00" for the others. `None`
    /// when Claude Code didn't say, or once it has passed.
    pub resets: Option<String>,
}

/// A client using a Profile, drawn as a coloured pill after its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Badge {
    /// The CLI's Active Profile.
    Cli,
    /// The Claude desktop app's Active Profile.
    Desktop,
}

impl Badge {
    /// The pill's text.
    pub fn title(self) -> &'static str {
        match self {
            Badge::Cli => "CLI",
            Badge::Desktop => "Desktop",
        }
    }
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
    /// Profiles: every Profile, in the user's order, to reorder, rename and
    /// mute.
    pub profiles: Vec<ProfileSettings>,
    /// Profiles > Hidden (n)…: each hidden Discovered Profile still on the
    /// Mac.
    pub hidden_discovered: Vec<HiddenDiscovered>,
    pub sidebar_sync: SidebarSyncView,
    pub notifications: NotificationsView,
    pub uninstall: UninstallView,
}

/// "Uninstall…" in Settings, and why the last uninstall stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct UninstallView {
    /// Why the last uninstall stopped and what is left, when it did.
    pub note: Option<String>,
    /// "Retry", present after a stopped uninstall: [`Command::Uninstall`].
    pub retry: Option<Command>,
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

/// Sidebar sync in Settings: the mode and "Sync now".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidebarSyncView {
    pub mode: SidebarSyncMode,
    /// "Sync now", present while sync is on: [`Command::SyncSidebarNow`].
    pub sync_now: Option<Command>,
    /// What the last sync did, or why it didn't run.
    pub note: Option<String>,
    /// "Include SSH Sessions": [`Command::SetIncludeSshSessions`].
    pub ssh_sessions: Toggle,
}

/// One Profile in Settings > Profiles, in the user's order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileSettings {
    /// The nickname, else the Profile name.
    pub title: String,
    /// The Profile name, when a nickname titles the line.
    pub detail: Option<String>,
    /// `None` for the first Profile.
    pub move_up: Option<Command>,
    /// `None` for the last Profile.
    pub move_down: Option<Command>,
    /// Asks for the nickname.
    pub rename: Command,
    /// Mute notifications; `None` for a Profile with no Login.
    pub mute: Option<Toggle>,
}

/// A checkbox: whether it is on, and what clicking it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toggle {
    pub on: bool,
    pub command: Command,
}

/// One hidden Discovered Profile in Settings > Hidden (n)….
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HiddenDiscovered {
    /// The nickname, else the Profile name, else "Unknown Login" with
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
    Sidebar(ProfileKey),
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
    /// The Open section, top to bottom; empty when the Transcript was
    /// deleted.
    pub open: Vec<OpenChoice>,
}

/// One button of the detail pane's Open section, with what it costs spelled
/// out under it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenChoice {
    /// For example "Resume in Terminal".
    pub title: String,
    /// For example "New turns count against Perso (CLI)".
    pub note: Option<String>,
    /// Sent when clicked.
    pub command: Command,
    /// Drawn disabled when `false` (`note` then says why).
    pub enabled: bool,
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
