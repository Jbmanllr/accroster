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
    AboutView, ClaudeVersions, CliSwitchingView, Command, DiscoveredPair, Effect, Event,
    FirstRunView, HiddenDiscovered, LaunchAtLoginView, MenuBarView, MenuItem, MenuView,
    NicknameTarget, SafetyFeature, SettingsView, View,
};
use crate::logging::{Field, Level, Logger};
use crate::profiles::{self, ProfileRegistry, SignInTarget};
use crate::runner::Runner;
use crate::safety::{self, CheckId, Finding, Safety};
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
    /// What the last CLI switching change left for the user to do.
    cli_note: Option<String>,
    desktop: DesktopBridge,
    /// Discovered pairs as of the last scan (launch, menu open).
    discovered: Vec<Evidence>,
    /// Format check results and Safety pauses.
    safety: Safety,
    /// The Claude versions as of the last check (launch, menu open).
    versions: ClaudeVersions,
    /// Each Profile's links as of the last check, by Profile id.
    links: Vec<ProfileLinks>,
}

/// What the last link check found in one Profile folder.
struct ProfileLinks {
    id: String,
    /// Links whose place holds something else.
    replaced: Vec<&'static str>,
    /// Whether every shared-Transcript link is in place.
    transcripts_shared: bool,
}

/// An account as `(email, Organization uuid)`.
type Account = (Option<String>, Option<String>);

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
            cli_note: None,
            desktop: DesktopBridge::default(),
            discovered: Vec::new(),
            safety: Safety::load(&paths_for_safety),
            versions: ClaudeVersions::default(),
            links: Vec::new(),
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
        Ok(core)
    }

    /// How soon the driver should send the next [`Event::Tick`]: every second
    /// while a sign-in is waiting (so its end shows at once), otherwise
    /// [`crate::driver::TICK_INTERVAL`].
    pub fn tick_interval(&self) -> Duration {
        if self.profiles.signing_in() || self.desktop.waiting() {
            Duration::from_secs(1)
        } else {
            crate::driver::TICK_INTERVAL
        }
    }

    /// Effects the core queued outside an input (a Safety pause noticed at
    /// launch). The driver hands them to the UI with the first view;
    /// [`Core::command`] and [`Core::event`] include them in what they return.
    pub fn take_effects(&mut self) -> Vec<Effect> {
        self.safety.take_effects()
    }

    /// Handles a user intent.
    pub fn command(&mut self, command: Command) -> Vec<Effect> {
        self.log(
            Level::Debug,
            "command",
            &[("name", Field::Word(command.name()))],
        );
        let mut effects = self.handle(command);
        effects.extend(self.take_effects());
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
                desktop.start(&ctx, target)
            }
            Command::ConfirmDesktopSwitch => {
                let (desktop, ctx) = self.desktop();
                desktop.confirm(&ctx)
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
        }
    }

    /// Handles something that happened outside the core.
    pub fn event(&mut self, event: Event) -> Vec<Effect> {
        let mut effects = match event {
            Event::Tick => {
                if self.settings.verbose_logging_until.is_some() && !self.verbose_logging() {
                    self.set_verbose_logging_until(None);
                }
                let was_signing_in = self.profiles.signing_in();
                let (profiles, ctx) = self.profiles();
                profiles.poll(&ctx);
                let mut effects = profiles.take_effects();
                if self.profiles.take_auth_disagreement() {
                    let finding = self.config_dir_disagreement("in a new Profile");
                    self.safety.record(CheckId::ConfigDirHonoured, finding);
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
                effects
            }
            Event::MenuOpened => {
                self.rescan_discovered();
                if !self.self_check_if_versions_changed() {
                    self.safety.check_settings_env(&self.env, true);
                    self.check_links();
                    self.settle();
                }
                Vec::new()
            }
        };
        effects.extend(self.take_effects());
        effects
    }

    /// Everything the UI shows right now.
    pub fn view(&self) -> View {
        let login_item = self.env.login_item.status();
        let safety_lines = self.safety.warning_lines();
        let mut items: Vec<MenuItem> = self
            .warnings
            .iter()
            .chain(&safety_lines)
            .map(String::as_str)
            .chain(self.profiles.notice())
            .map(|text| MenuItem::Warning(text.to_owned()))
            .collect();
        let mut listed = self
            .profiles
            .listed(self.env.clock.as_ref(), &self.settings);
        for profile in &mut listed {
            self.annotate_links(profile.id, &mut profile.row);
        }
        let cli = cli_switcher::present(
            &self.paths,
            &mut listed,
            self.safety.is_paused(SafetyFeature::CliSwitching),
        );
        items.extend(self.desktop.menu_items());
        if !items.is_empty() {
            items.push(MenuItem::Separator);
        }
        let signed_in = self.profiles.signed_in();
        let desktop_account = desktop::desktop_account(&DesktopPaths::new(&self.env.home));
        let desktop_profile = desktop_account
            .as_ref()
            .and_then(|account| account.profile(&signed_in));
        // The CLI part comes first.
        let mut header: Vec<String> = cli.header.into_iter().collect();
        if desktop_account.is_some() {
            let name = desktop_profile.map_or("another account", |profile| &profile.name);
            header.push(format!("Desktop: {name}"));
        }
        if !header.is_empty() {
            items.push(MenuItem::Detail(header.join("    ")));
        }
        let desktop_installed = self.env.os.desktop_app_version().is_some();
        for listed in listed.iter_mut() {
            let Some(profile) = signed_in.iter().find(|profile| profile.id == listed.id) else {
                continue;
            };
            let row = &mut listed.row;
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
                dimmed: false,
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
            diagnostics: self.diagnostics(),
        }
    }

    /// The Diagnostics window: every check, the versions, the pauses.
    fn diagnostics(&self) -> crate::interface::DiagnosticsView {
        let usage = self.profiles.usage_summary();
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
    /// `auth`; then pauses whatever a failed check blocks.
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
        self.safety
            .record(CheckId::SidebarEntries, checks::sidebar_entries(&home));
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
                &[("feature", Field::Word(feature_word(feature)))],
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
    /// blocks Transcript sharing; other replaced links are only reported.
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
            Some(text) => {
                Finding::fail(text.clone()).blocking(SafetyFeature::TranscriptSharing, text)
            }
            None => Finding::pass(),
        };
        let others = match summary(false) {
            Some(text) => Finding::fail(text),
            None => Finding::pass(),
        };
        self.safety.record(CheckId::TranscriptLinks, transcripts);
        self.safety.record(CheckId::OtherLinks, others);
    }

    /// Runs `claude auth status --json` in every signed-in Profile and
    /// compares it with the folder's `oauthAccount`. Unreadable answers say
    /// nothing either way.
    fn check_config_dir_honoured(&mut self) {
        let runner = Runner::new(&self.env, &self.paths);
        // Per signed-in Profile: the (email, Organization) its folder holds,
        // and what a readable, logged-in `auth status` said, if anything.
        let answers: Vec<(Account, Option<Account>)> = self
            .profiles
            .accounts()
            .into_iter()
            .map(|account| {
                let said = runner
                    .auth_status(account.config_dir)
                    .filter(|status| status.logged_in)
                    .map(|status| (status.email, status.org_id));
                let held = (
                    Some(account.email.to_owned()),
                    Some(account.organization_uuid.to_owned()),
                );
                (held, said)
            })
            .collect();
        let said: Vec<&Account> = answers
            .iter()
            .filter_map(|(_, said)| said.as_ref())
            .collect();
        if said.is_empty() {
            return;
        }
        let one_identity = answers.len() > 1
            && said.len() == answers.len()
            && said.iter().all(|account| *account == said[0]);
        let disagreeing = answers
            .iter()
            .filter(|(held, said)| said.as_ref().is_some_and(|said| said != held))
            .count();
        let finding = if one_identity {
            let reason = format!(
                "{} ignores Profile folders",
                safety::claude_code_name(&self.versions)
            );
            Finding::fail("every Profile reports one account")
                .blocking(SafetyFeature::CliSwitching, reason)
        } else if disagreeing > 0 {
            let whose = if disagreeing > 1 {
                "Profiles"
            } else {
                "Profile"
            };
            self.config_dir_disagreement(&format!("in {disagreeing} {whose}"))
        } else {
            Finding::pass()
        };
        self.safety.record(CheckId::ConfigDirHonoured, finding);
    }

    /// `auth status` named another account than `oauthAccount`.
    fn config_dir_disagreement(&self, where_: &str) -> Finding {
        Finding::fail(format!("auth status names another account {where_}")).blocking(
            SafetyFeature::CliSwitching,
            format!(
                "{} didn't use a Profile folder's login",
                safety::claude_code_name(&self.versions)
            ),
        )
    }

    /// "Resume <feature>": runs the feature's checks again and ends its
    /// pause if none fails.
    fn resume(&mut self, feature: SafetyFeature) {
        match feature {
            SafetyFeature::CliSwitching => {
                self.versions = desktop::versions(&self.env);
                self.safety.check_settings_env(&self.env, false);
                self.check_accounts();
                self.check_config_dir_honoured();
            }
            SafetyFeature::TranscriptSharing => self.check_links(),
        }
        self.settle();
        if !self.safety.resume(feature) {
            self.log(
                Level::Info,
                "safety pause kept",
                &[("feature", Field::Word(feature_word(feature)))],
            );
            return;
        }
        self.log(
            Level::Info,
            "safety pause resumed",
            &[("feature", Field::Word(feature_word(feature)))],
        );
        if feature == SafetyFeature::TranscriptSharing {
            let (profiles, ctx) = self.profiles();
            profiles.heal_links(&ctx);
            self.check_links();
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

    /// Remove… confirmed: signs the Profile out and deletes its folder, then
    /// forgets its nickname and place in the order.
    fn remove_profile(&mut self, id: &str) {
        let (profiles, ctx) = self.profiles();
        if profiles.remove(&ctx, id) {
            self.forget_profile_settings(id);
        }
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
    fn desktop(&mut self) -> (&mut DesktopBridge, profiles::Context<'_>) {
        let verbose = self.verbose_logging();
        let ctx = profiles::Context {
            env: &self.env,
            paths: &self.paths,
            logger: &self.logger,
            verbose,
            share_transcripts: !self.safety.is_paused(SafetyFeature::TranscriptSharing),
        };
        (&mut self.desktop, ctx)
    }

    /// The Profile registry, with what it needs from the rest of the core.
    fn profiles(&mut self) -> (&mut ProfileRegistry, profiles::Context<'_>) {
        let verbose = self.verbose_logging();
        let ctx = profiles::Context {
            env: &self.env,
            paths: &self.paths,
            logger: &self.logger,
            verbose,
            share_transcripts: !self.safety.is_paused(SafetyFeature::TranscriptSharing),
        };
        (&mut self.profiles, ctx)
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

fn feature_word(feature: SafetyFeature) -> &'static str {
    match feature {
        SafetyFeature::CliSwitching => "cli_switching",
        SafetyFeature::TranscriptSharing => "transcript_sharing",
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
