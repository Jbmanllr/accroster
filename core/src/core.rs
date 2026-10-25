//! [`Core`]: the one object the UI layer talks to.

use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use jiff::SignedDuration;

use crate::backups;
use crate::cli_switcher::{self, TurnedOff};
use crate::discovered::{self, Evidence};
use crate::env::{Environment, LoginItemStatus};
use crate::interface::{
    AboutView, CliSwitchingView, Command, DiscoveredPair, Effect, Event, FirstRunView,
    HiddenDiscovered, LaunchAtLoginView, MenuBarView, MenuItem, MenuView, NicknameTarget,
    SettingsView, View,
};
use crate::logging::{Field, Level, Logger};
use crate::monitor::{LimitMonitor, Watched};
use crate::profiles::{self, ProfileRegistry, SignInTarget};
use crate::settings::{self, DiscoveredChoice, Settings};
use crate::storage::{self, AppPaths};

/// The tagline, shown in About and the README.
pub const TAGLINE: &str =
    "A menu bar app for Claude Code accounts: limits, sessions and one-click switching.";

/// The non-affiliation disclaimer, shown in About and the README.
pub const DISCLAIMER: &str = "AccRoster is an independent open-source project. It is not affiliated with, endorsed by, or sponsored by Anthropic. Claude and Claude Code are trademarks of Anthropic, PBC.";

/// How long Verbose logging stays on once turned on.
const VERBOSE_LOGGING_LASTS: SignedDuration = SignedDuration::from_hours(24);

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
    /// What the last CLI switching change left for the user to do.
    cli_note: Option<String>,
    /// Discovered pairs as of the last scan (launch, menu open).
    discovered: Vec<Evidence>,
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
            cli_note: None,
            discovered: Vec::new(),
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
        Ok(core)
    }

    /// How soon the driver should send the next [`Event::Tick`]: every second
    /// while a sign-in is waiting (so its end shows at once), more often
    /// while a usage read runs, by the time the next read is due, and at most
    /// [`crate::driver::TICK_INTERVAL`].
    pub fn tick_interval(&self) -> Duration {
        let idle = if self.profiles.signing_in() {
            Duration::from_secs(1)
        } else {
            crate::driver::TICK_INTERVAL
        };
        self.monitor
            .tick_interval(self.env.clock.now(), &self.watched(), idle)
    }

    /// Handles a user intent.
    pub fn command(&mut self, command: Command) -> Vec<Effect> {
        self.log(
            Level::Debug,
            "command",
            &[("name", Field::Word(command.name()))],
        );
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
                let target = SignInTarget {
                    email: evidence.email().map(str::to_owned),
                    name: evidence.known_name(),
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
            Command::OpenLimitResets => vec![Effect::OpenUrl(LIMIT_RESETS_URL.into())],
        }
    }

    /// Handles something that happened outside the core.
    pub fn event(&mut self, event: Event) -> Vec<Effect> {
        match event {
            Event::Tick => {
                if self.settings.verbose_logging_until.is_some() && !self.verbose_logging() {
                    self.set_verbose_logging_until(None);
                }
                let (profiles, ctx) = self.profiles();
                profiles.poll(&ctx);
            }
            Event::MenuOpened => {
                self.rescan_discovered();
                let watched = self.watched();
                self.monitor.menu_opened(self.env.clock.now(), &watched);
            }
            Event::WillSleep | Event::DidWake => {
                let watched = self.watched();
                self.monitor.set_asleep(event == Event::WillSleep, &watched);
            }
            Event::ScreenLocked | Event::ScreenUnlocked => {
                let watched = self.watched();
                self.monitor
                    .set_locked(event == Event::ScreenLocked, &watched);
            }
            Event::NetworkChanged { online } => self.monitor.set_online(online),
        }
        self.run_monitor();
        Vec::new()
    }

    /// Lets the Limit monitor collect a finished read and start the next.
    fn run_monitor(&mut self) {
        let watched = self.watched();
        let ctx = profiles::Context {
            env: &self.env,
            paths: &self.paths,
            logger: &self.logger,
            verbose: self.verbose_logging(),
        };
        // Finished reads, for the Profile registry to act on (Login expired,
        // ticket #37); nothing else needs them yet.
        let _completed = self.monitor.tick(&ctx, &watched);
    }

    /// The signed-in Profiles the Limit monitor reads, with which are a
    /// client's Active Profile (today the CLI's, from its pointer).
    fn watched(&self) -> Vec<Watched> {
        let cli_active = cli_switcher::pointer(&self.paths);
        self.profiles
            .watched(|config_dir| cli_active.as_deref() == Some(config_dir))
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
                    .map(String::clone)
                    .chain(self.profiles.notice().map(str::to_owned)),
            )
            .map(MenuItem::Warning)
            .collect();
        let mut listed = self.profiles.listed(|id| {
            let active = watched.iter().any(|w| w.id == id && w.active);
            self.monitor.view(&ctx, id, active)
        });
        let cli = cli_switcher::present(&self.paths, &mut listed);
        let pointer = cli_switcher::pointer(&self.paths);
        // The menu bar's figure is the CLI's Active Profile's: dimmed with it.
        let menu_bar_dimmed = listed
            .iter()
            .any(|profile| Some(profile.config_dir) == pointer.as_deref() && profile.row.dimmed);
        let header: Vec<String> = cli.header.into_iter().collect();
        if !header.is_empty() {
            items.push(MenuItem::Detail(header.join("    ")));
        }
        if !items.is_empty() {
            items.push(MenuItem::Separator);
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
                hidden_discovered: shown
                    .iter()
                    .filter(|(_, choice)| choice.is_some_and(|c| c.hidden))
                    .map(|(evidence, choice)| HiddenDiscovered {
                        title: evidence.hidden_title(choice.and_then(|c| c.nickname.as_deref())),
                        restore: Command::UnhideDiscovered(evidence.pair.clone()),
                    })
                    .collect(),
            },
            about: AboutView {
                name: "AccRoster".into(),
                version: env!("CARGO_PKG_VERSION").into(),
                tagline: TAGLINE.into(),
                disclaimer: DISCLAIMER.into(),
            },
            // Offered once: only until answered, and only while it's ours to
            // offer (not already on, not turned off in System Settings).
            first_run: (self.settings.launch_at_login.is_none()
                && login_item == LoginItemStatus::NotRegistered)
                .then_some(FirstRunView {
                    launch_at_login: true,
                }),
        }
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
        self.settings
            .verbose_logging_until
            .is_some_and(|until| until > self.env.clock.now())
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

    /// The Profile registry, with what it needs from the rest of the core.
    fn profiles(&mut self) -> (&mut ProfileRegistry, profiles::Context<'_>) {
        let verbose = self.verbose_logging();
        let ctx = profiles::Context {
            env: &self.env,
            paths: &self.paths,
            logger: &self.logger,
            verbose,
        };
        (&mut self.profiles, ctx)
    }

    /// What the registry and the monitor need from the rest of the core.
    fn context(&self) -> profiles::Context<'_> {
        profiles::Context {
            env: &self.env,
            paths: &self.paths,
            logger: &self.logger,
            verbose: self.verbose_logging(),
        }
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
