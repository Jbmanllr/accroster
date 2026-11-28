//! The Profile registry: which Profiles exist, each one's menu row and its
//! latest Limit reading. It drives two flows kept in their own files: the
//! browser sign-ins ([`sign_in`]) and Remove… ([`removal`]).
//!
//! A Profile is a folder `profiles/<id>/` with a fixed id. It is never renamed
//! or moved, because its Keychain item is a hash of the literal path, so the
//! absolute path is stored once in `state/profiles.json` and reused verbatim.
//! Profile folders decide which Profiles exist; the file only records each
//! one's path.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::claude_files::{self, ClaudePaths};
use crate::claude_json;
use crate::context::Context;
use crate::interface::{
    Command, Effect, MenuItem, NicknameTarget, ProfileKey, ProfileRow, SafetyFeature,
};
use crate::logging::{Field, Level};
use crate::monitor::{Completed, LimitsView, Watched};
use crate::names::{self, LoginNames};
use crate::profile_folder;
use crate::runner::{AuthStatus, Ended, Finished, JobKind, Jobs};
use crate::settings::Settings;
use crate::storage::{self, ReadError, VersionedFile};

mod removal;
mod sign_in;

use self::removal::{remove_folder, signed_out};
pub(crate) use self::sign_in::SignInTarget;
use self::sign_in::{SignIn, restore_account};

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
        let account = claude_json::read_account(&claude_files::config_file(config_dir))?;
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

/// Each Profile's Short label, in `profiles`' order.
fn short_labels(profiles: &[&Profile], settings: &Settings) -> Vec<String> {
    names::short_labels(profiles.iter().map(|profile| {
        let nickname = settings.nicknames.get(&profile.id).map(String::as_str);
        let login = profile.identity.as_ref().map(|identity| &identity.names);
        (nickname, login)
    }))
}

/// Each Profile's Profile name, in `profiles`' order.
fn profile_names(profiles: &[&Profile]) -> Vec<String> {
    names::profile_names(
        profiles
            .iter()
            .map(|profile| profile.identity.as_ref().map(|identity| &identity.names)),
    )
}

/// A signed-in Profile as the menu lists it.
pub(crate) struct Listed<'a> {
    pub id: &'a str,
    /// The folder's absolute path, verbatim.
    pub config_dir: &'a Path,
    /// Whether its folder holds a Login (signed in or expired).
    pub signed_in: bool,
    /// Its Profile name (see [`names::profile_names`]).
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
    /// Its Profile name (see [`names::profile_names`]).
    pub name: String,
    /// Its Organization as people call it (see
    /// [`LoginNames::organization_label`]); empty when unknown.
    pub organization_label: String,
    /// Its Organization is its personal default: no other account is in it.
    pub personal_organization: bool,
}

/// Each signed-in Profile's personal Organization, with the one account that
/// can be in it: Claude lets no one else join a personal Organization.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct PersonalOrganizations(BTreeMap<String, String>);

impl PersonalOrganizations {
    /// Whether the Sidebar folder of `account` in `organization` is what an
    /// account switch in the desktop app left behind: another account paired
    /// with a Profile's personal Organization (ADR 0003). Such a folder is
    /// read, never listed and never written into.
    pub(crate) fn is_leftover(&self, account: &str, organization: &str) -> bool {
        self.0
            .get(organization)
            .is_some_and(|owner| owner != account)
    }
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
    /// The menu row: `title` is the nickname, else the Profile name;
    /// `muted` says whether its limit notifications are muted.
    fn row(&self, title: String, limits: LimitsView, muted: bool) -> ProfileRow {
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
        // Only a Profile with a Login has limits to be notified about.
        if self.identity.is_some() {
            submenu.push(MenuItem::Check {
                title: "Mute notifications".to_owned(),
                checked: muted,
                command: Command::SetMuted {
                    id: self.id.clone(),
                    muted: !muted,
                },
            });
        }
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
        let names = profile_names(&ordered);
        ordered
            .into_iter()
            .zip(short_labels.into_iter().zip(names))
            .map(|(profile, (short_label, name))| {
                let nickname = settings.nicknames.get(&profile.id);
                let title = nickname.cloned().unwrap_or_else(|| name.clone());
                let muted = settings.muted_profiles.contains(&profile.id);
                let mut row = profile.row(title, limits(&profile.id), muted);
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

    /// A Profile's name, as its row has it without a nickname (see
    /// [`names`]), or "Signed-out Profile".
    pub(crate) fn name(&self, id: &str) -> Option<String> {
        let all: Vec<&Profile> = self.profiles.iter().collect();
        let index = all.iter().position(|profile| profile.id == id)?;
        profile_names(&all).into_iter().nth(index)
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

    /// The personal Organizations of the signed-in Profiles.
    pub(crate) fn personal_organizations(&self) -> PersonalOrganizations {
        PersonalOrganizations(
            self.signed_in()
                .into_iter()
                .filter(|profile| profile.personal_organization)
                .map(|profile| (profile.key.organization_uuid, profile.key.account_uuid))
                .collect(),
        )
    }

    /// Who each signed-in Profile is, in add order.
    pub(crate) fn signed_in(&self) -> Vec<SignedInProfile> {
        let all: Vec<&Profile> = self.profiles.iter().collect();
        all.iter()
            .zip(profile_names(&all))
            .filter_map(|(profile, name)| {
                let identity = profile.identity.as_ref()?;
                Some(SignedInProfile {
                    id: profile.id.clone(),
                    key: identity.key.clone(),
                    name,
                    organization_label: identity.names.organization_label(),
                    personal_organization: identity.names.personal_organization(),
                })
            })
            .collect()
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
    match profile_folder::ensure_links(config_dir, &ClaudePaths::new(&ctx.env.home).claude_home()) {
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
