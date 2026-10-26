//! [`Core`]: the one object the UI layer talks to.

use std::fmt;
use std::io;
use std::path::PathBuf;
use std::time::Duration;

use jiff::SignedDuration;

use crate::backups;
use crate::checks;
use crate::claude_code::{self, ClaudeLocator};
use crate::cli_switcher;
use crate::desktop::{DesktopAccount, DesktopBridge, DesktopPaths, NewestEntries, Reopen, Target};
use crate::discovered::{self, Evidence};
use crate::env::{Environment, LoginItemStatus};
use crate::interface::{
    AboutView, Badge, ClaudeVersions, CliSwitchingView, Command, Effect, Event, FirstRunView,
    HiddenDiscovered, LaunchAtLoginView, MOST_HEADROOM, MenuBarView, MenuItem, MenuView,
    NicknameTarget, NotificationAction, NotificationsView, ProfileKey, ProfileRow, ProfileSettings,
    SafetyFeature, SettingsView, SidebarSyncMode, SidebarSyncView, Toggle, UninstallView, View,
};
use crate::logging::{Field, Level, Logger};
use crate::monitor::{LimitMonitor, Watched};
use crate::notifications::{self, Choices, Headroom, Notifier, Subject};
use crate::opening;
use crate::profiles::{self, Listed, ProfileRegistry, SignInTarget};
use crate::runner::Jobs;
use crate::safety::{CheckId, Safety};
use crate::sessions::{SessionIndex, SidebarProfile};
use crate::settings::{self, DiscoveredChoice, Settings, Thresholds};
use crate::sidebar_sync;
use crate::storage::{self, AppPaths};

/// The tagline, shown in About and the README.
pub const TAGLINE: &str =
    "A menu bar app for Claude Code accounts: limits, sessions and one-click switching.";

/// The non-affiliation disclaimer, shown in About and the README.
pub const DISCLAIMER: &str = "AccRoster is an independent open-source project. It is not affiliated with, endorsed by, or sponsored by Anthropic. Claude and Claude Code are trademarks of Anthropic, PBC.";

/// How long Verbose logging stays on once turned on.
const VERBOSE_LOGGING_LASTS: SignedDuration = SignedDuration::from_hours(24);

/// How many Sessions the menu's Recent Sessions submenu lists.
const RECENT_SESSIONS: usize = 10;

/// claude.ai Settings > Usage, where Banked resets can be checked by hand.
const LIMIT_RESETS_URL: &str = "https://claude.ai/settings/usage";

/// Why the core couldn't start. The UI shows it and quits.
#[derive(Debug)]
pub struct LaunchError {
    path: PathBuf,
    source: io::Error,
}

impl fmt::Display for LaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "couldn't set up {}: {}",
            self.path.display(),
            self.source
        )
    }
}

impl std::error::Error for LaunchError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.source)
    }
}

/// All of the app's behaviour and state.
///
/// Drive it with [`Core::command`] and [`Core::event`], perform the returned
/// [`Effect`]s, and render [`Core::view`]. Each call runs to completion before
/// the next; the [`crate::driver`] runs the core on its own thread so slow work
/// never blocks the UI.
pub struct Core {
    env: Environment,
    paths: AppPaths,
    logger: Logger,
    settings: Settings,
    /// Menu warning lines for this run.
    warnings: Vec<String>,
    profiles: ProfileRegistry,
    monitor: LimitMonitor,
    /// Which limit notifications already fired.
    notifier: Notifier,
    /// What the last CLI switching change left for the user to do.
    cli_note: Option<String>,
    desktop: DesktopBridge,
    /// Reopen Claude: offered after a sync added to the sidebar Claude is
    /// using while it runs.
    reopen: Reopen,
    /// Where `claude` is, once found.
    claude: ClaudeLocator,
    /// Discovered pairs as of the last scan (launch, menu open).
    discovered: Vec<Evidence>,
    /// What the last scan read, so the next reads only what changed.
    discovery: discovered::Scanner,
    /// The All Sessions list and the Session index behind it.
    sessions: SessionIndex,
    /// Format check results and Safety pauses.
    safety: Safety,
    /// The Claude versions as of the last check (launch, menu open).
    versions: ClaudeVersions,
    /// Each Profile's links as of the last check, by Profile id.
    links: Vec<ProfileLinks>,
    /// What the last "Sync now" did, for Settings.
    sync_note: Option<String>,
    /// The sync under way, written one sidebar per tick.
    sync: Option<sidebar::ActiveSync>,
    /// How the last sync ended, for the menu (for a while).
    sync_result: Option<sidebar::SyncResult>,
    /// The sidebars' watch while Sidebar sync is on.
    sidebar_watch: sidebar_sync::Watch,
    /// The Sidebar entries as the last sync read or wrote them.
    sidebar_entries: sidebar_sync::EntryCache,
    /// `claude --desktop --resume` runs, by Session id.
    openings: Jobs<String>,
    /// A Resume whose checks wait for the `auth status` cross-check.
    resume_waiting: Option<SafetyFeature>,
    /// An uninstall waiting for its sign-outs, with what is left for the
    /// user to do so far.
    uninstalling: Option<Vec<String>>,
    /// Why the last uninstall stopped, for Settings (offers Retry).
    uninstall_note: Option<String>,
    /// The app's folder is gone: nothing is read, written or started again.
    uninstalled: bool,
    /// What finding the desktop's Organization last read: the menu and
    /// [`Core::watched`] ask for it several times per input.
    newest_entries: NewestEntries,
}

/// What the registry, the monitor and the bridge need from the core, as
/// [`Core::context`] gives it, borrowing only the core's `env`, `paths`,
/// `logger`, `settings` and `safety`: the core's other parts stay free to
/// change while it is in use.
macro_rules! context {
    ($core:expr) => {
        context_of(
            &$core.env,
            &$core.paths,
            &$core.logger,
            &$core.settings,
            &$core.safety,
            &$core.claude,
        )
    };
}

fn context_of<'a>(
    env: &'a Environment,
    paths: &'a AppPaths,
    logger: &'a Logger,
    settings: &Settings,
    safety: &Safety,
    claude: &'a ClaudeLocator,
) -> profiles::Context<'a> {
    profiles::Context {
        env,
        paths,
        logger,
        verbose: verbose_logging(settings, env),
        paused: safety.paused(),
        claude: claude.program(),
    }
}

/// Whether Verbose logging is on now.
fn verbose_logging(settings: &Settings, env: &Environment) -> bool {
    settings
        .verbose_logging_until
        .is_some_and(|until| until > env.clock.now())
}

/// How often the core wants a tick while a short Claude Code run is out.
const RUN_TICK: Duration = Duration::from_millis(250);

mod cli;
mod open;
mod self_check;
mod sidebar;
mod uninstaller;

use self_check::{AuthCheck, ProfileLinks};

impl Core {
    /// Starts the core: creates the app's folder if needed, opens the log and
    /// loads settings.
    pub fn launch(env: Environment) -> Result<Core, LaunchError> {
        let paths = AppPaths::new(&env.home);
        let fail = |path: PathBuf| move |source| LaunchError { path, source };
        paths
            .ensure_layout()
            .map_err(fail(paths.root().to_path_buf()))?;
        let logger = Logger::new(paths.app_log(), env.home.clone());
        let paths_for_safety = paths.clone();
        let loaded =
            settings::load(&paths.settings(), env.clock.now()).map_err(fail(paths.settings()))?;
        let mut core = Core {
            env,
            paths,
            logger,
            settings: loaded.settings,
            warnings: Vec::new(),
            profiles: ProfileRegistry::default(),
            monitor: LimitMonitor::default(),
            notifier: Notifier::default(),
            cli_note: None,
            desktop: DesktopBridge::default(),
            reopen: Reopen::default(),
            claude: ClaudeLocator::default(),
            discovered: Vec::new(),
            discovery: discovered::Scanner::default(),
            safety: Safety::load(&paths_for_safety),
            versions: ClaudeVersions::default(),
            links: Vec::new(),
            sessions: SessionIndex::default(),
            sync_note: None,
            sync: None,
            sync_result: None,
            sidebar_watch: sidebar_sync::Watch::default(),
            sidebar_entries: sidebar_sync::EntryCache::default(),
            openings: Jobs::default(),
            resume_waiting: None,
            uninstalling: None,
            uninstall_note: None,
            uninstalled: false,
            newest_entries: NewestEntries::default(),
        };
        core.log(
            Level::Info,
            "launch",
            &[
                ("version", Field::Text(env!("CARGO_PKG_VERSION"))),
                ("app_folder", Field::Path(core.paths.root())),
            ],
        );
        match backups::prune(&core.paths.backups(), core.env.clock.now()) {
            Ok(0) => {}
            Ok(pruned) => core.log(
                Level::Info,
                "backups pruned",
                &[("count", Field::Number(pruned as i64))],
            ),
            Err(error) => {
                let error = error.to_string();
                core.log(
                    Level::Warn,
                    "backups not pruned",
                    &[("error", Field::Text(&error))],
                );
            }
        }
        if let Some(set_aside) = loaded.set_aside {
            core.log(
                Level::Warn,
                "settings set aside",
                &[
                    ("reason", Field::Text(&set_aside.reason)),
                    ("kept_as", Field::Path(&set_aside.path)),
                ],
            );
            let name = set_aside
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            core.warnings.push(format!(
                "Settings were unreadable and were reset · old file kept as {name}"
            ));
        }
        core.locate_claude();
        let (profiles, ctx) = core.profiles();
        *profiles = ProfileRegistry::load(&ctx);
        // Checks first: a Safety pause stops a feature before it writes.
        core.self_check(AuthCheck::IfVersionsChanged);
        let (profiles, ctx) = core.profiles();
        profiles.heal_links(&ctx);
        core.check_links();
        match cli_switcher::heal_shim(&core.paths) {
            Ok(false) => {}
            Ok(true) => core.log(Level::Info, "cli shim rewritten", &[]),
            Err(error) => {
                let error = error.to_string();
                core.log(
                    Level::Warn,
                    "cli shim not rewritten",
                    &[("error", Field::Text(&error))],
                );
            }
        }
        core.rescan_discovered();
        core.monitor = LimitMonitor::load(&core.context());
        core.notifier = Notifier::load(&core.context());
        // Recent Sessions in the first menu: the index fills in by ticks.
        core.sessions.warm_up(&core.env, &core.paths);
        Ok(core)
    }

    /// How soon the driver should send the next [`Event::Tick`]: every second
    /// while a sign-in is waiting (so its end shows at once), more often
    /// while a usage read runs, by the time the next read is due, and at most
    /// [`crate::driver::TICK_INTERVAL`].
    pub fn tick_interval(&self) -> Duration {
        if self.uninstalled {
            return crate::driver::TICK_INTERVAL;
        }
        if self.sessions.busy() || self.sync.is_some() {
            // The next batch of the Session index, as soon as input allows.
            return Duration::ZERO;
        }
        if self.profiles.busy() || !self.openings.is_empty() || self.claude.looking() {
            // Its end shows at once.
            return RUN_TICK;
        }
        let idle = if self.profiles.signing_in() || self.desktop.waiting() || self.reopen.waiting()
        {
            // An Automatic sync waits for the switch to end (see `Tick`).
            Duration::from_secs(1)
        } else {
            let sync_due = self.sidebar_watch.until_due(self.env.clock.now());
            sync_due.map_or(crate::driver::TICK_INTERVAL, |due| {
                due.min(crate::driver::TICK_INTERVAL)
            })
        };
        self.monitor
            .tick_interval(self.env.clock.now(), &self.watched(), idle)
    }

    /// Effects the core queued outside an input (a Safety pause noticed at
    /// launch). The driver hands them to the UI with the first view;
    /// [`Core::command`] and [`Core::event`] include them in what they return.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        if self.uninstalled {
            return Vec::new();
        }
        let mut effects = self.safety.take_effects();
        let sync_on = self.settings.sidebar_sync != SidebarSyncMode::Off;
        effects.extend(self.sidebar_watch.reconcile(&self.env, sync_on));
        effects
    }

    /// Handles a user intent.
    pub fn command(&mut self, command: Command) -> Vec<Effect> {
        if self.uninstalled {
            // Only quitting is left; the app's folder must not reappear.
            return match command {
                Command::Quit => vec![Effect::Terminate],
                _ => Vec::new(),
            };
        }
        self.log(
            Level::Debug,
            "command",
            &[("name", Field::Word(command.name()))],
        );
        self.locate_claude();
        let mut effects = self.handle(command);
        // A run the command started may have ended at once (it couldn't
        // start), and an uninstall with nothing to sign out ends now.
        effects.extend(self.act_on_runs());
        effects.extend(self.take_effects());
        effects
    }

    /// Checks on the sign-in, then [`Core::act_on_runs`].
    fn poll_runs(&mut self) -> Vec<Effect> {
        if self.uninstalled {
            return Vec::new();
        }
        let (profiles, ctx) = self.profiles();
        profiles.poll_sign_in(&ctx);
        self.act_on_runs()
    }

    /// Collects the short Claude Code runs that ended (`auth status`, `auth
    /// logout`, `--desktop --resume`) and acts on what they say.
    fn act_on_runs(&mut self) -> Vec<Effect> {
        if self.uninstalled {
            return Vec::new();
        }
        let (profiles, ctx) = self.profiles();
        profiles.run_jobs(&ctx);
        let mut effects = profiles.take_effects();
        if self.profiles.take_auth_disagreement() {
            let finding = checks::config_dir_disagreement(&self.versions, "in a new Profile");
            self.safety.record(CheckId::ConfigDirHonoured, finding);
        }
        for id in self.profiles.take_removed() {
            self.forget_removed(&id);
        }
        self.carry_over_nicknames();
        if let Some(answers) = self.profiles.take_cross_check() {
            self.cross_checked(&answers);
        }
        effects.extend(self.poll_openings());
        effects.extend(self.continue_uninstall());
        effects
    }

    fn handle(&mut self, command: Command) -> Vec<Effect> {
        match command {
            Command::OpenSettings => vec![Effect::ShowSettings],
            Command::OpenAbout => vec![Effect::ShowAbout],
            Command::Quit => {
                self.log(Level::Info, "quit", &[]);
                vec![Effect::Terminate]
            }
            Command::SetVerboseLogging(on) => {
                let until = on.then(|| self.env.clock.now() + VERBOSE_LOGGING_LASTS);
                self.set_verbose_logging_until(until);
                Vec::new()
            }
            Command::RevealLogs => vec![Effect::RevealInFinder(self.paths.app_log())],
            Command::AddProfile => {
                let (profiles, ctx) = self.profiles();
                profiles.add(&ctx, None);
                Vec::new()
            }
            Command::SignInDiscovered(pair) => {
                let Some(evidence) = self.discovered.iter().find(|e| e.pair == pair) else {
                    return Vec::new();
                };
                let nickname = self.choice(&pair).and_then(|c| c.nickname.as_deref());
                let target = SignInTarget {
                    email: evidence.email().map(str::to_owned),
                    name: evidence.name(nickname),
                    pair,
                };
                let (profiles, ctx) = self.profiles();
                profiles.add(&ctx, Some(target));
                Vec::new()
            }
            Command::HideDiscovered(pair) => {
                self.update_choice(&pair, |choice| choice.hidden = true);
                Vec::new()
            }
            Command::UnhideDiscovered(pair) => {
                self.update_choice(&pair, |choice| choice.hidden = false);
                Vec::new()
            }
            Command::Rename(NicknameTarget::Discovered(pair)) => {
                let Some(evidence) = self.discovered.iter().find(|e| e.pair == pair) else {
                    return Vec::new();
                };
                vec![Effect::AskNickname {
                    name: evidence.known_name(),
                    current: self.choice(&pair).and_then(|c| c.nickname.clone()),
                    target: NicknameTarget::Discovered(pair),
                }]
            }
            Command::SetNickname {
                target: NicknameTarget::Discovered(pair),
                nickname,
            } => {
                let nickname = nickname
                    .map(|name| name.trim().to_owned())
                    .filter(|name| !name.is_empty());
                self.update_choice(&pair, |choice| choice.nickname = nickname);
                Vec::new()
            }
            Command::CancelSignIn => {
                let (profiles, ctx) = self.profiles();
                profiles.cancel(&ctx);
                Vec::new()
            }
            Command::SetLaunchAtLogin(on) => self.set_launch_at_login(on),
            Command::SetCliSwitching(on) => {
                self.set_cli_switching(on);
                Vec::new()
            }
            Command::UseForNewCliRuns(id) => {
                self.use_for_new_cli_runs(&id);
                Vec::new()
            }
            Command::CopyShellOverride(id) => self
                .profiles
                .config_dir(&id)
                .map(|config_dir| {
                    vec![Effect::CopyToClipboard(cli_switcher::shell_override(
                        config_dir,
                    ))]
                })
                .unwrap_or_default(),
            Command::SwitchDesktopApp(id) => {
                let target = self
                    .profiles
                    .signed_in()
                    .iter()
                    .find(|profile| profile.id == id)
                    .map(Target::of);
                // One thing at a time with Claude: a reopen under way first.
                let Some(target) = target.filter(|_| !self.reopen.active()) else {
                    return Vec::new();
                };
                let (desktop, ctx) = self.desktop();
                let cleared = desktop.start(&ctx, target);
                self.guide_desktop_switch(cleared)
            }
            Command::ReopenDesktopApp => {
                if self.desktop.active() {
                    return Vec::new();
                }
                let (reopen, ctx) = self.reopen();
                reopen.start(&ctx)
            }
            Command::ConfirmReopenDesktopApp => {
                let (reopen, ctx) = self.reopen();
                reopen.confirm(&ctx)
            }
            Command::CancelReopenDesktopApp => {
                let (reopen, ctx) = self.reopen();
                reopen.cancel(&ctx);
                Vec::new()
            }
            Command::ConfirmDesktopSwitch => self.confirm_desktop_switch(),
            Command::NotifyWhenDesktopIdle => {
                self.desktop.notify_when_idle();
                Vec::new()
            }
            Command::CancelDesktopSwitch => {
                let (desktop, ctx) = self.desktop();
                desktop.cancel(&ctx);
                Vec::new()
            }
            Command::OpenLimitResets => vec![Effect::OpenUrl(LIMIT_RESETS_URL.into())],
            Command::OpenDiagnostics => vec![Effect::ShowDiagnostics],
            Command::CopyDiagnosticsReport => {
                vec![Effect::CopyToClipboard(self.diagnostics().report)]
            }
            Command::ResumeFeature(feature) => {
                self.resume(feature);
                Vec::new()
            }
            Command::Rename(NicknameTarget::Profile(id)) => self
                .profiles
                .nickname_prompt(&id, &self.settings)
                .into_iter()
                .collect(),
            Command::SetNickname {
                target: NicknameTarget::Profile(id),
                nickname,
            } => {
                self.set_nickname(id, nickname);
                Vec::new()
            }
            Command::MoveProfile { id, to } => {
                self.move_profile(&id, to);
                Vec::new()
            }
            Command::SignInAgain(id) => {
                let (profiles, ctx) = self.profiles();
                profiles.sign_in_again(&ctx, &id);
                Vec::new()
            }
            Command::AskRemoveProfile(id) => {
                let (profiles, ctx) = self.profiles();
                let confirm = Command::RemoveProfile(id.clone());
                profiles
                    .removal_confirmation(&ctx, &id, confirm)
                    .into_iter()
                    .collect()
            }
            Command::RemoveProfile(id) => {
                self.remove_profile(&id);
                Vec::new()
            }
            Command::AskRemoveProfileWithNextCli { id, next } => {
                let (profiles, ctx) = self.profiles();
                let confirm = Command::RemoveProfileWithNextCli {
                    id: id.clone(),
                    next,
                };
                profiles
                    .removal_confirmation(&ctx, &id, confirm)
                    .into_iter()
                    .collect()
            }
            Command::RemoveProfileWithNextCli { id, next } => {
                if self.point_cli_elsewhere(next.as_deref()) {
                    self.remove_profile(&id);
                }
                Vec::new()
            }
            Command::OpenAllSessions => {
                let watch = self.sessions.open(&self.env, &self.paths);
                let mut effects = vec![Effect::ShowAllSessions];
                effects.extend(watch.map(Effect::WatchSessionFiles));
                effects
            }
            Command::SearchSessions(text) => {
                self.sessions.set_search(text);
                Vec::new()
            }
            Command::SetSidebarSync(mode) => {
                self.set_sidebar_sync(mode);
                Vec::new()
            }
            Command::AskUninstall => vec![Effect::ConfirmUninstall {
                title: uninstaller::TITLE.into(),
                message: uninstaller::confirmation(self.profiles.folders().len()),
                confirm: Command::Uninstall,
            }],
            Command::Uninstall => self.uninstall(),
            Command::SyncSidebarNow => {
                self.sync_sidebar();
                Vec::new()
            }
            Command::SetIncludeSshAndNoFolderSessions(on) => {
                let mut updated = self.settings.clone();
                updated.leave_out_ssh_and_no_folder_sessions = !on;
                if self.save_settings(updated) {
                    self.log(
                        Level::Info,
                        "sidebar sync ssh and no-folder sessions",
                        &[("on", Field::Word(if on { "on" } else { "off" }))],
                    );
                }
                Vec::new()
            }
            Command::FilterSessions(filter) => {
                self.sessions.set_filter(filter);
                Vec::new()
            }
            Command::SelectSession(id) => {
                self.sessions.select(id);
                Vec::new()
            }
            Command::CloseAllSessions => {
                if let Err(error) = self.sessions.close(&self.paths) {
                    let error = error.to_string();
                    self.log(
                        Level::Warn,
                        "session index not saved",
                        &[("error", Field::Text(&error))],
                    );
                }
                vec![Effect::StopWatchingSessionFiles]
            }
            Command::OpenSession(id) => self.open_session(id),
            Command::ResumeInTerminal(id) => self.open_in_terminal(&id),
            Command::OpenInClaude(id) => self.open_in_claude(&id),
            Command::SwitchClaudeThenOpen { profile, session } => {
                self.switch_claude_then_open(&profile, &session)
            }
            Command::SetMuted { id, muted } => {
                let mut updated = self.settings.clone();
                updated.muted_profiles.retain(|known| *known != id);
                if muted {
                    updated.muted_profiles.push(id);
                }
                self.save_settings(updated);
                Vec::new()
            }
            Command::SetThresholds(values) => {
                let mut updated = self.settings.clone();
                updated.thresholds = Thresholds::new(values);
                self.save_settings(updated);
                Vec::new()
            }
            Command::SetResetNotification(on) => {
                let mut updated = self.settings.clone();
                updated.reset_notification = on;
                self.save_settings(updated);
                Vec::new()
            }
        }
    }

    /// Handles something that happened outside the core.
    pub fn event(&mut self, event: Event) -> Vec<Effect> {
        if self.uninstalled {
            return Vec::new();
        }
        self.locate_claude();
        let mut effects = match event {
            Event::Tick => {
                if self.settings.verbose_logging_until.is_some() && !self.verbose_logging() {
                    self.set_verbose_logging_until(None);
                }
                let was_signing_in = self.profiles.signing_in();
                let mut effects = self.poll_runs();
                if self.uninstalled {
                    return effects;
                }
                if was_signing_in && !self.profiles.signing_in() {
                    self.check_links();
                }
                if !self.self_check_if_versions_changed() {
                    self.settle();
                }
                let signed_in = self.profiles.signed_in();
                let (desktop, ctx) = self.desktop();
                let (polled, cleared) = desktop.poll(&ctx, &signed_in);
                effects.extend(polled);
                effects.extend(self.guide_desktop_switch(cleared));
                effects.extend(self.poll_reopen());
                self.sidebar_watch.notice_switch(&self.env);
                // While a Guided switch waits, copies sync wrote would look
                // like Claude writing the target's folder: sync waits too.
                // So it does while Claude quits and opens again.
                if self.settings.sidebar_sync == SidebarSyncMode::Automatic
                    && !self.desktop.waiting()
                    && !self.reopen.restarting()
                    && self.sidebar_watch.take_due(self.env.clock.now())
                {
                    self.sync_sidebar();
                }
                if let Some(ended) = self.step_sync() {
                    effects.extend(self.continue_desktop_switch(&ended));
                }
                self.index_sessions();
                effects
            }
            Event::MenuOpened => {
                self.rescan_discovered();
                if !self.self_check_if_versions_changed() {
                    self.safety.check_settings_env(&self.env, true);
                    self.check_links();
                    self.settle();
                }
                let watched = self.watched();
                self.monitor.menu_opened(self.env.clock.now(), &watched);
                // While the window is closed nothing is watched: a stat-only
                // rescan keeps the index current.
                if !self.sessions.window_open() {
                    self.sessions.rescan(&self.env, &self.paths);
                }
                Vec::new()
            }
            Event::SessionFilesChanged(changed) => {
                // A late batch after the window closed is left to the next
                // rescan.
                if self.sessions.window_open() {
                    self.sessions
                        .files_changed(&self.env, &self.paths, &changed);
                    self.index_sessions();
                }
                Vec::new()
            }
            Event::SidebarFilesChanged(changed) => {
                self.sidebar_files_changed(&changed);
                Vec::new()
            }
            Event::WillSleep | Event::DidWake => {
                let watched = self.watched();
                self.monitor.set_asleep(event == Event::WillSleep, &watched);
                Vec::new()
            }
            Event::ScreenLocked | Event::ScreenUnlocked => {
                let watched = self.watched();
                self.monitor
                    .set_locked(event == Event::ScreenLocked, &watched);
                Vec::new()
            }
            Event::NetworkChanged { online } => {
                self.monitor.set_online(online);
                Vec::new()
            }
            Event::NotificationActionClicked(NotificationAction::SwitchCli { profile }) => {
                // The notification may predate a relaunch: only a Profile
                // still signed in, and only while CLI switching may change.
                if self.profiles.signed_in().iter().any(|p| p.id == profile) {
                    self.use_for_new_cli_runs(&profile);
                }
                Vec::new()
            }
        };
        effects.extend(self.run_monitor());
        effects.extend(self.take_effects());
        effects
    }

    /// Lets the Limit monitor collect a finished read and start the next.
    /// Finished reads go to the Profile registry, which notices an expired
    /// Login; returns the notifications that produced.
    fn run_monitor(&mut self) -> Vec<Effect> {
        let watched = self.watched();
        let ctx = context!(self);
        // A finished read is handed over before the next starts: a Profile
        // whose `auth status` the registry queues isn't watched meanwhile.
        if let Some(done) = self.monitor.collect_finished(&ctx, &watched) {
            self.profiles.record_reading(&ctx, &done);
        }
        let watched = self.watched();
        if let Some(done) = self.monitor.start_due(&ctx, &watched) {
            self.profiles.record_reading(&ctx, &done);
        }
        let mut effects = self.profiles.take_effects();
        effects.extend(self.notify_limits());
        effects
    }

    /// The limit notifications due now (thresholds crossed, warned limits
    /// that reset).
    fn notify_limits(&mut self) -> Vec<Effect> {
        let ctx = context!(self);
        let watched = self.watched();
        let listed = self.profiles.listed(&self.settings, |id| {
            let active = watched.iter().any(|w| w.id == id && w.active);
            self.monitor.view(&ctx, id, active)
        });
        let headroom = most_headroom(&listed, &watched);
        let pointer = cli_switcher::pointer(&self.paths);
        let subjects: Vec<Subject> = watched
            .iter()
            .filter_map(|w| {
                let listed = listed.iter().find(|listed| listed.id == w.id)?;
                Some(Subject {
                    id: &w.id,
                    name: &listed.short_label,
                    limits: self.monitor.limits(&w.id),
                    tightest: tightest(&listed.row),
                    muted: self.settings.muted_profiles.contains(&w.id),
                    cli_active: pointer.as_deref() == Some(w.config_dir.as_path()),
                })
            })
            .collect();
        let choices = Choices {
            thresholds: self.settings.thresholds.values(),
            reset_notification: self.settings.reset_notification,
        };
        self.notifier
            .check(&ctx, &subjects, headroom.as_ref(), &choices)
    }

    /// The signed-in Profiles the Limit monitor reads, with which are a
    /// client's Active Profile: the CLI's (its pointer) or the desktop app's.
    fn watched(&self) -> Vec<Watched> {
        let cli_active = cli_switcher::pointer(&self.paths);
        let desktop_active = self.desktop_account().and_then(|account| {
            let signed_in = self.profiles.signed_in();
            account
                .profile(&signed_in)
                .map(|profile| profile.id.clone())
        });
        let mut watched = self.profiles.watched(|id, config_dir| {
            cli_active.as_deref() == Some(config_dir) || desktop_active.as_deref() == Some(id)
        });
        // Two Claude Code runs never share a folder.
        watched.retain(|w| !self.openings.queued().any(|(dir, _)| dir == w.config_dir));
        watched
    }

    /// The desktop's account and Organization.
    fn desktop_account(&self) -> Option<DesktopAccount> {
        self.newest_entries
            .desktop_account(&DesktopPaths::new(&self.env.home))
    }

    /// Everything the UI shows right now.
    pub fn view(&self) -> View {
        let login_item = self.env.login_item.status();
        let ctx = self.context();
        let watched = self.watched();
        let mut items: Vec<MenuItem> = self
            .monitor
            .offline_line(&ctx, &watched)
            .into_iter()
            .chain(
                self.warnings
                    .iter()
                    .cloned()
                    .chain(
                        self.claude
                            .missing()
                            .then(|| claude_code::MISSING.to_owned()),
                    )
                    .chain(self.safety.warning_lines())
                    .chain(self.profiles.notice().map(str::to_owned)),
            )
            .map(MenuItem::Warning)
            .collect();
        let mut listed = self.profiles.listed(&self.settings, |id| {
            let active = watched.iter().any(|w| w.id == id && w.active);
            self.monitor.view(&ctx, id, active)
        });
        if let Some(headroom) = most_headroom(&listed, &watched) {
            for listed in listed.iter_mut() {
                listed.row.marker = (listed.id == headroom.id).then(|| MOST_HEADROOM.to_owned());
            }
        }
        for profile in &mut listed {
            self.annotate_links(profile.id, &mut profile.row);
        }
        let profile_settings = self.profile_settings(&listed);
        let cli = cli_switcher::present(&self.paths, &mut listed, self.cli_hold());
        let pointer = cli_switcher::pointer(&self.paths);
        // The menu bar's figure is the CLI's Active Profile's: dimmed with it.
        let menu_bar_dimmed = listed
            .iter()
            .any(|profile| Some(profile.config_dir) == pointer.as_deref() && profile.row.dimmed);
        // The status line goes first: a Guided switch's, else a sync's.
        let mut status = self.desktop.menu_items(self.sync_progress());
        status.extend(self.sync_menu_items());
        items.splice(0..0, status);
        if !items.is_empty() {
            items.push(MenuItem::Separator);
        }
        let signed_in = self.profiles.signed_in();
        let desktop_account = self.desktop_account();
        let desktop_profile = desktop_account
            .as_ref()
            .and_then(|account| account.profile(&signed_in));
        // Each client's Active Profile by its Short label, the CLI's first:
        // without CLI switching that is the Default login, else Default,
        // said when the desktop's is.
        let desktop_part = desktop_account.as_ref().map(|_| {
            let name = desktop_profile
                .and_then(|desktop| listed.iter().find(|listed| listed.id == desktop.id))
                .map_or("another Profile", |listed| listed.short_label.as_str());
            format!("Desktop: {name}")
        });
        // Without CLI switching the CLI uses `~/.claude`: named by its Login.
        let default_login = self
            .discovered
            .iter()
            .find_map(|evidence| Some((&evidence.pair, evidence.default_login()?)))
            .filter(|_| cli.header.is_none());
        let default_login_part = default_login.map(|(pair, names)| {
            let listed = signed_in
                .iter()
                .find(|profile| profile.key == *pair)
                .and_then(|profile| listed.iter_mut().find(|listed| listed.id == profile.id));
            let name = match listed {
                Some(listed) => {
                    listed.row.default_login = true;
                    listed.short_label.clone()
                }
                None => names.name(),
            };
            format!("CLI: {name} (default login)")
        });
        let cli_part = cli
            .header
            .or(default_login_part)
            .or_else(|| desktop_part.as_ref().map(|_| "CLI: Default".to_owned()));
        let header: Vec<String> = cli_part.into_iter().chain(desktop_part).collect();
        if !header.is_empty() {
            items.push(MenuItem::Detail(header.join("    ")));
            items.push(MenuItem::Separator);
        }
        let desktop_installed = self.env.os.desktop_app_version().is_some();
        for listed in listed.iter_mut() {
            let Some(profile) = signed_in.iter().find(|profile| profile.id == listed.id) else {
                continue;
            };
            let row = &mut listed.row;
            let muted = self.settings.muted_profiles.contains(&profile.id);
            let remove = row
                .submenu
                .iter()
                .position(|item| {
                    matches!(item, MenuItem::Action { title, .. } | MenuItem::Submenu { title, .. }
                        if title == "Remove…")
                })
                .unwrap_or(row.submenu.len());
            row.submenu.insert(
                remove,
                MenuItem::Check {
                    title: "Mute notifications".into(),
                    checked: muted,
                    command: Command::SetMuted {
                        id: profile.id.clone(),
                        muted: !muted,
                    },
                },
            );
            if desktop_profile.is_some_and(|desktop| desktop.id == profile.id) {
                row.badges.push(Badge::Desktop);
            } else if desktop_installed {
                row.submenu.push(MenuItem::Action {
                    title: "Switch desktop app to this Profile…".into(),
                    command: Command::SwitchDesktopApp(profile.id.clone()),
                });
            }
        }
        let shown = self.discovered_rows();
        let tz = self.env.clock.time_zone();
        let discovered_rows = shown
            .iter()
            .filter(|(_, choice)| !choice.is_some_and(|c| c.hidden))
            .map(|(evidence, choice)| {
                MenuItem::Discovered(evidence.row(choice.and_then(|c| c.nickname.as_deref()), &tz))
            });
        let before = items.len();
        items.extend(
            listed
                .into_iter()
                .map(|profile| MenuItem::Profile(profile.row)),
        );
        items.extend(discovered_rows);
        if items.len() > before {
            items.push(MenuItem::Separator);
        }
        let recent: Vec<MenuItem> = self
            .sessions
            .recent(RECENT_SESSIONS)
            .into_iter()
            .map(|(id, title)| MenuItem::Action {
                title,
                command: Command::OpenSession(id),
            })
            .collect();
        if !recent.is_empty() {
            items.push(MenuItem::Submenu {
                title: "Recent Sessions".into(),
                items: recent,
            });
        }
        items.push(MenuItem::Action {
            title: "All Sessions…".into(),
            command: Command::OpenAllSessions,
        });
        items.push(if self.profiles.signing_in() {
            MenuItem::Action {
                title: "Waiting for sign-in… Cancel".into(),
                command: Command::CancelSignIn,
            }
        } else {
            MenuItem::Action {
                title: "Add Profile…".into(),
                command: Command::AddProfile,
            }
        });
        items.extend([
            MenuItem::Action {
                title: "Settings…".into(),
                command: Command::OpenSettings,
            },
            MenuItem::Action {
                title: "Diagnostics…".into(),
                command: Command::OpenDiagnostics,
            },
            MenuItem::Action {
                title: "About".into(),
                command: Command::OpenAbout,
            },
            MenuItem::Separator,
            MenuItem::Action {
                title: "Quit".into(),
                command: Command::Quit,
            },
        ]);
        View {
            menu_bar: MenuBarView {
                label: cli.label,
                dimmed: menu_bar_dimmed,
            },
            menu: MenuView { items },
            settings: SettingsView {
                verbose_logging: self.verbose_logging(),
                verbose_logging_note: "Turns itself off after 24 hours.".into(),
                launch_at_login: launch_at_login_view(login_item),
                cli_switching: CliSwitchingView {
                    on: cli_switcher::is_on(&self.paths),
                    line: cli_switcher::path_line(&self.paths),
                    note: self.cli_switching_note(),
                },
                sidebar_sync: SidebarSyncView {
                    mode: self.settings.sidebar_sync,
                    sync_now: (self.settings.sidebar_sync != SidebarSyncMode::Off)
                        .then_some(Command::SyncSidebarNow),
                    note: self.sync_settings_note(),
                    reopen: self.reopen_offered().then_some(Command::ReopenDesktopApp),
                    ssh_and_no_folder_sessions: Toggle {
                        on: !self.settings.leave_out_ssh_and_no_folder_sessions,
                        command: Command::SetIncludeSshAndNoFolderSessions(
                            self.settings.leave_out_ssh_and_no_folder_sessions,
                        ),
                    },
                },
                profiles: profile_settings,
                hidden_discovered: shown
                    .iter()
                    .filter(|(_, choice)| choice.is_some_and(|c| c.hidden))
                    .map(|(evidence, choice)| HiddenDiscovered {
                        title: evidence
                            .hidden_title(choice.and_then(|c| c.nickname.as_deref()), &tz),
                        restore: Command::UnhideDiscovered(evidence.pair.clone()),
                    })
                    .collect(),
                notifications: NotificationsView {
                    thresholds: self.settings.thresholds.values().to_vec(),
                    reset_notification: self.settings.reset_notification,
                },
                uninstall: match &self.uninstalling {
                    Some(_) => UninstallView {
                        note: Some("Uninstalling · signing every Profile out…".into()),
                        retry: None,
                    },
                    None => UninstallView {
                        note: self.uninstall_note.clone(),
                        retry: self.uninstall_note.as_ref().map(|_| Command::Uninstall),
                    },
                },
            },
            about: AboutView {
                name: "AccRoster".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                tagline: TAGLINE.into(),
                disclaimer: DISCLAIMER.into(),
            },
            all_sessions: self.all_sessions(),
            // Offered once: only until answered, and only while it's ours to
            // offer (not already on, not turned off in System Settings).
            first_run: (self.settings.launch_at_login.is_none()
                && login_item == LoginItemStatus::NotRegistered)
                .then_some(FirstRunView {
                    launch_at_login: true,
                }),
            diagnostics: self.diagnostics(),
        }
    }

    /// Settings > Profiles: each Profile in the user's order, with what
    /// reorders, renames and mutes it.
    fn profile_settings(&self, listed: &[Listed<'_>]) -> Vec<ProfileSettings> {
        let last = listed.len().saturating_sub(1);
        listed
            .iter()
            .enumerate()
            .map(|(position, profile)| {
                let id = profile.id.to_owned();
                let move_to = |to: usize| Command::MoveProfile { id: id.clone(), to };
                let muted = self.settings.muted_profiles.contains(&id);
                ProfileSettings {
                    title: profile.row.title.clone(),
                    detail: profile.nicknamed.then(|| profile.name.clone()),
                    move_up: (position > 0).then(|| move_to(position - 1)),
                    move_down: (position < last).then(|| move_to(position + 1)),
                    rename: Command::Rename(NicknameTarget::Profile(id.clone())),
                    mute: profile.signed_in.then(|| Toggle {
                        on: muted,
                        command: Command::SetMuted {
                            id: id.clone(),
                            muted: !muted,
                        },
                    }),
                }
            })
            .collect()
    }

    /// The All Sessions window, with the selected Session's Open section.
    fn all_sessions(&self) -> crate::interface::AllSessionsView {
        let mut view = self
            .sessions
            .view(self.env.clock.now(), &self.sidebar_profiles());
        if let Some(detail) = &mut view.detail
            && let Some(facts) = self.sessions.facts(&detail.id)
        {
            detail.open = opening::choices(&facts, &self.clients());
        }
        view
    }

    /// Reads the next batch of the Session index while a build runs.
    fn index_sessions(&mut self) {
        match self.sessions.step(&self.paths) {
            None => {}
            Some(Ok(built)) => self.log(
                // Menu opens rescan often; only a build that read is news.
                if built.read > 0 {
                    Level::Info
                } else {
                    Level::Debug
                },
                "session index built",
                &[
                    ("transcripts", Field::Number(built.transcripts as i64)),
                    ("read", Field::Number(built.read as i64)),
                ],
            ),
            Some(Err(error)) => {
                let error = error.to_string();
                self.log(
                    Level::Warn,
                    "session index not saved",
                    &[("error", Field::Text(&error))],
                );
            }
        }
    }

    /// The Sidebars All Sessions names: signed-in Profiles in the user's
    /// order (by nickname when set), then Discovered Profiles.
    fn sidebar_profiles(&self) -> Vec<SidebarProfile> {
        let signed_in = self.profiles.signed_in();
        let mut sidebars: Vec<SidebarProfile> = self
            .profiles
            .ordered_ids(&self.settings.profile_order)
            .into_iter()
            .filter_map(|id| {
                let profile = signed_in.iter().find(|profile| profile.id == id)?;
                Some(SidebarProfile {
                    pair: profile.key.clone(),
                    name: (self.settings.nicknames.get(&id).cloned())
                        .unwrap_or_else(|| profile.name.clone()),
                    signed_in: true,
                })
            })
            .collect();
        for (evidence, choice) in self.shown_discovered() {
            if !sidebars.iter().any(|sidebar| sidebar.pair == evidence.pair) {
                sidebars.push(SidebarProfile {
                    pair: evidence.pair.clone(),
                    name: evidence.hidden_title(
                        choice.and_then(|c| c.nickname.as_deref()),
                        &self.env.clock.time_zone(),
                    ),
                    signed_in: false,
                });
            }
        }
        sidebars
    }

    /// Re-reads the Mac's evidence of Discovered Profiles. New pairs simply
    /// appear and emptied ones disappear.
    fn rescan_discovered(&mut self) {
        let found = self.discovery.scan(&self.env.home);
        if found.len() != self.discovered.len() {
            self.log(
                Level::Debug,
                "discovered profiles",
                &[("count", Field::Number(found.len() as i64))],
            );
        }
        self.discovered = found;
    }

    /// Discovered pairs without a signed-in Profile, with the user's choices.
    /// The Discovered rows (shown or hidden): [`Core::shown_discovered`]
    /// without the Sidebar folders of an account that has a signed-in
    /// Profile. Those are the same Login in another Organization, and Sidebar
    /// sync copies entries into them, so they look active. Today's
    /// `~/.claude.json` login stays unless its exact pair is signed in.
    fn discovered_rows(&self) -> Vec<(&Evidence, Option<&DiscoveredChoice>)> {
        let signed_in = self.profiles.signed_in();
        self.shown_discovered()
            .into_iter()
            .filter(|(evidence, _)| {
                evidence.default_login().is_some()
                    || !signed_in
                        .iter()
                        .any(|profile| profile.key.account_uuid == evidence.pair.account_uuid)
            })
            .collect()
    }

    fn shown_discovered(&self) -> Vec<(&Evidence, Option<&DiscoveredChoice>)> {
        self.discovered
            .iter()
            .filter(|evidence| !self.profiles.has_signed_in(&evidence.pair))
            .map(|evidence| (evidence, self.choice(&evidence.pair)))
            .collect()
    }

    fn choice(&self, pair: &ProfileKey) -> Option<&DiscoveredChoice> {
        self.settings
            .discovered_profiles
            .iter()
            .find(|choice| choice.is(pair))
    }

    /// A Discovered Profile that became a Profile (the exact pair signed
    /// in) hands its nickname over, unless the Profile already has one.
    fn carry_over_nicknames(&mut self) {
        let signed_in = self.profiles.signed_in();
        let mut updated = self.settings.clone();
        for choice in &mut updated.discovered_profiles {
            let Some(profile) = signed_in.iter().find(|profile| choice.is(&profile.key)) else {
                continue;
            };
            if let Some(nickname) = choice.nickname.take() {
                updated
                    .nicknames
                    .entry(profile.id.clone())
                    .or_insert(nickname);
            }
        }
        updated
            .discovered_profiles
            .retain(|choice| choice.hidden || choice.nickname.is_some());
        if updated != self.settings && self.save_settings(updated) {
            self.log(Level::Info, "discovered nickname carried over", &[]);
        }
    }

    /// Changes the saved choice for `pair` (creating it), dropping choices
    /// that no longer say anything.
    fn update_choice(&mut self, pair: &ProfileKey, change: impl FnOnce(&mut DiscoveredChoice)) {
        let mut updated = self.settings.clone();
        let choices = &mut updated.discovered_profiles;
        let index = match choices.iter().position(|choice| choice.is(pair)) {
            Some(index) => index,
            None => {
                choices.push(DiscoveredChoice {
                    account_uuid: pair.account_uuid.clone(),
                    organization_uuid: pair.organization_uuid.clone(),
                    nickname: None,
                    hidden: false,
                });
                choices.len() - 1
            }
        };
        change(&mut choices[index]);
        choices.retain(|choice| choice.hidden || choice.nickname.is_some());
        self.save_settings(updated);
    }

    fn verbose_logging(&self) -> bool {
        verbose_logging(&self.settings, &self.env)
    }

    fn set_verbose_logging_until(&mut self, until: Option<jiff::Timestamp>) {
        let mut updated = self.settings.clone();
        updated.verbose_logging_until = until;
        if !self.save_settings(updated) {
            return;
        }
        let state = if until.is_some() { "on" } else { "off" };
        self.log(
            Level::Info,
            "verbose logging",
            &[("state", Field::Word(state))],
        );
    }

    /// Sets or (when blank) clears a Profile's nickname.
    fn set_nickname(&mut self, id: String, nickname: Option<String>) {
        let mut updated = self.settings.clone();
        match nickname
            .as_deref()
            .map(str::trim)
            .filter(|name| !name.is_empty())
        {
            Some(name) => updated.nicknames.insert(id.clone(), name.to_owned()),
            None => updated.nicknames.remove(&id),
        };
        if self.save_settings(updated) {
            self.log(
                Level::Info,
                "nickname changed",
                &[("profile", Field::Profile(&id))],
            );
        }
    }

    /// Remove… confirmed: starts signing the Profile out; once that worked,
    /// its folder is deleted ([`Core::forget_removed`] follows).
    fn remove_profile(&mut self, id: &str) {
        let (profiles, ctx) = self.profiles();
        profiles.remove(&ctx, id);
    }

    /// A removed Profile: forgets its readings, fired notifications,
    /// nickname, mute and place in the order.
    fn forget_removed(&mut self, id: &str) {
        let ctx = context!(self);
        self.monitor.forget(&ctx, id);
        self.notifier.forget(&ctx, id);
        self.forget_profile_settings(id);
    }

    /// Drops a removed Profile's nickname and place in the order.
    fn forget_profile_settings(&mut self, id: &str) {
        let mut updated = self.settings.clone();
        updated.nicknames.remove(id);
        updated.muted_profiles.retain(|known| known != id);
        updated.profile_order.retain(|known| known != id);
        if updated != self.settings {
            self.save_settings(updated);
        }
    }

    /// Moves a Profile to position `to` (clamped) and saves the whole order.
    fn move_profile(&mut self, id: &str, to: usize) {
        let mut order = self.profiles.ordered_ids(&self.settings.profile_order);
        let Some(from) = order.iter().position(|known| known == id) else {
            return;
        };
        let moved = order.remove(from);
        order.insert(to.min(order.len()), moved);
        let mut updated = self.settings.clone();
        updated.profile_order = order;
        if self.save_settings(updated) {
            self.log(
                Level::Info,
                "profile moved",
                &[
                    ("profile", Field::Profile(id)),
                    ("to", Field::Number(to as i64)),
                ],
            );
        }
    }

    /// Writes `settings.json` and keeps `updated` as the current settings.
    /// On failure, logs it, keeps the previous settings and returns `false`.
    fn save_settings(&mut self, updated: Settings) -> bool {
        if let Err(error) = storage::write_versioned(&self.paths.settings(), &updated) {
            let error = error.to_string();
            self.log(
                Level::Error,
                "settings not saved",
                &[("error", Field::Text(&error))],
            );
            return false;
        }
        self.settings = updated;
        true
    }

    /// Records the choice, then acts on it. Once the user turned the app off
    /// in System Settings, only they can turn it back on there: the app opens
    /// that pane instead of registering again.
    fn set_launch_at_login(&mut self, on: bool) -> Vec<Effect> {
        let mut updated = self.settings.clone();
        updated.launch_at_login = Some(on);
        self.save_settings(updated);
        let status = self.env.login_item.status();
        let (action, result) = match (on, status) {
            (true, LoginItemStatus::RequiresApproval) => {
                self.log(Level::Info, "login item left to System Settings", &[]);
                return vec![Effect::OpenLoginItemsSettings];
            }
            (true, LoginItemStatus::Enabled) | (false, LoginItemStatus::NotRegistered) => {
                return Vec::new();
            }
            (true, _) => ("register", self.env.login_item.register()),
            (false, _) => ("unregister", self.env.login_item.unregister()),
        };
        match result {
            Ok(()) => self.log(
                Level::Info,
                "login item changed",
                &[("action", Field::Word(action))],
            ),
            Err(error) => self.log(
                Level::Warn,
                "login item not changed",
                &[
                    ("action", Field::Word(action)),
                    ("error", Field::Text(&error)),
                ],
            ),
        }
        Vec::new()
    }

    /// Looks for `claude` again (cheap once found).
    fn locate_claude(&mut self) {
        let verbose = self.verbose_logging();
        self.claude
            .update(&self.env, &self.paths, &self.logger, verbose);
    }

    /// The Desktop bridge, with what it needs from the rest of the core.
    fn desktop(&mut self) -> (&mut DesktopBridge, profiles::Context<'_>) {
        (&mut self.desktop, context!(self))
    }

    /// Reopen Claude, with what it needs from the rest of the core.
    fn reopen(&mut self) -> (&mut Reopen, profiles::Context<'_>) {
        (&mut self.reopen, context!(self))
    }

    /// The Profile registry, with what it needs from the rest of the core.
    fn profiles(&mut self) -> (&mut ProfileRegistry, profiles::Context<'_>) {
        (&mut self.profiles, context!(self))
    }

    /// What the registry and the monitor need from the rest of the core.
    fn context(&self) -> profiles::Context<'_> {
        context!(self)
    }

    fn log(&self, level: Level, message: &'static str, fields: &[(&'static str, Field<'_>)]) {
        self.logger.write(
            self.env.clock.now(),
            self.verbose_logging(),
            level,
            message,
            fields,
        );
    }
}

/// A row's tightest limit: its highest bar, unknown limit kinds included.
fn tightest(row: &ProfileRow) -> Option<u32> {
    row.limits.iter().map(|bar| bar.percent).max()
}

/// The non-stale watched Profile with the lowest tightest limit, among two
/// or more.
fn most_headroom(listed: &[Listed<'_>], watched: &[Watched]) -> Option<Headroom> {
    notifications::most_headroom(listed.iter().filter_map(|listed| {
        if listed.row.dimmed || !watched.iter().any(|w| w.id == listed.id) {
            return None;
        }
        Some((
            listed.id,
            listed.short_label.as_str(),
            tightest(&listed.row)?,
        ))
    }))
}

/// The Settings toggle shows what macOS has now, not what the app last asked.
fn launch_at_login_view(status: LoginItemStatus) -> LaunchAtLoginView {
    match status {
        LoginItemStatus::Enabled => LaunchAtLoginView {
            on: true,
            enabled: true,
            note: None,
        },
        LoginItemStatus::NotRegistered => LaunchAtLoginView {
            on: false,
            enabled: true,
            note: None,
        },
        LoginItemStatus::RequiresApproval => LaunchAtLoginView {
            on: false,
            enabled: true,
            note: Some(
                "Turned off in System Settings → Login Items. Turning it on opens System Settings."
                    .into(),
            ),
        },
        LoginItemStatus::NotFound => LaunchAtLoginView {
            on: false,
            enabled: false,
            note: Some(
                "Launch at login needs a copy of AccRoster signed with an Apple Developer ID, \
                 like the downloadable releases. This copy can't offer it."
                    .into(),
            ),
        },
    }
}
