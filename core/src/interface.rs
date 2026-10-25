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
    /// "Limit resets → claude.ai" in a Profile's hover submenu: open
    /// claude.ai Settings > Usage, where the user checks resets by hand.
    OpenLimitResets,
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
            Command::OpenLimitResets => "open_limit_resets",
        }
    }
}

/// Something that happened outside the core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// Time passed. The driver sends one every [`crate::driver::TICK_INTERVAL`];
    /// the core reads the actual time from its [`crate::Clock`].
    Tick,
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
    /// Quit the app. The core has already flushed what it needed to.
    Terminate,
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
}

/// One Profile's row in the menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRow {
    /// The nickname, else `email · Organization`.
    pub title: String,
    /// Badges after the name: `CLI` when it is the CLI's Active Profile.
    pub badges: Vec<String>,
    /// What clicking the row sends, if it does anything.
    pub command: Option<Command>,
    /// Session limit, Weekly limit, then each model-specific weekly limit, in
    /// that order; empty until the first Limit reading.
    pub limits: Vec<LimitBar>,
    /// A short line under the bars (for example "Couldn't read limits").
    pub note: Option<String>,
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
