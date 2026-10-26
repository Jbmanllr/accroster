//! The Profile registry: which Profiles exist, adding one through Claude
//! Code's browser sign-in, and each Profile's latest Limit reading.
//!
//! A Profile is a folder `profiles/<id>/` with a fixed id. It is never renamed
//! or moved, because its Keychain item is a hash of the literal path, so the
//! absolute path is stored once in `state/profiles.json` and reused verbatim.
//! Profile folders decide which Profiles exist; the file only records each
//! one's path.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::backups;
use crate::claude_json::{self, LoginNames};
use crate::cli_switcher;
use crate::env::{Environment, RunningProcess};
use crate::interface::{
    Command, Effect, MenuItem, NicknameTarget, ProfileKey, ProfileRow, SafetyFeature,
};
use crate::logging::{Field, Level, Logger};
use crate::monitor::{Completed, LimitsView, Watched};
use crate::runner::{AuthStatus, Ended, Finished, JobKind, Jobs, Runner};
use crate::settings::Settings;
use crate::storage::{self, AppPaths, ReadError, VersionedFile};

/// A sign-in still running after this long is killed.
pub(crate) const SIGN_IN_TIMEOUT: SignedDuration = SignedDuration::from_mins(10);

/// What the registry needs from the core for one call.
pub(crate) struct Context<'a> {
    pub env: &'a Environment,
    pub paths: &'a AppPaths,
    pub logger: &'a Logger,
    pub verbose: bool,
    /// The write features under a Safety pause: nothing they write is
    /// written.
    pub paused: Vec<SafetyFeature>,
    /// The `claude` spawns run; `None` while none is found.
    pub claude: Option<&'a std::path::Path>,
}

impl Context<'_> {
    pub(crate) fn is_paused(&self, feature: SafetyFeature) -> bool {
        self.paused.contains(&feature)
    }

    pub(crate) fn log(
        &self,
        level: Level,
        message: &'static str,
        fields: &[(&'static str, Field<'_>)],
    ) {
        self.logger
            .write(self.env.clock.now(), self.verbose, level, message, fields);
    }

    pub(crate) fn runner(&self) -> Runner<'_> {
        Runner::new(self.env, self.paths, self.claude)
    }
}

/// A Login acting within an Organization, from `oauthAccount` in a Profile
/// folder's `.claude.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    key: ProfileKey,
    names: LoginNames,
    /// For example `claude_max`; the plan when no usage read has said.
    organization_type: Option<String>,
}

impl Identity {
    /// `oauthAccount` of `<config_dir>/.claude.json`, or `None` when the
    /// folder holds no sign-in.
    fn read(config_dir: &Path) -> Option<Identity> {
        let account = claude_json::read_account(&config_dir.join(".claude.json"))?;
        Some(Identity {
            key: account.key,
            names: account.names,
            organization_type: account.organization_type,
        })
    }

    /// The Profile name on its own (see [`LoginNames::name`]).
    fn name(&self) -> String {
        self.names.name()
    }
}

/// Each Profile's short name, in `profiles`' order: its nickname, else the
/// shortest of [`LoginNames::short_labels`] no other Profile shares (two
/// Profiles named `Lucie` become `Lucie · Work Inc` and `Lucie · Research
/// Lab`).
fn short_labels(profiles: &[&Profile], settings: &Settings) -> Vec<String> {
    unique(
        profiles
            .iter()
            .map(|profile| match settings.nicknames.get(&profile.id) {
                Some(nickname) => vec![nickname.clone()],
                None => match &profile.identity {
                    Some(identity) => identity.names.short_labels(),
                    None => vec!["Signed out".to_owned()],
                },
            })
            .collect(),
    )
}

/// Each Profile's name, in `profiles`' order: [`LoginNames::name`], with the
/// Organization added when two Profiles would otherwise share it.
fn names(profiles: &[&Profile]) -> Vec<String> {
    unique(
        profiles
            .iter()
            .map(|profile| match &profile.identity {
                Some(identity) => vec![identity.name(), identity.names.name_with_organization()],
                None => vec!["Signed-out Profile".to_owned()],
            })
            .collect(),
    )
}

/// One pick per list of choices (shortest first): the first no other list's
/// pick shares. Colliding lists move on together until none collides or a
/// list runs out of choices.
fn unique(choices: Vec<Vec<String>>) -> Vec<String> {
    let mut level = vec![0; choices.len()];
    loop {
        let label = |index: usize| &choices[index][level[index]];
        let collided: Vec<usize> = (0..choices.len())
            .filter(|&index| {
                level[index] + 1 < choices[index].len()
                    && (0..choices.len())
                        .any(|other| other != index && label(other) == label(index))
            })
            .collect();
        if collided.is_empty() {
            break;
        }
        for index in collided {
            level[index] += 1;
        }
    }
    choices
        .into_iter()
        .zip(level)
        .map(|(mut choices, level)| choices.swap_remove(level))
        .collect()
}

/// A signed-in Profile as the menu lists it.
pub(crate) struct Listed<'a> {
    pub id: &'a str,
    /// The folder's absolute path, verbatim.
    pub config_dir: &'a Path,
    /// Whether its folder holds a Login (signed in or expired).
    pub signed_in: bool,
    /// Its Profile name (see [`names`]), or "Signed-out Profile".
    pub name: String,
    /// A nickname titles its row.
    pub nicknamed: bool,
    /// Its Short label (menu bar, header): the nickname, else the person's
    /// first name (`Lucie`), lengthened when two Profiles share it.
    pub short_label: String,
    pub row: ProfileRow,
}

/// A signed-in Profile as other modules see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SignedInProfile {
    pub id: String,
    pub key: ProfileKey,
    /// Its Profile name (see [`names`]).
    pub name: String,
    /// Its Organization as people call it (see
    /// [`LoginNames::organization_label`]); empty when unknown.
    pub organization_label: String,
}

/// `state/profiles.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct RegistryFile {
    profiles: Vec<Entry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    id: String,
    /// The folder's absolute path as first created; every spawn uses it
    /// byte for byte.
    config_dir: PathBuf,
    /// The Login expired (and was notified); cleared by Sign in again….
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    expired: bool,
}

impl VersionedFile for RegistryFile {
    const VERSION: u64 = 1;
}

struct Profile {
    id: String,
    config_dir: PathBuf,
    /// `None` when the folder holds no `oauthAccount`: signed out.
    identity: Option<Identity>,
    /// Login expired: `rate_limits_available: false`, or `loggedIn: false`
    /// while `oauthAccount` remains. Not polled until signed in again.
    expired: bool,
}

impl Profile {
    /// The menu row: `title` is the nickname, else the Profile name.
    fn row(&self, title: String, limits: LimitsView) -> ProfileRow {
        let mut submenu = Vec::new();
        let mut row_limits = LimitsView::default();
        if let Some(identity) = &self.identity {
            let plan = limits.plan.clone().or_else(|| {
                identity
                    .organization_type
                    .as_deref()
                    .map(|kind| kind.strip_prefix("claude_").unwrap_or(kind).to_owned())
            });
            let mut detail = identity.names.email_and_organization();
            if let Some(plan) = plan.filter(|plan| !plan.is_empty()) {
                detail.push_str(" · ");
                detail.push_str(&capitalized(&plan));
            }
            submenu.push(MenuItem::Detail(detail));
            row_limits = limits;
            submenu.extend(row_limits.details.drain(..).map(MenuItem::Detail));
            if !self.expired {
                submenu.push(MenuItem::Action {
                    title: "Limit resets → claude.ai".to_owned(),
                    command: Command::OpenLimitResets,
                });
            }
        }
        let LimitsView {
            bars,
            mut dimmed,
            mut note,
            offers,
            ..
        } = row_limits;
        let action = |title: &str, command: Command| MenuItem::Action {
            title: title.to_owned(),
            command,
        };
        if self.identity.is_none() {
            note = Some("Signed out".to_owned());
        } else if self.expired {
            // The last reading stays, dimmed: polling stopped.
            note = Some("Login expired".to_owned());
            dimmed = true;
        }
        if self.identity.is_none() || self.expired {
            submenu.push(action(
                "Sign in again…",
                Command::SignInAgain(self.id.clone()),
            ));
        }
        submenu.push(action(
            "Rename…",
            Command::Rename(NicknameTarget::Profile(self.id.clone())),
        ));
        submenu.push(action(
            "Remove…",
            Command::AskRemoveProfile(self.id.clone()),
        ));
        ProfileRow {
            title,
            badges: Vec::new(),
            command: None,
            limits: bars,
            note,
            dimmed,
            marker: None,
            default_login: false,
            offers,
            submenu,
        }
    }
}

fn capitalized(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The account a sign-in started from a Discovered Profile is meant to add.
pub(crate) struct SignInTarget {
    pub pair: ProfileKey,
    /// Passed as `--email` when known.
    pub email: Option<String>,
    /// How the menu names it ("X" in "Signed in as Y, not X").
    pub name: String,
}

/// A sign-in waiting for the user in the browser.
struct SignIn {
    id: String,
    config_dir: PathBuf,
    process: Box<dyn RunningProcess>,
    started_at: Timestamp,
    /// The Discovered Profile Add Profile… was aimed at, if any.
    target: Option<SignInTarget>,
    /// Set for "Sign in again…" in an existing Profile's folder.
    again: Option<Again>,
}

/// What a sign-in in an existing Profile's folder must match or put back.
struct Again {
    /// The Login the Profile had; `None` for a signed-out Profile.
    expected: Option<Identity>,
    /// Its `oauthAccount`, restored when another Login's sign-in is undone
    /// so the Profile stays expired rather than signed out.
    previous_account: Option<Value>,
}

/// A queued `auth status` or `auth logout` in a Profile folder, and what
/// happens once it ends.
struct Tag {
    id: String,
    then: Then,
}

enum Then {
    /// Remove…: delete the folder and forget the Profile.
    Remove,
    /// A folder that must not stay (a duplicate or abandoned sign-in):
    /// delete it once signed out. `failed` is the notice if that fails.
    Discard { failed: Option<String> },
    /// Another Login's sign-in into an existing Profile, undone: put the
    /// Profile's own Login back. `undone` is the notice when that worked,
    /// `failed` the one when the sign-out failed; `name` names the Profile.
    UndoSignInAgain {
        previous_account: Option<Value>,
        name: String,
        undone: String,
        failed: String,
    },
    /// A folder under `profiles/` that isn't a Profile (Uninstall…).
    RemoveUnlisted,
    /// `auth status` cross-checks a new Profile's sign-in. `keep_notice`:
    /// a mismatch notice already says more.
    ConfirmSignIn {
        identity: Identity,
        keep_notice: bool,
    },
    /// After a failed usage read: did the Login go?
    CheckExpiry,
    /// The self-check's cross-check of a signed-in Profile.
    CrossCheck { held: Account },
}

/// An account as `auth status` names it: email and Organization uuid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Account {
    pub email: Option<String>,
    pub organization_uuid: Option<String>,
}

/// What one Profile's `auth status` said in the self-check's cross-check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CrossCheckAnswer {
    /// The account the folder's `oauthAccount` names.
    pub held: Account,
    /// What a readable, logged-in answer named.
    pub said: Option<Account>,
    /// It never answered and was killed.
    pub timed_out: bool,
}

/// The self-check's cross-check while its answers come in.
struct CrossCheck {
    waiting: usize,
    answers: Vec<CrossCheckAnswer>,
}

/// Every Profile, plus the sign-in in progress, if any.
#[derive(Default)]
pub(crate) struct ProfileRegistry {
    profiles: Vec<Profile>,
    sign_in: Option<SignIn>,
    /// `auth status` and `auth logout` runs, one at a time.
    jobs: Jobs<Tag>,
    /// The outcome of the last sign-in, shown as a menu line until the next.
    notice: Option<String>,
    /// Set when `auth status` in a newly signed-in folder named another
    /// account than its `oauthAccount`; taken by the format checks.
    auth_disagreed: bool,
    /// Profiles removed since the core last asked.
    removed: Vec<String>,
    /// Unlisted folders kept because their sign-out failed, since the core
    /// last asked.
    unlisted_kept: usize,
    cross_check: Option<CrossCheck>,
    /// Effects (notifications) waiting for the core to hand to the UI.
    effects: Vec<Effect>,
}

impl ProfileRegistry {
    /// Loads the Profiles at launch: recorded folders that still exist, plus
    /// any signed-in folder missing from the record (recorded now). A folder
    /// with no record and no sign-in is what an interrupted Add Profile left
    /// behind, and is deleted. Links are healed separately
    /// ([`ProfileRegistry::heal_links`]), after the format checks.
    pub(crate) fn load(ctx: &Context) -> ProfileRegistry {
        let path = ctx.paths.state().join("profiles.json");
        let file = match storage::read_versioned::<RegistryFile>(&path) {
            Ok(file) => file.unwrap_or_default(),
            Err(ReadError::Io(error)) => {
                let error = error.to_string();
                ctx.log(
                    Level::Error,
                    "profiles record unreadable",
                    &[("error", Field::Text(&error))],
                );
                RegistryFile::default()
            }
            Err(ReadError::Unparsable(reason)) => {
                ctx.log(
                    Level::Warn,
                    "profiles record unparsable",
                    &[("error", Field::Text(&reason))],
                );
                RegistryFile::default()
            }
        };
        let mut registry = ProfileRegistry::default();
        for entry in file.profiles {
            if entry.config_dir.is_dir() && !registry.has(&entry.id) {
                registry.profiles.push(Profile {
                    identity: Identity::read(&entry.config_dir),
                    id: entry.id,
                    config_dir: entry.config_dir,
                    expired: entry.expired,
                });
            }
        }
        let mut found: Vec<(String, PathBuf)> = fs::read_dir(ctx.paths.profiles())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                )
            })
            .filter(|(id, _)| !registry.has(id))
            .collect();
        found.sort();
        let mut changed = false;
        for (id, config_dir) in found {
            match Identity::read(&config_dir) {
                Some(identity) if registry.find(&identity).is_some() => {
                    // A duplicate sign-in whose sign-out failed: kept so its
                    // login can still be revoked, never listed.
                    ctx.log(
                        Level::Warn,
                        "unrecorded duplicate left alone",
                        &[("profile", Field::Profile(&id))],
                    );
                }
                Some(identity) => {
                    ctx.log(
                        Level::Warn,
                        "unrecorded profile adopted",
                        &[("profile", Field::Profile(&id))],
                    );
                    registry.profiles.push(Profile {
                        id,
                        config_dir,
                        identity: Some(identity),
                        expired: false,
                    });
                    changed = true;
                }
                None => {
                    ctx.log(
                        Level::Info,
                        "abandoned sign-in folder deleted",
                        &[("profile", Field::Profile(&id))],
                    );
                    remove_folder(ctx, &id, &config_dir);
                }
            }
        }
        if changed {
            registry.save(ctx);
        }
        registry
    }

    /// Makes every Profile's missing links again.
    pub(crate) fn heal_links(&self, ctx: &Context) {
        for profile in &self.profiles {
            link(ctx, &profile.id, &profile.config_dir);
        }
    }

    /// Every Profile folder, signed in or not, with its id.
    pub(crate) fn folders(&self) -> Vec<(&str, &Path)> {
        self.profiles
            .iter()
            .map(|profile| (profile.id.as_str(), profile.config_dir.as_path()))
            .collect()
    }

    /// Queues `claude auth status --json` in every signed-in Profile, to
    /// compare with the account its folder's `oauthAccount` names. Returns
    /// whether anything was queued; the answers come from
    /// [`ProfileRegistry::take_cross_check`]. A cross-check already
    /// running is not started again.
    pub(crate) fn start_cross_check(&mut self, ctx: &Context) -> bool {
        if self.cross_check.is_some() {
            return true;
        }
        let mut waiting = 0;
        for profile in &self.profiles {
            let Some(identity) = &profile.identity else {
                continue;
            };
            let held = Account {
                email: Some(identity.names.email.clone()),
                organization_uuid: Some(identity.key.organization_uuid.clone()),
            };
            let tag = Tag {
                id: profile.id.clone(),
                then: Then::CrossCheck { held },
            };
            self.jobs
                .push(&profile.config_dir, JobKind::AuthStatus, tag);
            waiting += 1;
        }
        if waiting == 0 {
            return false;
        }
        self.cross_check = Some(CrossCheck {
            waiting,
            answers: Vec::new(),
        });
        self.run_jobs(ctx);
        true
    }

    /// Whether a cross-check's answers are still coming in.
    pub(crate) fn cross_checking(&self) -> bool {
        self.cross_check.is_some()
    }

    /// The cross-check's answers, once every one is in.
    pub(crate) fn take_cross_check(&mut self) -> Option<Vec<CrossCheckAnswer>> {
        if self.cross_check.as_ref()?.waiting > 0 {
            return None;
        }
        self.cross_check.take().map(|check| check.answers)
    }

    /// Whether `auth status` contradicted a sign-in since the last call.
    pub(crate) fn take_auth_disagreement(&mut self) -> bool {
        std::mem::take(&mut self.auth_disagreed)
    }

    /// Whether an `auth status` or `auth logout` is running or waiting.
    pub(crate) fn busy(&self) -> bool {
        !self.jobs.is_empty()
    }

    /// The Profiles removed since the last call.
    pub(crate) fn take_removed(&mut self) -> Vec<String> {
        std::mem::take(&mut self.removed)
    }

    /// How many unlisted folders were kept since the last call.
    pub(crate) fn take_unlisted_kept(&mut self) -> usize {
        std::mem::take(&mut self.unlisted_kept)
    }

    /// Whether Remove… is signing Profile `id` out.
    fn removing(&self, id: &str) -> bool {
        self.jobs
            .queued()
            .any(|(_, tag)| tag.id == id && matches!(tag.then, Then::Remove))
    }

    /// Whether a run is queued in `config_dir`: the Limit monitor leaves it
    /// alone meanwhile, so two Claude Code runs never share a folder.
    fn in_use(&self, config_dir: &Path) -> bool {
        self.jobs.queued().any(|(dir, _)| dir == config_dir)
    }

    /// Collects the runs that ended and starts the next.
    pub(crate) fn run_jobs(&mut self, ctx: &Context) {
        loop {
            let ended = self.jobs.poll(&ctx.runner());
            if ended.is_empty() {
                return;
            }
            for ended in ended {
                self.job_ended(ctx, ended);
            }
        }
    }

    fn job_ended(&mut self, ctx: &Context, ended: Ended<Tag>) {
        let Ended {
            config_dir,
            tag: Tag { id, then },
            finished,
        } = ended;
        let finished = &finished;
        if matches!(finished, Finished::TimedOut) {
            ctx.log(
                Level::Warn,
                "claude code run timed out",
                &[("profile", Field::Profile(&id))],
            );
        }
        match then {
            Then::Remove => self.removal_ended(ctx, &id, finished.succeeded()),
            Then::Discard { failed } => {
                if signed_out(ctx, &id, finished) {
                    remove_folder(ctx, &id, &config_dir);
                } else if let Some(failed) = failed {
                    self.notice = Some(failed);
                }
            }
            Then::UndoSignInAgain {
                previous_account,
                name,
                undone,
                failed,
            } => {
                if !signed_out(ctx, &id, finished) {
                    self.notice = Some(failed);
                } else if let Some(account) = previous_account
                    && let Err(not_restored) = restore_account(ctx, &id, &config_dir, &account)
                {
                    self.notice = Some(format!(
                        "{undone}, but {}, so its Login wasn't put back",
                        not_restored.reason(&name)
                    ));
                }
            }
            Then::RemoveUnlisted => {
                if signed_out(ctx, &id, finished) {
                    // Signed out: a folder left here goes with the app's folder.
                    remove_folder(ctx, &id, &config_dir);
                } else {
                    self.unlisted_kept += 1;
                }
            }
            Then::ConfirmSignIn {
                identity,
                keep_notice,
            } => self.confirm_sign_in(ctx, &id, &identity, keep_notice, finished),
            Then::CheckExpiry => {
                let logged_out =
                    AuthStatus::parse(finished).is_some_and(|status| !status.logged_in);
                let index = self.profiles.iter().position(|profile| profile.id == id);
                if let Some(index) = index.filter(|_| logged_out)
                    && self.profiles[index].identity.is_some()
                {
                    self.expire(ctx, index);
                }
            }
            Then::CrossCheck { held } => {
                let said = AuthStatus::parse(finished)
                    .filter(|status| status.logged_in)
                    .map(|status| Account {
                        email: status.email,
                        organization_uuid: status.org_id,
                    });
                if let Some(check) = &mut self.cross_check {
                    check.waiting -= 1;
                    check.answers.push(CrossCheckAnswer {
                        held,
                        said,
                        timed_out: matches!(finished, Finished::TimedOut),
                    });
                }
            }
        }
    }

    /// Remove…'s sign-out ended: on success the folder is deleted and the
    /// Profile forgotten; otherwise both stay.
    fn removal_ended(&mut self, ctx: &Context, id: &str, signed_out: bool) {
        let Some(index) = self.profiles.iter().position(|profile| profile.id == id) else {
            return;
        };
        let name = self.name(id).unwrap_or_default();
        let profile = &self.profiles[index];
        if !signed_out {
            ctx.log(
                Level::Warn,
                "sign-out failed, profile kept",
                &[("profile", Field::Profile(id))],
            );
            self.notice = Some(format!("Couldn't sign out {name} · nothing was removed"));
            return;
        }
        if !remove_folder(ctx, id, &profile.config_dir) {
            self.notice = Some(format!(
                "Signed out {name}, but its folder couldn't be deleted"
            ));
        }
        self.profiles.remove(index);
        self.save(ctx);
        ctx.log(
            Level::Info,
            "profile removed",
            &[("profile", Field::Profile(id))],
        );
        self.removed.push(id.to_owned());
    }

    /// A new Profile's sign-in, cross-checked: an answer that doesn't name
    /// the folder's account gets a notice, and a logged-in one naming
    /// another account counts against Claude Code honouring Profile folders.
    fn confirm_sign_in(
        &mut self,
        ctx: &Context,
        id: &str,
        identity: &Identity,
        keep_notice: bool,
        finished: &Finished,
    ) {
        let status = AuthStatus::parse(finished);
        let confirmed = status.as_ref().is_some_and(|status| {
            status.logged_in
                && status.email.as_deref() == Some(identity.names.email.as_str())
                && status.org_id.as_deref() == Some(identity.key.organization_uuid.as_str())
        });
        if confirmed {
            return;
        }
        // A readable, logged-in answer naming another account means Claude
        // Code didn't use this folder's login.
        self.auth_disagreed |= status.is_some_and(|status| status.logged_in);
        ctx.log(
            Level::Warn,
            "sign-in not confirmed by auth status",
            &[("profile", Field::Profile(id))],
        );
        if !keep_notice {
            self.notice = Some(format!(
                "Claude Code didn't confirm the sign-in of {}",
                identity.name()
            ));
        }
    }

    /// Add Profile…: creates the folder, links it, seeds `.claude.json`, and
    /// starts the browser sign-in. Ignored while a sign-in is running. With a
    /// `target` (a Discovered Profile), its email is passed to the sign-in and
    /// the account that signs in is checked against its pair.
    pub(crate) fn add(&mut self, ctx: &Context, target: Option<SignInTarget>) {
        if self.sign_in.is_some() {
            return;
        }
        self.notice = None;
        let (id, config_dir) = match create_folder(ctx.paths) {
            Ok(created) => created,
            Err(error) => {
                let error = error.to_string();
                ctx.log(
                    Level::Error,
                    "profile folder not created",
                    &[("error", Field::Text(&error))],
                );
                self.notice = Some("Couldn't create a Profile folder".into());
                return;
            }
        };
        ctx.log(
            Level::Info,
            "sign-in started",
            &[("profile", Field::Profile(&id))],
        );
        link(ctx, &id, &config_dir);
        if ctx.is_paused(SafetyFeature::Seeding) {
            ctx.log(
                Level::Info,
                "claude.json not seeded: paused",
                &[("profile", Field::Profile(&id))],
            );
        } else if let Err(error) = cli_switcher::seed_claude_json(&config_dir, &ctx.env.home) {
            let error = error.to_string();
            ctx.log(
                Level::Warn,
                "claude.json not seeded",
                &[
                    ("profile", Field::Profile(&id)),
                    ("error", Field::Text(&error)),
                ],
            );
        }
        let email = target.as_ref().and_then(|target| target.email.as_deref());
        match ctx.runner().start_sign_in(&config_dir, email) {
            Ok(process) => {
                self.sign_in = Some(SignIn {
                    id,
                    config_dir,
                    process,
                    started_at: ctx.env.clock.now(),
                    target,
                    again: None,
                })
            }
            Err(error) => {
                let error = error.to_string();
                ctx.log(
                    Level::Error,
                    "sign-in not started",
                    &[
                        ("profile", Field::Profile(&id)),
                        ("error", Field::Text(&error)),
                    ],
                );
                remove_folder(ctx, &id, &config_dir);
                self.notice = Some("Couldn't start Claude Code to sign in".into());
            }
        }
    }

    /// "Waiting for sign-in… Cancel": kills the sign-in and deletes its folder.
    pub(crate) fn cancel(&mut self, ctx: &Context) {
        if let Some(sign_in) = self.sign_in.take() {
            ctx.log(
                Level::Info,
                "sign-in cancelled",
                &[("profile", Field::Profile(&sign_in.id))],
            );
            self.abandon(ctx, sign_in);
            self.run_jobs(ctx);
        }
    }

    /// Checks on the running sign-in: finishes it once Claude Code has exited,
    /// or abandons it after [`SIGN_IN_TIMEOUT`]. Collect what that queued
    /// with [`ProfileRegistry::run_jobs`].
    pub(crate) fn poll_sign_in(&mut self, ctx: &Context) {
        let Some(sign_in) = self.sign_in.as_mut() else {
            return;
        };
        if ctx.env.clock.now().duration_since(sign_in.started_at) >= SIGN_IN_TIMEOUT {
            let sign_in = self.sign_in.take().expect("checked above");
            ctx.log(
                Level::Info,
                "sign-in timed out",
                &[("profile", Field::Profile(&sign_in.id))],
            );
            let again = sign_in.again.is_some();
            self.abandon(ctx, sign_in);
            self.notice = Some(if again {
                "Sign-in timed out after 10 minutes · nothing changed".into()
            } else {
                "Sign-in timed out after 10 minutes · nothing was added".into()
            });
            return;
        }
        match sign_in.process.try_wait() {
            Ok(None) => {}
            Ok(Some(output)) => {
                let sign_in = self.sign_in.take().expect("checked above");
                let code = output.exit_code.map_or(-1, i64::from);
                ctx.log(
                    Level::Info,
                    "sign-in exited",
                    &[
                        ("profile", Field::Profile(&sign_in.id)),
                        ("exit_code", Field::Number(code)),
                    ],
                );
                if !output.succeeded() {
                    self.notice = Some(if sign_in.again.is_some() {
                        "Sign-in didn't complete · nothing changed".into()
                    } else {
                        self.discard(ctx, &sign_in.id, &sign_in.config_dir, None);
                        "Sign-in didn't complete · nothing was added".into()
                    });
                } else if sign_in.again.is_some() {
                    self.finish_again(ctx, sign_in);
                } else {
                    self.finish(ctx, sign_in);
                }
            }
            Err(error) => {
                let sign_in = self.sign_in.take().expect("checked above");
                let error = error.to_string();
                ctx.log(
                    Level::Error,
                    "sign-in lost",
                    &[
                        ("profile", Field::Profile(&sign_in.id)),
                        ("error", Field::Text(&error)),
                    ],
                );
                self.abandon(ctx, sign_in);
                self.notice = Some("Sign-in didn't complete · nothing was added".into());
            }
        }
    }

    /// Whether a sign-in is waiting for the user.
    pub(crate) fn signing_in(&self) -> bool {
        self.sign_in.is_some()
    }

    /// The outcome of the last sign-in, if it needs saying.
    pub(crate) fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Whether a signed-in Profile has this (account, Organization) pair.
    pub(crate) fn has_signed_in(&self, pair: &ProfileKey) -> bool {
        self.profiles.iter().any(|profile| {
            profile
                .identity
                .as_ref()
                .is_some_and(|identity| identity.key == *pair)
        })
    }

    /// The folder of signed-in Profile `id`, verbatim.
    pub(crate) fn config_dir(&self, id: &str) -> Option<&Path> {
        self.profiles
            .iter()
            .find(|profile| profile.id == id && profile.identity.is_some())
            .map(|profile| profile.config_dir.as_path())
    }

    /// Every Profile (signed in, expired or signed out), in the user's
    /// order, with its row titled by nickname when there is one; `limits`
    /// says what each row shows about its limits.
    pub(crate) fn listed(
        &self,
        settings: &Settings,
        limits: impl Fn(&str) -> LimitsView,
    ) -> Vec<Listed<'_>> {
        let ordered = self.ordered(&settings.profile_order);
        let short_labels = short_labels(&ordered, settings);
        let names = names(&ordered);
        ordered
            .into_iter()
            .zip(short_labels.into_iter().zip(names))
            .map(|(profile, (short_label, name))| {
                let nickname = settings.nicknames.get(&profile.id);
                let title = nickname.cloned().unwrap_or_else(|| name.clone());
                let mut row = profile.row(title, limits(&profile.id));
                if self.removing(&profile.id) {
                    row.note = Some("Signing out…".to_owned());
                    row.dimmed = true;
                    row.submenu.retain(|item| {
                        !matches!(
                            item,
                            MenuItem::Action {
                                command: Command::AskRemoveProfile(_),
                                ..
                            }
                        )
                    });
                }
                Listed {
                    id: &profile.id,
                    config_dir: &profile.config_dir,
                    signed_in: profile.identity.is_some(),
                    name,
                    nicknamed: nickname.is_some(),
                    short_label,
                    row,
                }
            })
            .collect()
    }

    /// The Profiles the Limit monitor reads, in the user's order: signed in,
    /// not expired, and with no `auth status` or `auth logout` queued in
    /// their folder. `active` says, by id and folder, which are a client's
    /// Active Profile.
    pub(crate) fn watched(&self, active: impl Fn(&str, &Path) -> bool) -> Vec<Watched> {
        self.profiles
            .iter()
            .filter(|profile| !profile.expired && !self.in_use(&profile.config_dir))
            .filter_map(|profile| {
                let identity = profile.identity.as_ref()?;
                Some(Watched {
                    id: profile.id.clone(),
                    config_dir: profile.config_dir.clone(),
                    account_uuid: identity.key.account_uuid.clone(),
                    active: active(&profile.id, &profile.config_dir),
                })
            })
            .collect()
    }

    /// Every Profile id, in the user's order: `order` first (ids it names
    /// that no longer exist are skipped), then the rest in add order.
    pub(crate) fn ordered_ids(&self, order: &[String]) -> Vec<String> {
        self.ordered(order)
            .into_iter()
            .map(|profile| profile.id.clone())
            .collect()
    }

    /// The prompt for Rename…, or `None` for an unknown Profile.
    pub(crate) fn nickname_prompt(&self, id: &str, settings: &Settings) -> Option<Effect> {
        Some(Effect::AskNickname {
            name: self.name(id)?,
            current: settings.nicknames.get(id).cloned(),
            target: NicknameTarget::Profile(id.to_owned()),
        })
    }

    /// The confirmation Remove… shows before `confirm` removes Profile `id`.
    /// It warns when `~/.config/anthropic/configs/` exists, because Claude
    /// Code's sign-out also deletes credentials there.
    pub(crate) fn removal_confirmation(
        &self,
        ctx: &Context,
        id: &str,
        confirm: Command,
    ) -> Option<Effect> {
        let name = self.name(id)?;
        let mut message = "AccRoster signs it out with Claude Code, then deletes its folder. \
                           Shared Transcripts and settings in ~/.claude stay."
            .to_owned();
        if ctx.env.home.join(".config/anthropic/configs").is_dir() {
            message.push_str(
                "\n\n~/.config/anthropic/configs/ exists: Claude Code's sign-out also \
                 revokes and deletes the credential files kept there, which other tools may use.",
            );
        }
        Some(Effect::ConfirmRemoval {
            title: format!("Remove {name}?"),
            message,
            confirm,
        })
    }

    /// Starts removing Profile `id`: `claude auth logout` first, then its
    /// folder (links are deleted, never followed, so shared Transcripts
    /// stay). A failed sign-out keeps the folder and the Profile. Removed
    /// Profiles come out of [`ProfileRegistry::take_removed`].
    pub(crate) fn remove(&mut self, ctx: &Context, id: &str) {
        let Some(config_dir) = self.profile(id).map(|profile| profile.config_dir.clone()) else {
            return;
        };
        let signing_in = self
            .sign_in
            .as_ref()
            .is_some_and(|sign_in| sign_in.id == id);
        if signing_in || self.removing(id) {
            return;
        }
        self.notice = None;
        let tag = Tag {
            id: id.to_owned(),
            then: Then::Remove,
        };
        self.jobs.push(&config_dir, JobKind::SignOut, tag);
        self.run_jobs(ctx);
    }

    /// A Profile's name, as its row has it without a nickname (see
    /// [`names`]), or "Signed-out Profile".
    pub(crate) fn name(&self, id: &str) -> Option<String> {
        let all: Vec<&Profile> = self.profiles.iter().collect();
        let index = all.iter().position(|profile| profile.id == id)?;
        names(&all).into_iter().nth(index)
    }

    /// Starts signing out, then deleting, every folder under `profiles/`
    /// that isn't a Profile (a kept duplicate sign-in); a folder holding no
    /// sign-in is just deleted. A folder whose sign-out fails is kept so its
    /// login can still be revoked, and counted by
    /// [`ProfileRegistry::take_unlisted_kept`].
    pub(crate) fn remove_unlisted(&mut self, ctx: &Context) {
        let unlisted: Vec<(String, PathBuf)> = fs::read_dir(ctx.paths.profiles())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|entry| {
                (
                    entry.file_name().to_string_lossy().into_owned(),
                    entry.path(),
                )
            })
            .filter(|(id, config_dir)| !self.has(id) && !self.in_use(config_dir))
            .collect();
        for (id, config_dir) in unlisted {
            if Identity::read(&config_dir).is_none() {
                remove_folder(ctx, &id, &config_dir);
            } else {
                let tag = Tag {
                    id,
                    then: Then::RemoveUnlisted,
                };
                self.jobs.push(&config_dir, JobKind::SignOut, tag);
            }
        }
        self.run_jobs(ctx);
    }

    fn profile(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|profile| profile.id == id)
    }

    fn ordered(&self, order: &[String]) -> Vec<&Profile> {
        let mut ordered: Vec<&Profile> = order
            .iter()
            .filter_map(|id| self.profiles.iter().find(|profile| &profile.id == id))
            .collect();
        for profile in &self.profiles {
            if !order.contains(&profile.id) {
                ordered.push(profile);
            }
        }
        ordered
    }

    /// Who each signed-in Profile is, in add order.
    pub(crate) fn signed_in(&self) -> Vec<SignedInProfile> {
        let all: Vec<&Profile> = self.profiles.iter().collect();
        all.iter()
            .zip(names(&all))
            .filter_map(|(profile, name)| {
                let identity = profile.identity.as_ref()?;
                Some(SignedInProfile {
                    id: profile.id.clone(),
                    key: identity.key.clone(),
                    name,
                    organization_label: identity.names.organization_label(),
                })
            })
            .collect()
    }

    /// A sign-in exited successfully: reads who signed in, undoes a duplicate,
    /// otherwise records the Profile (the Limit monitor reads it next).
    fn finish(&mut self, ctx: &Context, sign_in: SignIn) {
        let SignIn {
            id,
            config_dir,
            target,
            ..
        } = sign_in;
        let Some(identity) = Identity::read(&config_dir) else {
            ctx.log(
                Level::Warn,
                "sign-in left no account",
                &[("profile", Field::Profile(&id))],
            );
            self.discard(ctx, &id, &config_dir, None);
            self.notice = Some("Sign-in didn't complete · nothing was added".into());
            return;
        };
        if let Some(existing) = self.find(&identity) {
            ctx.log(
                Level::Info,
                "duplicate sign-in undone",
                &[
                    ("profile", Field::Profile(&id)),
                    ("existing", Field::Profile(&existing.id)),
                ],
            );
            let name = self.name(&existing.id).unwrap_or_default();
            self.notice = Some(format!("Already added: {name}"));
            let failed = format!("Already added: {name} · the new sign-in couldn't be undone");
            self.discard(ctx, &id, &config_dir, Some(failed));
            return;
        }
        let mismatched = target
            .as_ref()
            .is_some_and(|target| identity.key != target.pair);
        // Cross-checked before the Limit monitor reads it.
        let tag = Tag {
            id: id.clone(),
            then: Then::ConfirmSignIn {
                identity: identity.clone(),
                keep_notice: mismatched,
            },
        };
        self.jobs.push(&config_dir, JobKind::AuthStatus, tag);
        if let Some(target) = target.filter(|_| mismatched) {
            // Kept: it is a real Profile, just not the one asked for.
            ctx.log(
                Level::Info,
                "sign-in matched another account",
                &[("profile", Field::Profile(&id))],
            );
            self.notice = Some(format!(
                "Signed in as {}, not {} · to add it, pick that Login and Organization in the browser",
                identity.name(),
                target.name
            ));
        }
        ctx.log(
            Level::Info,
            "profile added",
            &[("profile", Field::Profile(&id))],
        );
        // The Limit monitor reads it once the cross-check ends, and hands
        // the result to `record_reading`.
        self.profiles.push(Profile {
            id,
            config_dir,
            identity: Some(identity),
            expired: false,
        });
        self.save(ctx);
    }

    /// "Sign in again…" exited successfully. The same Login and Organization
    /// (or, for a signed-out Profile, any Login not already listed) brings
    /// the Profile back; any other is undone like a duplicate, and the
    /// Profile keeps its Login and stays expired.
    fn finish_again(&mut self, ctx: &Context, sign_in: SignIn) {
        let SignIn {
            id,
            config_dir,
            again,
            ..
        } = sign_in;
        let Again {
            expected,
            previous_account,
        } = again.expect("only called for a sign-in again");
        let Some(index) = self.profiles.iter().position(|profile| profile.id == id) else {
            return;
        };
        let Some(identity) = Identity::read(&config_dir) else {
            self.notice = Some("Sign-in didn't complete · nothing changed".into());
            return;
        };
        let accepted = match &expected {
            Some(expected) => expected.key == identity.key,
            None => self
                .find(&identity)
                .is_none_or(|existing| existing.id == id),
        };
        if !accepted {
            let used = identity.name();
            let wanted = expected.as_ref().map_or_else(
                || {
                    self.find(&identity)
                        .and_then(|existing| self.name(&existing.id))
                        .unwrap_or_default()
                },
                Identity::name,
            );
            ctx.log(
                Level::Info,
                "sign-in again as another login undone",
                &[("profile", Field::Profile(&id))],
            );
            let (undone, failed) = if expected.is_some() {
                (
                    format!("Signed in as {used}, not {wanted} · that sign-in was undone"),
                    format!("Signed in as {used}, not {wanted} · that sign-in couldn't be undone"),
                )
            } else {
                (
                    format!("Already added: {wanted}"),
                    format!("Already added: {wanted} · the new sign-in couldn't be undone"),
                )
            };
            self.notice = Some(undone.clone());
            let tag = Tag {
                id: id.clone(),
                then: Then::UndoSignInAgain {
                    previous_account,
                    name: wanted,
                    undone,
                    failed,
                },
            };
            self.jobs.push(&config_dir, JobKind::SignOut, tag);
            return;
        }
        ctx.log(
            Level::Info,
            "signed in again",
            &[("profile", Field::Profile(&id))],
        );
        let profile = &mut self.profiles[index];
        profile.identity = Some(identity);
        profile.expired = false;
        self.save(ctx);
        // Watched again, so the Limit monitor reads it at once.
    }

    /// "Sign in again…": Claude Code's browser sign-in in the Profile's own
    /// folder (its Keychain slot is tied to that path), with `--email` when
    /// the Login is known. Ignored while another sign-in is running.
    pub(crate) fn sign_in_again(&mut self, ctx: &Context, id: &str) {
        if self.sign_in.is_some() {
            return;
        }
        let Some(profile) = self.profile(id) else {
            return;
        };
        let expected = profile.identity.clone();
        let config_dir = profile.config_dir.clone();
        self.notice = None;
        let previous_account = fs::read(config_dir.join(".claude.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|body| body.get("oauthAccount").cloned());
        let email = expected
            .as_ref()
            .map(|identity| identity.names.email.clone());
        ctx.log(
            Level::Info,
            "sign-in again started",
            &[("profile", Field::Profile(id))],
        );
        match ctx.runner().start_sign_in(
            &config_dir,
            email.as_deref().filter(|email| !email.is_empty()),
        ) {
            Ok(process) => {
                self.sign_in = Some(SignIn {
                    id: id.to_owned(),
                    config_dir,
                    process,
                    started_at: ctx.env.clock.now(),
                    target: None,
                    again: Some(Again {
                        expected,
                        previous_account,
                    }),
                })
            }
            Err(error) => {
                let error = error.to_string();
                ctx.log(
                    Level::Error,
                    "sign-in not started",
                    &[
                        ("profile", Field::Profile(id)),
                        ("error", Field::Text(&error)),
                    ],
                );
                self.notice = Some("Couldn't start Claude Code to sign in".into());
            }
        }
    }

    /// Takes in one usage read of Profile `id`. A read saying
    /// `rate_limits_available: false`, or a failed read (`check_login`)
    /// after which `claude auth status` says `loggedIn: false` while the
    /// folder still holds its `oauthAccount`, means the Login expired: the
    /// last reading is kept (dimmed), polling stops and one notification
    /// goes out per expiry.
    pub(crate) fn record_reading(&mut self, ctx: &Context, done: &Completed) {
        let Some(index) = self
            .profiles
            .iter()
            .position(|profile| profile.id == done.id)
        else {
            return;
        };
        let profile = &self.profiles[index];
        if profile.identity.is_none() {
            return;
        }
        match &done.reply {
            Some(reply) if reply.rate_limits_available == Some(false) => self.expire(ctx, index),
            Some(_) => {}
            None if done.check_login => {
                let tag = Tag {
                    id: done.id.clone(),
                    then: Then::CheckExpiry,
                };
                self.jobs
                    .push(&profile.config_dir, JobKind::AuthStatus, tag);
                self.run_jobs(ctx);
            }
            None => {}
        }
    }

    /// Effects produced since the last call (notifications).
    pub(crate) fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    fn expire(&mut self, ctx: &Context, index: usize) {
        if self.profiles[index].expired {
            return;
        }
        let name = self.name(&self.profiles[index].id).unwrap_or_default();
        let profile = &mut self.profiles[index];
        profile.expired = true;
        ctx.log(
            Level::Warn,
            "login expired",
            &[("profile", Field::Profile(&profile.id))],
        );
        self.effects.push(Effect::Notify {
            title: "Login expired".into(),
            body: format!("{name} needs signing in again. Choose Sign in again… in its menu."),
        });
        self.save(ctx);
    }

    /// The Profile signed in as the same Login and Organization.
    fn find(&self, identity: &Identity) -> Option<&Profile> {
        self.profiles.iter().find(|profile| {
            profile
                .identity
                .as_ref()
                .is_some_and(|known| known.key == identity.key)
        })
    }

    fn has(&self, id: &str) -> bool {
        self.profiles.iter().any(|profile| profile.id == id)
    }

    /// Removes a Profile folder that must not stay: signs it out first if a
    /// login landed in it, and keeps the folder when that sign-out fails
    /// (its Keychain item would be orphaned otherwise; `failed` is then the
    /// notice).
    fn discard(&mut self, ctx: &Context, id: &str, config_dir: &Path, failed: Option<String>) {
        if Identity::read(config_dir).is_none() {
            remove_folder(ctx, id, config_dir);
            return;
        }
        let tag = Tag {
            id: id.to_owned(),
            then: Then::Discard { failed },
        };
        self.jobs.push(config_dir, JobKind::SignOut, tag);
    }

    /// Kills an unfinished sign-in and, for Add Profile…, removes what it
    /// left.
    fn abandon(&mut self, ctx: &Context, mut sign_in: SignIn) {
        if let Err(error) = sign_in.process.kill() {
            let error = error.to_string();
            ctx.log(
                Level::Warn,
                "sign-in not killed",
                &[
                    ("profile", Field::Profile(&sign_in.id)),
                    ("error", Field::Text(&error)),
                ],
            );
        }
        if sign_in.again.is_none() {
            self.discard(ctx, &sign_in.id, &sign_in.config_dir, None);
        }
    }

    fn save(&self, ctx: &Context) {
        let file = RegistryFile {
            profiles: self
                .profiles
                .iter()
                .map(|profile| Entry {
                    id: profile.id.clone(),
                    config_dir: profile.config_dir.clone(),
                    expired: profile.expired,
                })
                .collect(),
        };
        if let Err(error) =
            storage::write_versioned(&ctx.paths.state().join("profiles.json"), &file)
        {
            let error = error.to_string();
            ctx.log(
                Level::Error,
                "profiles record not saved",
                &[("error", Field::Text(&error))],
            );
        }
    }
}

/// Creates `profiles/<id>/` with a fresh random id.
fn create_folder(paths: &AppPaths) -> io::Result<(String, PathBuf)> {
    loop {
        let mut bytes = [0u8; 6];
        // SAFETY: arc4random_buf fills exactly `bytes.len()` bytes.
        unsafe { libc::arc4random_buf(bytes.as_mut_ptr().cast(), bytes.len()) };
        let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        let config_dir = paths.profiles().join(&id);
        match fs::create_dir(&config_dir) {
            Ok(()) => return Ok((id, config_dir)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

/// Makes the Profile folder's missing links back to `~/.claude`.
fn link(ctx: &Context, id: &str, config_dir: &Path) {
    if ctx.is_paused(SafetyFeature::Links) {
        ctx.log(
            Level::Info,
            "links not made: paused",
            &[("profile", Field::Profile(id))],
        );
        return;
    }
    match cli_switcher::ensure_links(config_dir, &ctx.env.home.join(".claude")) {
        Ok(blocked) => {
            for blocked in blocked {
                ctx.log(
                    Level::Warn,
                    "link blocked by a file",
                    &[
                        ("profile", Field::Profile(id)),
                        ("path", Field::Path(&blocked.path)),
                    ],
                );
            }
        }
        Err(error) => {
            let error = error.to_string();
            ctx.log(
                Level::Error,
                "links not made",
                &[
                    ("profile", Field::Profile(id)),
                    ("error", Field::Text(&error)),
                ],
            );
        }
    }
}

/// Whether a sign-out run succeeded; logs a failure.
fn signed_out(ctx: &Context, id: &str, finished: &Finished) -> bool {
    let succeeded = finished.succeeded();
    if !succeeded {
        ctx.log(
            Level::Warn,
            "sign-out failed, folder kept",
            &[("profile", Field::Profile(id))],
        );
    }
    succeeded
}

/// Why a Profile's Login wasn't put back.
enum NotRestored {
    /// Seeding `.claude.json` is under a Safety pause.
    Paused,
    /// The file isn't a JSON object: its format is unknown, so it is left
    /// as it is (ADR 0004).
    Unreadable,
    Failed(io::Error),
}

impl NotRestored {
    /// The reason, for the notice "…, but <reason>, so its Login wasn't put
    /// back".
    fn reason(&self, name: &str) -> String {
        match self {
            NotRestored::Paused => format!("{} is paused", SafetyFeature::Seeding.name()),
            NotRestored::Unreadable => format!("{name}'s .claude.json couldn't be read"),
            NotRestored::Failed(_) => format!("{name}'s .claude.json couldn't be written"),
        }
    }
}

/// Puts a Profile's `oauthAccount` back into its `.claude.json` after another
/// Login's sign-in there was undone (sign-out deletes it), so the Profile
/// keeps its Login. Backed up first, written atomically; never while
/// Seeding is paused, and never over a file that isn't a JSON object.
fn restore_account(
    ctx: &Context,
    id: &str,
    config_dir: &Path,
    account: &Value,
) -> Result<(), NotRestored> {
    let result = if ctx.is_paused(SafetyFeature::Seeding) {
        Err(NotRestored::Paused)
    } else {
        write_account(ctx, &config_dir.join(".claude.json"), account)
    };
    if let Err(not_restored) = &result {
        let reason = match not_restored {
            NotRestored::Paused => "paused".to_owned(),
            NotRestored::Unreadable => "unreadable claude.json".to_owned(),
            NotRestored::Failed(error) => error.to_string(),
        };
        ctx.log(
            Level::Warn,
            "account not restored",
            &[
                ("profile", Field::Profile(id)),
                ("error", Field::Text(&reason)),
            ],
        );
    }
    result
}

fn write_account(ctx: &Context, path: &Path, account: &Value) -> Result<(), NotRestored> {
    let mut body = match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            Ok(body @ Value::Object(_)) => {
                backups::back_up(&ctx.paths.backups(), path, ctx.env.clock.now())
                    .map_err(NotRestored::Failed)?;
                body
            }
            _ => return Err(NotRestored::Unreadable),
        },
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Value::Object(serde_json::Map::new())
        }
        Err(error) => return Err(NotRestored::Failed(error)),
    };
    body.as_object_mut()
        .expect("an object, checked above")
        .insert("oauthAccount".into(), account.clone());
    let bytes =
        serde_json::to_vec_pretty(&body).map_err(|error| NotRestored::Failed(error.into()))?;
    storage::write_atomically(path, &bytes).map_err(NotRestored::Failed)
}

/// Deletes a Profile folder without following its links.
fn remove_folder(ctx: &Context, id: &str, config_dir: &Path) -> bool {
    match fs::remove_dir_all(config_dir) {
        Ok(()) => true,
        Err(error) => {
            let error = error.to_string();
            ctx.log(
                Level::Error,
                "profile folder not deleted",
                &[
                    ("profile", Field::Profile(id)),
                    ("error", Field::Text(&error)),
                ],
            );
            false
        }
    }
}
