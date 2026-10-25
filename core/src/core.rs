//! [`Core`]: the one object the UI layer talks to.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::SignedDuration;

use crate::backups;
use crate::checks;
use crate::cli_switcher::{self, TurnedOff};
use crate::desktop::{self, DesktopBridge, DesktopPaths, Target};
use crate::discovered::{self, Evidence};
use crate::env::{Environment, LoginItemStatus};
use crate::interface::{
    AboutView, CheckStatus, ClaudeVersions, CliSwitchingView, Command, DiscoveredPair, Effect,
    Event, FirstRunView, HiddenDiscovered, LaunchAtLoginView, MenuBarView, MenuItem, MenuView,
    NicknameTarget, NotificationsView, ProfileRow, SafetyFeature, SettingsView, SidebarSyncMode,
    SidebarSyncView, UninstallView, View,
};
use crate::logging::{Field, Level, Logger};
use crate::monitor::{LimitMonitor, LimitsView, Watched};
use crate::notifications::{self, Choices, Headroom, Notifier, Subject};
use crate::opening::{self, Clients, DefaultOpen, DesktopClient, InClaude, NamedProfile};
use crate::profiles::{self, CrossCheckAnswer, Listed, ProfileRegistry, SignInTarget};
use crate::runner::{Ended, JobKind, Jobs, Runner};
use crate::safety::{self, CheckId, Finding, Safety};
use crate::sessions::{SessionFacts, SessionIndex, SidebarProfile};
use crate::settings::{self, DiscoveredChoice, Settings, Thresholds};
use crate::sidebar_sync::{self, Run};
use crate::storage::{self, AppPaths};
use crate::uninstaller;

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
    /// Discovered pairs as of the last scan (launch, menu open).
    discovered: Vec<Evidence>,
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
    /// The sidebars' watch while Sidebar sync is on.
    sidebar_watch: sidebar_sync::Watch,
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
}

/// What the last link check found in one Profile folder.
struct ProfileLinks {
    id: String,
    /// Links whose place holds something else.
    replaced: Vec<&'static str>,
    /// Whether every shared-Transcript link is in place.
    transcripts_shared: bool,
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
        )
    };
}

fn context_of<'a>(
    env: &'a Environment,
    paths: &'a AppPaths,
    logger: &'a Logger,
    settings: &Settings,
    safety: &Safety,
) -> profiles::Context<'a> {
    profiles::Context {
        env,
        paths,
        logger,
        verbose: verbose_logging(settings, env),
        paused: safety.paused(),
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

/// When a self-check runs Claude Code's `auth status` in every Profile.
#[derive(Clone, Copy, PartialEq, Eq)]
enum AuthCheck {
    /// Only when the Claude versions changed since the last self-check:
    /// a short-lived `claude` can start a token refresh and exit before
    /// saving it, so the cross-check stays rare.
    IfVersionsChanged,
    Always,
}

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
            discovered: Vec::new(),
            safety: Safety::load(&paths_for_safety),
            versions: ClaudeVersions::default(),
            links: Vec::new(),
            sessions: SessionIndex::default(),
            sync_note: None,
            sidebar_watch: sidebar_sync::Watch::default(),
            openings: Jobs::default(),
            resume_waiting: None,
            uninstalling: None,
            uninstall_note: None,
            uninstalled: false,
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
        if self.sessions.busy() {
            // The next batch of the Session index, as soon as input allows.
            return Duration::ZERO;
        }
        if self.profiles.busy() || !self.openings.is_empty() {
            // Its end shows at once.
            return RUN_TICK;
        }
        let idle = if self.profiles.signing_in() || self.desktop.waiting() {
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
                let Some(target) = target else {
                    return Vec::new();
                };
                let (desktop, ctx) = self.desktop();
                let cleared = desktop.start(&ctx, target);
                self.guide_desktop_switch(cleared)
            }
            Command::ConfirmDesktopSwitch => {
                let cleared = self.desktop.confirm();
                self.guide_desktop_switch(cleared)
            }
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
                effects.extend(desktop.poll(&ctx, &signed_in));
                self.sidebar_watch.notice_switch(&self.env);
                // While a Guided switch waits, copies sync wrote would look
                // like Claude writing the target's folder: sync waits too.
                if self.settings.sidebar_sync == SidebarSyncMode::Automatic
                    && !self.desktop.waiting()
                    && self.sidebar_watch.take_due(self.env.clock.now())
                {
                    self.sync_sidebar();
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
        let desktop_active =
            desktop::desktop_account(&DesktopPaths::new(&self.env.home)).and_then(|account| {
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
                listed.row.most_headroom = listed.id == headroom.id;
            }
        }
        for profile in &mut listed {
            self.annotate_links(profile.id, &mut profile.row);
        }
        let cli = cli_switcher::present(
            &self.paths,
            &mut listed,
            self.safety.is_paused(SafetyFeature::CliSwitching),
        );
        let pointer = cli_switcher::pointer(&self.paths);
        // The menu bar's figure is the CLI's Active Profile's: dimmed with it.
        let menu_bar_dimmed = listed
            .iter()
            .any(|profile| Some(profile.config_dir) == pointer.as_deref() && profile.row.dimmed);
        items.extend(self.desktop.menu_items());
        if !items.is_empty() {
            items.push(MenuItem::Separator);
        }
        let signed_in = self.profiles.signed_in();
        let desktop_account = desktop::desktop_account(&DesktopPaths::new(&self.env.home));
        let desktop_profile = desktop_account
            .as_ref()
            .and_then(|account| account.profile(&signed_in));
        // Each client's Active Profile by its short name, the CLI's first:
        // without CLI switching that is Default, said when the desktop's is.
        let desktop_part = desktop_account.as_ref().map(|_| {
            let name = desktop_profile
                .and_then(|desktop| listed.iter().find(|listed| listed.id == desktop.id))
                .map_or("another Profile", |listed| listed.short_label.as_str());
            format!("Desktop: {name}")
        });
        let cli_part = cli
            .header
            .or_else(|| desktop_part.as_ref().map(|_| "CLI: Default".to_owned()));
        let header: Vec<String> = cli_part.into_iter().chain(desktop_part).collect();
        if !header.is_empty() {
            items.push(MenuItem::Detail(header.join("    ")));
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
                row.badges.push("Desktop".into());
            } else if desktop_installed {
                row.submenu.push(MenuItem::Action {
                    title: "Switch desktop app to this Profile…".into(),
                    command: Command::SwitchDesktopApp(profile.id.clone()),
                });
            }
        }
        let shown = self.shown_discovered();
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
                label: cli.label.unwrap_or_else(|| "AccRoster".into()),
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
                    note: self.cli_note.clone(),
                },
                sidebar_sync: SidebarSyncView {
                    mode: self.settings.sidebar_sync,
                    sync_now: (self.settings.sidebar_sync != SidebarSyncMode::Off)
                        .then_some(Command::SyncSidebarNow),
                    note: self.sync_note.clone(),
                },
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

    /// Who could open a Session right now: the CLI's and the desktop's
    /// Active Profiles, and the signed-in Profiles.
    fn clients(&self) -> Clients {
        let signed_in = self.profiles.signed_in();
        let profiles: Vec<NamedProfile> = self
            .profiles
            .ordered_ids(&self.settings.profile_order)
            .into_iter()
            .filter_map(|id| {
                let profile = signed_in.iter().find(|profile| profile.id == id)?;
                Some(NamedProfile {
                    name: (self.settings.nicknames.get(&id).cloned())
                        .unwrap_or_else(|| profile.name.clone()),
                    pair: DiscoveredPair {
                        account_uuid: profile.account_uuid.clone(),
                        organization_uuid: profile.organization_uuid.clone(),
                    },
                    id,
                })
            })
            .collect();
        let desktop = self.env.os.desktop_app_version().map(|_| {
            let account = desktop::desktop_account(&DesktopPaths::new(&self.env.home));
            let pair = account
                .as_ref()
                .and_then(|account| match account.profile(&signed_in) {
                    Some(profile) => Some(DiscoveredPair {
                        account_uuid: profile.account_uuid.clone(),
                        organization_uuid: profile.organization_uuid.clone(),
                    }),
                    None => Some(DiscoveredPair {
                        account_uuid: account.account_uuid.clone(),
                        organization_uuid: account.organization_uuid.clone()?,
                    }),
                });
            let name = pair.as_ref().and_then(|pair| {
                self.sidebar_profiles()
                    .into_iter()
                    .find(|sidebar| sidebar.pair == *pair)
                    .map(|sidebar| sidebar.name)
            });
            DesktopClient { pair, name }
        });
        let desktop_turn_running = desktop.is_some()
            && matches!(
                desktop::guard(&self.env, &DesktopPaths::new(&self.env.home)),
                desktop::Guard::Busy(_)
            );
        Clients {
            cli: self.cli_profile_name(),
            desktop,
            profiles,
            desktop_turn_running,
            can_run_claude_code: self.claude_code_folder().is_some(),
        }
    }

    /// The CLI's pointer while CLI switching is on.
    fn cli_pointer(&self) -> Option<PathBuf> {
        cli_switcher::is_on(&self.paths)
            .then(|| cli_switcher::pointer(&self.paths))
            .flatten()
    }

    /// The CLI's Active Profile as the menu header names it: the pointer's
    /// Profile, or Default (`~/.claude`) without one.
    fn cli_profile_name(&self) -> String {
        let Some(pointer) = self.cli_pointer() else {
            return "Default".into();
        };
        self.profiles
            .listed(&self.settings, |_| LimitsView::default())
            .into_iter()
            .find(|profile| profile.config_dir == pointer)
            .map_or_else(
                || "an unknown Profile".into(),
                |profile| profile.short_label,
            )
    }

    /// Where `claude --desktop --resume` runs (any subscription login will
    /// do, never `~/.claude`): the CLI's Active Profile, else the desktop's,
    /// else the first signed-in Profile.
    fn claude_code_folder(&self) -> Option<PathBuf> {
        let ordered = self.profiles.ordered_ids(&self.settings.profile_order);
        let folders: Vec<(&str, &Path)> = ordered
            .iter()
            .filter_map(|id| Some((id.as_str(), self.profiles.config_dir(id)?)))
            .collect();
        let cli = self
            .cli_pointer()
            .filter(|pointer| folders.iter().any(|(_, dir)| *dir == pointer.as_path()));
        let desktop = || {
            let account = desktop::desktop_account(&DesktopPaths::new(&self.env.home))?;
            let profile = account.profile(&self.profiles.signed_in())?.id.clone();
            let (_, dir) = folders.iter().find(|(id, _)| *id == profile)?;
            Some(dir.to_path_buf())
        };
        let first = || folders.first().map(|(_, dir)| dir.to_path_buf());
        cli.or_else(desktop).or_else(first)
    }

    /// A double-click or a Recent Sessions item: the client the Session was
    /// last used in. When Claude's Active Profile has no entry for it, the
    /// Session is shown in All Sessions, where the choices are spelled out.
    fn open_session(&mut self, id: String) -> Vec<Effect> {
        let Some(facts) = self.sessions.facts(&id) else {
            return Vec::new();
        };
        match opening::default_open(&facts, &self.clients()) {
            None => Vec::new(),
            Some(DefaultOpen::Terminal) => self.open_in_terminal(&id),
            Some(DefaultOpen::Claude) => self.open_in_claude(&id),
            Some(DefaultOpen::Choose) => {
                let effects = self.handle(Command::OpenAllSessions);
                self.sessions.set_filter(Default::default());
                self.sessions.set_search(String::new());
                self.sessions.select(Some(id));
                effects
            }
        }
    }

    /// Resume in Terminal, or Show in Terminal when it already runs there.
    fn open_in_terminal(&mut self, id: &str) -> Vec<Effect> {
        let Some(effect) = self
            .sessions
            .facts(id)
            .and_then(|facts| opening::terminal(&facts, &*self.env.os))
        else {
            return Vec::new();
        };
        let how = match effect {
            Effect::ShowInTerminal { .. } => "show_in_terminal",
            _ => "resume_in_terminal",
        };
        self.log_opened(id, how);
        vec![effect]
    }

    /// Open in Claude: its entry in the desktop's Active Profile, else
    /// `claude --desktop --resume`.
    fn open_in_claude(&mut self, id: &str) -> Vec<Effect> {
        let Some(facts) = self.sessions.facts(id) else {
            return Vec::new();
        };
        let clients = self.clients();
        match opening::in_claude(&facts, &clients) {
            None => Vec::new(),
            Some(InClaude::Entry(entry)) => {
                self.log_opened(id, "claude_entry");
                vec![Effect::OpenUrl(opening::continue_url(&entry))]
            }
            Some(InClaude::Resume) => {
                if opening::resume_blocked(&facts, &clients).is_some() {
                    return Vec::new();
                }
                self.resume_in_claude(&facts)
            }
        }
    }

    /// Starts `claude --desktop --resume <id>` in a Profile folder; its end
    /// is collected by [`Core::poll_openings`].
    fn resume_in_claude(&mut self, facts: &SessionFacts) -> Vec<Effect> {
        let Some(folder) = self.claude_code_folder() else {
            return Vec::new();
        };
        let kind = JobKind::OpenInDesktop {
            session_id: facts.id.clone(),
            session_folder: facts.cwd.clone(),
        };
        self.openings.push(&folder, kind, facts.id.clone());
        Vec::new()
    }

    /// Collects the `--desktop --resume` runs that ended; a failed or
    /// timed-out hand-over is notified.
    fn poll_openings(&mut self) -> Vec<Effect> {
        let ended = self.openings.poll(&Runner::new(&self.env, &self.paths));
        let mut effects = Vec::new();
        for Ended {
            tag: id, finished, ..
        } in ended
        {
            if finished.succeeded() {
                self.log_opened(&id, "claude_resume");
                continue;
            }
            self.log(
                Level::Warn,
                "session not opened in claude",
                &[
                    ("session", Field::SessionId(&id)),
                    ("error", Field::Word(finished.word())),
                ],
            );
            effects.push(Effect::Notify {
                title: "Couldn't open the Session in Claude".into(),
                body: "Claude Code couldn't hand it over. Check that Claude is signed in, then try again."
                    .into(),
            });
        }
        effects
    }

    /// "Switch Claude to <Profile>, then open…": a Guided switch that opens
    /// the Profile's entry once confirmed.
    fn switch_claude_then_open(&mut self, profile_id: &str, session: &str) -> Vec<Effect> {
        let Some(facts) = self.sessions.facts(session) else {
            return Vec::new();
        };
        let Some(profile) = self
            .profiles
            .signed_in()
            .into_iter()
            .find(|profile| profile.id == profile_id)
        else {
            return Vec::new();
        };
        let pair = DiscoveredPair {
            account_uuid: profile.account_uuid.clone(),
            organization_uuid: profile.organization_uuid.clone(),
        };
        let Some(entry) = opening::switch_entry(&facts, &pair) else {
            return Vec::new();
        };
        let target = Target {
            then_open: Some(entry.entry_id.clone()),
            ..Target::of(&profile)
        };
        let (desktop, ctx) = self.desktop();
        let cleared = desktop.start(&ctx, target);
        self.guide_desktop_switch(cleared)
    }

    fn log_opened(&self, id: &str, how: &'static str) {
        self.log(
            Level::Info,
            "session opened",
            &[("session", Field::SessionId(id)), ("how", Field::Word(how))],
        );
    }

    /// The Diagnostics window: every check, the versions, the pauses.
    fn diagnostics(&self) -> crate::interface::DiagnosticsView {
        let usage = self.monitor.usage_summary();
        let mut finding = if usage.unreadable > 0 {
            Finding::fail(format!(
                "{} of {} last answers couldn't be read",
                usage.unreadable,
                usage.unreadable + usage.read
            ))
        } else if usage.read > 0 {
            Finding::pass()
        } else {
            Finding::never_seen()
        };
        if !usage.new_kinds.is_empty() {
            let kinds: Vec<String> = usage
                .new_kinds
                .iter()
                .map(|kind| safety::reportable(kind))
                .collect();
            let line = format!("New limit kind seen: {}", kinds.join(", "));
            finding.detail = Some(match finding.detail.take() {
                Some(detail) => format!("{detail}; {line}"),
                None => line,
            });
        }
        self.safety.diagnostics(self.versions.clone(), finding)
    }

    /// "Transcripts not shared" under a Profile whose shared-Transcript links
    /// aren't in place, and a detail line per other replaced link.
    fn annotate_links(&self, id: &str, row: &mut crate::interface::ProfileRow) {
        let Some(links) = self.links.iter().find(|links| links.id == id) else {
            return;
        };
        if !links.transcripts_shared {
            row.note = Some(match row.note.take() {
                Some(note) => format!("{note} · Transcripts not shared"),
                None => "Transcripts not shared".to_owned(),
            });
        }
        for name in links
            .replaced
            .iter()
            .filter(|name| !cli_switcher::TRANSCRIPT_LINKS.contains(name))
        {
            row.submenu.push(MenuItem::Detail(format!(
                "{name} link replaced · not shared"
            )));
        }
    }

    /// Runs the full self-check when a Claude version changed since the last
    /// one. Returns whether it ran.
    fn self_check_if_versions_changed(&mut self) -> bool {
        if !self.safety.versions_changed(&desktop::versions(&self.env)) {
            return false;
        }
        self.log(Level::Info, "claude versions changed", &[]);
        self.self_check(AuthCheck::IfVersionsChanged);
        true
    }

    /// The full self-check: settings `env`, accounts, links, Sidebar entries,
    /// live-process files and Transcript records, plus `auth status` per
    /// `auth` (recorded once its answers are in); then pauses whatever a
    /// failed check blocks.
    fn self_check(&mut self, auth: AuthCheck) {
        let versions = desktop::versions(&self.env);
        let run_auth = auth == AuthCheck::Always || self.safety.versions_changed(&versions);
        self.versions = versions;
        self.safety.check_settings_env(&self.env, false);
        self.check_accounts();
        self.check_links();
        if run_auth {
            self.check_config_dir_honoured();
        }
        let home = self.env.home.clone();
        self.check_sidebar_entries();
        let mut session_dirs = vec![home.join(".claude")];
        session_dirs.extend(
            self.profiles
                .folders()
                .into_iter()
                .map(|(_, dir)| dir.to_path_buf()),
        );
        self.safety.record(
            CheckId::LiveProcesses,
            checks::live_processes(&session_dirs),
        );
        self.safety.record(
            CheckId::TranscriptRecords,
            checks::transcript_records(&home),
        );
        self.safety.mark_checked(&self.versions);
        self.settle();
    }

    /// Pauses whatever a current finding blocks, and logs each new pause.
    fn settle(&mut self) {
        for feature in self.safety.settle() {
            self.log(
                Level::Warn,
                "safety pause",
                &[("feature", Field::Word(feature.key()))],
            );
        }
    }

    fn check_accounts(&mut self) {
        let folders: Vec<&Path> = self
            .profiles
            .folders()
            .into_iter()
            .map(|(_, dir)| dir)
            .collect();
        let (accounts, caches) =
            checks::accounts(&folders, &safety::claude_code_name(&self.versions));
        self.safety.record(CheckId::OauthAccount, accounts);
        self.safety.record(CheckId::CachedUsage, caches);
    }

    /// Checks every Profile folder's links: a replaced shared-Transcript link
    /// pauses Profile links; other replaced links are only reported.
    fn check_links(&mut self) {
        let claude_home = self.env.home.join(".claude");
        self.links = self
            .profiles
            .folders()
            .into_iter()
            .map(|(id, dir)| ProfileLinks {
                id: id.to_owned(),
                replaced: cli_switcher::replaced_links(dir, &claude_home),
                transcripts_shared: cli_switcher::TRANSCRIPT_LINKS.iter().all(|name| {
                    std::fs::read_link(dir.join(name)).ok() == Some(claude_home.join(name))
                }),
            })
            .collect();
        if self.links.is_empty() {
            return;
        }
        let summary = |transcript: bool| -> Option<String> {
            let mut names: Vec<&str> = Vec::new();
            let mut profiles = 0;
            for links in &self.links {
                let hit: Vec<&str> = links
                    .replaced
                    .iter()
                    .copied()
                    .filter(|name| cli_switcher::TRANSCRIPT_LINKS.contains(name) == transcript)
                    .collect();
                if !hit.is_empty() {
                    profiles += 1;
                }
                for name in hit {
                    if !names.contains(&name) {
                        names.push(name);
                    }
                }
            }
            (profiles > 0).then(|| {
                let plural = if names.len() > 1 { "links" } else { "link" };
                let whose = if profiles > 1 { "Profiles" } else { "Profile" };
                format!(
                    "{} {plural} replaced in {profiles} {whose}",
                    names.join(", ")
                )
            })
        };
        let transcripts = match summary(true) {
            Some(text) => Finding::fail(text.clone()).blocking(SafetyFeature::Links, text),
            None => Finding::pass(),
        };
        let others = match summary(false) {
            Some(text) => Finding::fail(text),
            None => Finding::pass(),
        };
        self.safety.record(CheckId::TranscriptLinks, transcripts);
        self.safety.record(CheckId::OtherLinks, others);
    }

    /// Queues `claude auth status --json` in every signed-in Profile, to
    /// compare with each folder's `oauthAccount` once the answers are in
    /// ([`Core::cross_checked`]). Returns whether anything was queued.
    fn check_config_dir_honoured(&mut self) -> bool {
        let (profiles, ctx) = self.profiles();
        profiles.start_cross_check(&ctx)
    }

    /// Every cross-check answer is in: records what they say.
    fn cross_checked(&mut self, answers: &[CrossCheckAnswer]) {
        if let Some(finding) = checks::config_dir_honoured(&self.versions, answers) {
            self.safety.record(CheckId::ConfigDirHonoured, finding);
        }
        self.settle();
        if let Some(feature) = self.resume_waiting.take() {
            self.finish_resume(feature);
        }
    }

    /// "Resume <feature>": runs the feature's checks again and ends its
    /// pause if none fails. CLI switching's include the `auth status`
    /// cross-check, so it resumes once that is in.
    fn resume(&mut self, feature: SafetyFeature) {
        match feature {
            SafetyFeature::CliSwitching => {
                self.versions = desktop::versions(&self.env);
                self.safety.check_settings_env(&self.env, false);
                self.check_accounts();
                if self.check_config_dir_honoured() {
                    self.resume_waiting = Some(feature);
                    return;
                }
            }
            SafetyFeature::Links => self.check_links(),
            SafetyFeature::Seeding => self.check_accounts(),
            SafetyFeature::SidebarSync => self.check_sidebar_entries(),
        }
        self.finish_resume(feature);
    }

    fn finish_resume(&mut self, feature: SafetyFeature) {
        self.settle();
        if !self.safety.resume(feature) {
            self.log(
                Level::Info,
                "safety pause kept",
                &[("feature", Field::Word(feature.key()))],
            );
            return;
        }
        self.log(
            Level::Info,
            "safety pause resumed",
            &[("feature", Field::Word(feature.key()))],
        );
        if feature == SafetyFeature::Links {
            let (profiles, ctx) = self.profiles();
            profiles.heal_links(&ctx);
            self.check_links();
        }
        if feature == SafetyFeature::SidebarSync {
            // The user accepts the sidebars as they are now.
            sidebar_sync::accept_current(&self.env, &self.paths);
            self.sync_note = None;
        }
    }

    /// The Sidebar entries check. A failure pauses Sidebar sync only while
    /// sync is on: with it off, nothing would be written anyway.
    fn check_sidebar_entries(&mut self) {
        let mut finding = checks::sidebar_entries(&self.env.home);
        if finding.status == CheckStatus::Fail && self.settings.sidebar_sync != SidebarSyncMode::Off
        {
            let reason = format!("{} changed Sidebar entries", desktop_name(&self.versions));
            finding = finding.blocking(SafetyFeature::SidebarSync, reason);
        }
        self.safety.record(CheckId::SidebarEntries, finding);
    }

    /// The Sidebar sync mode in Settings. Turning sync on checks the Sidebar
    /// entries first, so a changed format pauses it before any write.
    fn set_sidebar_sync(&mut self, mode: SidebarSyncMode) {
        let mut updated = self.settings.clone();
        updated.sidebar_sync = mode;
        if !self.save_settings(updated) {
            return;
        }
        self.sync_note = None;
        self.log(
            Level::Info,
            "sidebar sync mode",
            &[(
                "mode",
                Field::Word(match mode {
                    SidebarSyncMode::Off => "off",
                    SidebarSyncMode::Manual => "manual",
                    SidebarSyncMode::Automatic => "automatic",
                }),
            )],
        );
        self.check_sidebar_entries();
        self.settle();
    }

    /// "Sync now": checks the Sidebar entries again, then copies every entry
    /// into every other Profile's sidebar (ADR 0003).
    fn sync_sidebar(&mut self) {
        if self.settings.sidebar_sync == SidebarSyncMode::Off {
            return;
        }
        self.sidebar_watch.ran();
        self.check_sidebar_entries();
        self.settle();
        if self.safety.is_paused(SafetyFeature::SidebarSync) {
            self.sync_note =
                Some("Sidebar sync is paused · see Diagnostics for why and to resume it".into());
            self.log(Level::Info, "sidebar sync not run: paused", &[]);
            return;
        }
        match sidebar_sync::run(&self.env, &self.paths) {
            Run::Synced(report) => {
                self.safety
                    .record(CheckId::MultiAccountMode, Finding::pass());
                self.log(
                    Level::Info,
                    "sidebar synced",
                    &[
                        ("entries", Field::Number(report.written as i64)),
                        ("folders", Field::Number(report.folders as i64)),
                        ("not_backed_up", Field::Number(report.not_backed_up as i64)),
                    ],
                );
                self.sync_note = Some(sync_note(&report));
            }
            Run::DesktopAccountUnknown => {
                self.log(
                    Level::Info,
                    "sidebar sync not run: desktop account unknown",
                    &[],
                );
                self.sync_note =
                    Some("Couldn't tell which Profile Claude is using · nothing was synced".into());
            }
            Run::UnknownFields(fields) => {
                let detail = format!(
                    "unknown fields: {}",
                    fields.into_iter().collect::<Vec<_>>().join(", ")
                );
                let reason = format!("{} changed Sidebar entries", desktop_name(&self.versions));
                self.safety.record(
                    CheckId::SidebarEntries,
                    Finding::fail(detail).blocking(SafetyFeature::SidebarSync, reason),
                );
                self.paused_sync_note();
            }
            Run::FoldersChanged(count) => {
                let whose = if count > 1 { "sidebars" } else { "sidebar" };
                let detail =
                    format!("{count} {whose} Claude wasn't using changed since the last sync");
                self.pause_for_multi_account(detail);
                self.paused_sync_note();
            }
        }
    }

    /// The dormant multi-account mode's trace: a Safety pause of Sidebar
    /// sync. It stays until Resume, which accepts the sidebars as they are;
    /// the finding itself no longer blocks, so Resume shows at once.
    fn pause_for_multi_account(&mut self, detail: String) {
        self.safety.record(
            CheckId::MultiAccountMode,
            Finding::fail(detail.clone()).blocking(
                SafetyFeature::SidebarSync,
                "Claude wrote to a Profile it isn't using",
            ),
        );
        self.settle();
        self.safety
            .record(CheckId::MultiAccountMode, Finding::fail(detail));
    }

    /// The sidebars' watch reported changes: Automatic mode schedules a
    /// run, and an entry Claude wrote outside its Active Profile folder
    /// pauses sync.
    fn sidebar_files_changed(&mut self, changed: &[PathBuf]) {
        let mode = self.settings.sidebar_sync;
        let paused = self.safety.is_paused(SafetyFeature::SidebarSync);
        if mode == SidebarSyncMode::Off {
            return;
        }
        let automatic = mode == SidebarSyncMode::Automatic && !paused;
        let elsewhere =
            self.sidebar_watch
                .files_changed(&self.env, &self.paths, changed, automatic);
        if elsewhere == 0 || paused {
            return;
        }
        let entries = if elsewhere == 1 { "entry" } else { "entries" };
        self.pause_for_multi_account(format!(
            "Claude wrote {elsewhere} Sidebar {entries} outside the Profile it's using"
        ));
        self.log(
            Level::Warn,
            "sidebar sync paused: claude wrote another profile",
            &[("entries", Field::Number(elsewhere as i64))],
        );
        self.sync_note =
            Some("Sidebar sync is paused · Claude wrote to a Profile it isn't using".into());
    }

    fn paused_sync_note(&mut self) {
        self.settle();
        self.log(Level::Warn, "sidebar sync stopped before writing", &[]);
        self.sync_note =
            Some("Sidebar sync is paused · nothing was written · see Diagnostics".into());
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
                    pair: DiscoveredPair {
                        account_uuid: profile.account_uuid.clone(),
                        organization_uuid: profile.organization_uuid.clone(),
                    },
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
        let found = discovered::scan(&self.env.home);
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
    fn shown_discovered(&self) -> Vec<(&Evidence, Option<&DiscoveredChoice>)> {
        self.discovered
            .iter()
            .filter(|evidence| !self.profiles.has_signed_in(&evidence.pair))
            .map(|evidence| (evidence, self.choice(&evidence.pair)))
            .collect()
    }

    fn choice(&self, pair: &DiscoveredPair) -> Option<&DiscoveredChoice> {
        self.settings.discovered_profiles.iter().find(|choice| {
            choice.account_uuid == pair.account_uuid
                && choice.organization_uuid == pair.organization_uuid
        })
    }

    /// A Discovered Profile that became a Profile (the exact pair signed
    /// in) hands its nickname over, unless the Profile already has one.
    fn carry_over_nicknames(&mut self) {
        let signed_in = self.profiles.signed_in();
        let mut updated = self.settings.clone();
        for choice in &mut updated.discovered_profiles {
            let Some(profile) = signed_in.iter().find(|profile| {
                profile.account_uuid == choice.account_uuid
                    && profile.organization_uuid == choice.organization_uuid
            }) else {
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
    fn update_choice(&mut self, pair: &DiscoveredPair, change: impl FnOnce(&mut DiscoveredChoice)) {
        let mut updated = self.settings.clone();
        let choices = &mut updated.discovered_profiles;
        let index = match choices.iter().position(|choice| {
            choice.account_uuid == pair.account_uuid
                && choice.organization_uuid == pair.organization_uuid
        }) {
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

    /// Runs the uninstall's steps in order (see [`uninstaller`]), each
    /// skipped when already done. Stops, with a note and Retry in Settings,
    /// where going on could break `claude` or orphan a login. Steps 3 to 5
    /// wait for step 2's sign-outs ([`Core::continue_uninstall`]).
    fn uninstall(&mut self) -> Vec<Effect> {
        if self.uninstalling.is_some() {
            return Vec::new();
        }
        self.log(Level::Info, "uninstall started", &[]);
        self.uninstall_note = None;
        let mut left_to_user = Vec::new();
        if self.profiles.signing_in() {
            let (profiles, ctx) = self.profiles();
            profiles.cancel(&ctx);
        }
        if self.desktop.waiting() {
            let (desktop, ctx) = self.desktop();
            desktop.cancel(&ctx);
        }

        // 1. Unhook the CLI while every Profile folder still exists.
        self.set_cli_switching(false);
        if cli_switcher::is_on(&self.paths) {
            let why = self.cli_note.take().unwrap_or_default();
            return self.stop_uninstall(format!(
                "{why}\nUninstall stopped before signing anything out. Retry once that is fixed."
            ));
        }
        left_to_user.extend(self.cli_note.take());

        // 2. Remove each Profile: sign out, then delete.
        let ids: Vec<String> = self
            .profiles
            .folders()
            .into_iter()
            .map(|(id, _)| id.to_owned())
            .collect();
        for id in &ids {
            self.remove_profile(id);
        }
        self.profiles.take_unlisted_kept();
        let (profiles, ctx) = self.profiles();
        profiles.remove_unlisted(&ctx);
        self.uninstalling = Some(left_to_user);
        self.continue_uninstall()
    }

    /// Steps 3 to 5 of an uninstall, once every sign-out of step 2 ended.
    fn continue_uninstall(&mut self) -> Vec<Effect> {
        if self.profiles.busy() {
            return Vec::new();
        }
        let Some(mut left_to_user) = self.uninstalling.take() else {
            return Vec::new();
        };
        let mut kept: Vec<String> = self
            .profiles
            .folders()
            .into_iter()
            .filter_map(|(id, _)| self.profiles.name(id))
            .collect();
        let unlisted = self.profiles.take_unlisted_kept();
        if unlisted > 0 {
            kept.push(format!("{unlisted} unlisted sign-in folder(s)"));
        }
        if !kept.is_empty() {
            return self.stop_uninstall(format!(
                "Couldn't sign out {} · kept so the login can still be revoked. CLI switching is already off. Check your connection, then Retry.",
                kept.join(", ")
            ));
        }

        // 3. Unregister launch at login.
        match self.env.login_item.status() {
            LoginItemStatus::NotRegistered | LoginItemStatus::NotFound => {}
            LoginItemStatus::Enabled | LoginItemStatus::RequiresApproval => {
                if let Err(error) = self.env.login_item.unregister() {
                    self.log(
                        Level::Warn,
                        "login item not unregistered",
                        &[("error", Field::Text(&error))],
                    );
                    left_to_user.push(
                        "Launch at login couldn't be turned off: remove AccRoster in System Settings → Login Items.".into(),
                    );
                }
            }
        }

        // 4. Delete the app's folder, log included.
        self.log(Level::Info, "uninstall deleting app folder", &[]);
        if let Err(error) = uninstaller::delete_app_folder(&self.paths) {
            return self.stop_uninstall(format!(
                "Every Profile is signed out and removed, but AccRoster's folder couldn't be deleted ({error}). Retry, or delete it yourself: {}",
                self.paths.root().display()
            ));
        }
        self.uninstalled = true;

        // 5. The UI moves the app to the Trash and quits.
        let mut effects = vec![
            Effect::StopWatchingSessionFiles,
            Effect::StopWatchingSidebarFolders,
        ];
        if !left_to_user.is_empty() {
            effects.push(Effect::Notify {
                title: "AccRoster uninstalled · one thing left to do".into(),
                body: left_to_user.join("\n"),
            });
        }
        effects.push(Effect::TrashAppAndQuit);
        effects
    }

    /// Ends an uninstall early: Settings shows `note` and Retry.
    fn stop_uninstall(&mut self, note: String) -> Vec<Effect> {
        self.log(Level::Warn, "uninstall stopped", &[]);
        self.uninstall_note = Some(note);
        vec![Effect::ShowSettings]
    }

    /// Points new CLI runs at Profile `next`, or at Default (`~/.claude`,
    /// no pointer) for `None`, before the CLI's Active Profile is removed.
    /// Returns whether that worked; the removal waits for it.
    fn point_cli_elsewhere(&mut self, next: Option<&str>) -> bool {
        let result = match next {
            Some(next) => match self.profiles.config_dir(next) {
                Some(config_dir) => cli_switcher::set_pointer(&self.paths, config_dir),
                None => return false,
            },
            None => cli_switcher::clear_pointer(&self.paths),
        };
        match result {
            Ok(()) => {
                self.log(Level::Info, "cli profile moved before removal", &[]);
                true
            }
            Err(error) => {
                let error = error.to_string();
                self.log(
                    Level::Error,
                    "cli profile not moved",
                    &[("error", Field::Text(&error))],
                );
                false
            }
        }
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

    /// The CLI switching switch: on appends the `.zshrc` block and installs
    /// the shim; off removes them and the pointer, or lists the lines of an
    /// edited block for the user to remove.
    fn set_cli_switching(&mut self, on: bool) {
        if on && self.safety.is_paused(SafetyFeature::CliSwitching) {
            self.cli_note =
                Some("CLI switching is paused · see Diagnostics for why and to resume it".into());
            return;
        }
        let now = self.env.clock.now();
        let result = if on {
            cli_switcher::turn_on(&self.paths, &self.env.home, now).map(|()| TurnedOff::Clean)
        } else {
            cli_switcher::turn_off(&self.paths, &self.env.home, now)
        };
        let state = if on { "on" } else { "off" };
        match result {
            Ok(TurnedOff::Clean) => {
                self.cli_note = None;
                self.log(
                    Level::Info,
                    "cli switching changed",
                    &[("state", Field::Word(state))],
                );
            }
            Ok(TurnedOff::EditedBlock(lines)) => {
                self.cli_note = Some(format!(
                    "Your AccRoster block in ~/.zshrc was edited, so it was left in place. Remove these lines yourself:\n{}",
                    lines.join("\n")
                ));
                self.log(
                    Level::Info,
                    "cli switching changed",
                    &[
                        ("state", Field::Word(state)),
                        ("zshrc", Field::Word("edited")),
                    ],
                );
            }
            Err(error) => {
                self.cli_note = Some(format!("Couldn't turn CLI switching {state}: {error}"));
                let error = error.to_string();
                self.log(
                    Level::Error,
                    "cli switching not changed",
                    &[
                        ("state", Field::Word(state)),
                        ("error", Field::Text(&error)),
                    ],
                );
            }
        }
    }

    /// Makes Profile `id` the CLI's Active Profile by rewriting only the
    /// pointer. Ignored while CLI switching is off.
    fn use_for_new_cli_runs(&mut self, id: &str) {
        if !cli_switcher::is_on(&self.paths) {
            return;
        }
        // The shared settings may have changed since the menu opened.
        if self.safety.check_settings_env(&self.env, true) {
            self.settle();
        }
        if self.safety.is_paused(SafetyFeature::CliSwitching) {
            self.log(Level::Info, "cli profile not chosen: paused", &[]);
            return;
        }
        let Some(config_dir) = self.profiles.config_dir(id).map(Path::to_path_buf) else {
            return;
        };
        match cli_switcher::set_pointer(&self.paths, &config_dir) {
            Ok(()) => self.log(
                Level::Info,
                "cli profile chosen",
                &[("profile", Field::Profile(id))],
            ),
            Err(error) => {
                let error = error.to_string();
                self.log(
                    Level::Error,
                    "cli profile not chosen",
                    &[
                        ("profile", Field::Profile(id)),
                        ("error", Field::Text(&error)),
                    ],
                );
            }
        }
    }

    /// The Desktop bridge, with what it needs from the rest of the core.
    /// A Guided switch the guard let through: syncs the sidebars first when
    /// Sidebar sync is on (so the target's sidebar has every Session when
    /// Claude opens it), then brings Claude forward.
    fn guide_desktop_switch(&mut self, cleared: Option<Target>) -> Vec<Effect> {
        let Some(target) = cleared else {
            return Vec::new();
        };
        if self.settings.sidebar_sync != SidebarSyncMode::Off {
            self.sync_sidebar();
        }
        let (desktop, ctx) = self.desktop();
        desktop.guide(&ctx, target)
    }

    fn desktop(&mut self) -> (&mut DesktopBridge, profiles::Context<'_>) {
        (&mut self.desktop, context!(self))
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

/// "Claude 3.6556.2", or "Claude" when the desktop app's version is unknown.
fn desktop_name(versions: &ClaudeVersions) -> String {
    match &versions.desktop {
        Some(version) => format!("Claude {version}"),
        None => "Claude".to_owned(),
    }
}

/// Settings' line after a sync.
fn sync_note(report: &sidebar_sync::Report) -> String {
    let mut note = if report.written == 0 {
        "Every sidebar was already in sync".to_owned()
    } else {
        let entries = if report.written == 1 {
            "entry"
        } else {
            "entries"
        };
        let profiles = if report.folders == 1 {
            "Profile"
        } else {
            "Profiles"
        };
        format!(
            "Updated {} Sidebar {entries} in {} {profiles}",
            report.written, report.folders
        )
    };
    if report.left_desktop_account {
        note.push_str(" · the Profile Claude is using syncs once Claude is quit");
    }
    if report.not_backed_up > 0 {
        note.push_str(&format!(
            " · {} couldn't be backed up and were left as they were",
            report.not_backed_up
        ));
    }
    note
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
            note: Some("Available when AccRoster runs from its app bundle.".into()),
        },
    }
}
