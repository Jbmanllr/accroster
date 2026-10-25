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
    /// Put this text on the general pasteboard.
    CopyToClipboard(String),
    /// Bring the Claude desktop app (`com.anthropic.claudefordesktop`)
    /// forward, opening it if it isn't running. Nothing ever quits or
    /// relaunches it.
    BringDesktopAppForward,
    /// Post a user notification.
    Notify { title: String, body: String },
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
}

/// Everything the UI shows, recomputed after every command and event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub menu_bar: MenuBarView,
    pub menu: MenuView,
    pub settings: SettingsView,
    pub about: AboutView,
    /// The first-run window, shown while `Some` (until the user answers).
    pub first_run: Option<FirstRunView>,
    /// The installed Claude versions, for Diagnostics.
    pub versions: ClaudeVersions,
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
    /// Drawn dimmed: the reading is stale or couldn't be refreshed.
    pub dimmed: bool,
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
