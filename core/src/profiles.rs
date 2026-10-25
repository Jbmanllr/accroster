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
use crate::cli_switcher;
use crate::env::{Environment, RunningProcess};
use crate::interface::{Command, DiscoveredPair, Effect, MenuItem, NicknameTarget, ProfileRow};
use crate::logging::{Field, Level, Logger};
use crate::monitor::{LimitsView, Watched};
use crate::runner::Runner;
use crate::settings::Settings;
use crate::storage::{self, AppPaths, ReadError, VersionedFile};
use crate::usage::UsageReply;

/// A sign-in still running after this long is killed.
pub(crate) const SIGN_IN_TIMEOUT: SignedDuration = SignedDuration::from_mins(10);

/// What the registry needs from the core for one call.
pub(crate) struct Context<'a> {
    pub env: &'a Environment,
    pub paths: &'a AppPaths,
    pub logger: &'a Logger,
    pub verbose: bool,
    /// Whether the links sharing Transcripts may be made (Transcript sharing
    /// isn't under a Safety pause).
    pub share_transcripts: bool,
}

impl Context<'_> {
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
        Runner::new(self.env, self.paths)
    }
}

/// A Login acting within an Organization, from `oauthAccount` in a Profile
/// folder's `.claude.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    account_uuid: String,
    organization_uuid: String,
    email: String,
    organization_name: String,
    /// For example `claude_max`; the plan when no usage read has said.
    organization_type: Option<String>,
}

impl Identity {
    /// `oauthAccount` of `<config_dir>/.claude.json`, or `None` when the
    /// folder holds no sign-in.
    fn read(config_dir: &Path) -> Option<Identity> {
        let bytes = fs::read(config_dir.join(".claude.json")).ok()?;
        let body: Value = serde_json::from_slice(&bytes).ok()?;
        let account = body.get("oauthAccount")?;
        let text = |key: &str| account.get(key).and_then(Value::as_str).map(str::to_owned);
        Some(Identity {
            account_uuid: text("accountUuid")?,
            organization_uuid: text("organizationUuid")?,
            email: text("emailAddress").unwrap_or_default(),
            organization_name: text("organizationName").unwrap_or_default(),
            organization_type: text("organizationType"),
        })
    }

    fn same_profile(&self, other: &Identity) -> bool {
        self.account_uuid == other.account_uuid && self.organization_uuid == other.organization_uuid
    }

    fn is(&self, pair: &DiscoveredPair) -> bool {
        self.account_uuid == pair.account_uuid && self.organization_uuid == pair.organization_uuid
    }

    /// `email · Organization`.
    fn name(&self) -> String {
        format!("{} · {}", self.email, self.organization_name)
    }

    /// The menu bar's short name: the email's local part.
    fn short_label(&self) -> String {
        self.email.split('@').next().unwrap_or_default().to_owned()
    }
}

/// A signed-in Profile as the menu lists it.
pub(crate) struct Listed<'a> {
    pub id: &'a str,
    /// The folder's absolute path, verbatim.
    pub config_dir: &'a Path,
    /// Whether its folder holds a Login (signed in or expired).
    pub signed_in: bool,
    /// The menu bar's short name for it: the nickname, else the email's
    /// local part (`perso`).
    pub short_label: String,
    pub row: ProfileRow,
}

/// A signed-in Profile's folder and the account its `oauthAccount` names.
pub(crate) struct SignedInAccount<'a> {
    pub config_dir: &'a Path,
    pub email: &'a str,
    pub organization_uuid: &'a str,
}

/// A signed-in Profile as other modules see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SignedInProfile {
    pub id: String,
    pub account_uuid: String,
    pub organization_uuid: String,
    /// `email · Organization`.
    pub name: String,
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
    /// The menu row: `title` is the nickname, else `email · Organization`;
    /// `position` and `count` decide which Move items the submenu offers.
    fn row(&self, title: String, position: usize, count: usize, limits: LimitsView) -> ProfileRow {
        let mut submenu = Vec::new();
        let mut row_limits = LimitsView::default();
        if let Some(identity) = &self.identity {
            let plan = limits.plan.clone().or_else(|| {
                identity
                    .organization_type
                    .as_deref()
                    .map(|kind| kind.strip_prefix("claude_").unwrap_or(kind).to_owned())
            });
            let mut detail = identity.name();
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
        if position > 0 {
            submenu.push(action(
                "Move Up",
                Command::MoveProfile {
                    id: self.id.clone(),
                    to: position - 1,
                },
            ));
        }
        if position + 1 < count {
            submenu.push(action(
                "Move Down",
                Command::MoveProfile {
                    id: self.id.clone(),
                    to: position + 1,
                },
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
            offers,
            submenu,
        }
    }

    /// `email · Organization`, or what the row says when no Login is known.
    fn name(&self) -> String {
        self.identity
            .as_ref()
            .map_or_else(|| "Signed-out Profile".to_owned(), Identity::name)
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
    pub pair: DiscoveredPair,
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

/// Every Profile, plus the sign-in in progress, if any.
#[derive(Default)]
pub(crate) struct ProfileRegistry {
    profiles: Vec<Profile>,
    sign_in: Option<SignIn>,
    /// The outcome of the last sign-in, shown as a menu line until the next.
    notice: Option<String>,
    /// Set when `auth status` in a newly signed-in folder named another
    /// account than its `oauthAccount`; taken by the format checks.
    auth_disagreed: bool,
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

    /// Every signed-in Profile's folder with the account its `oauthAccount`
    /// names, for the `auth status` cross-check.
    pub(crate) fn accounts(&self) -> Vec<SignedInAccount<'_>> {
        self.profiles
            .iter()
            .filter_map(|profile| {
                let identity = profile.identity.as_ref()?;
                Some(SignedInAccount {
                    config_dir: &profile.config_dir,
                    email: &identity.email,
                    organization_uuid: &identity.organization_uuid,
                })
            })
            .collect()
    }

    /// Whether `auth status` contradicted a sign-in since the last call.
    pub(crate) fn take_auth_disagreement(&mut self) -> bool {
        std::mem::take(&mut self.auth_disagreed)
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
        if let Err(error) = cli_switcher::seed_claude_json(&config_dir, &ctx.env.home) {
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
            abandon(ctx, sign_in);
        }
    }

    /// Checks on the running sign-in: finishes it once Claude Code has exited,
    /// or abandons it after [`SIGN_IN_TIMEOUT`].
    pub(crate) fn poll(&mut self, ctx: &Context) {
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
            abandon(ctx, sign_in);
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
                        discard(ctx, &sign_in.id, &sign_in.config_dir);
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
                abandon(ctx, sign_in);
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
    pub(crate) fn has_signed_in(&self, pair: &DiscoveredPair) -> bool {
        self.profiles.iter().any(|profile| {
            profile
                .identity
                .as_ref()
                .is_some_and(|identity| identity.is(pair))
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
        let listed = self.ordered(&settings.profile_order);
        listed
            .iter()
            .enumerate()
            .map(|(position, profile)| {
                let nickname = settings.nicknames.get(&profile.id);
                let title = nickname.cloned().unwrap_or_else(|| profile.name());
                let short_label = nickname
                    .cloned()
                    .or_else(|| profile.identity.as_ref().map(Identity::short_label))
                    .unwrap_or_else(|| "Signed out".to_owned());
                Listed {
                    id: &profile.id,
                    config_dir: &profile.config_dir,
                    signed_in: profile.identity.is_some(),
                    short_label,
                    row: profile.row(title, position, listed.len(), limits(&profile.id)),
                }
            })
            .collect()
    }

    /// The Profiles the Limit monitor reads, in the user's order: signed in
    /// and not expired. `active` says, by id and folder, which are a
    /// client's Active Profile.
    pub(crate) fn watched(&self, active: impl Fn(&str, &Path) -> bool) -> Vec<Watched> {
        self.profiles
            .iter()
            .filter(|profile| !profile.expired)
            .filter_map(|profile| {
                let identity = profile.identity.as_ref()?;
                Some(Watched {
                    id: profile.id.clone(),
                    config_dir: profile.config_dir.clone(),
                    account_uuid: identity.account_uuid.clone(),
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
        let profile = self.profile(id)?;
        Some(Effect::AskNickname {
            name: profile.name(),
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
        let profile = self.profile(id)?;
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
            title: format!("Remove {}?", profile.name()),
            message,
            confirm,
        })
    }

    /// Removes Profile `id`: `claude auth logout` first, then its folder
    /// (links are deleted, never followed, so shared Transcripts stay). A
    /// failed sign-out keeps the folder and the Profile. Returns whether it
    /// was removed.
    pub(crate) fn remove(&mut self, ctx: &Context, id: &str) -> bool {
        let Some(index) = self.profiles.iter().position(|profile| profile.id == id) else {
            return false;
        };
        if self
            .sign_in
            .as_ref()
            .is_some_and(|sign_in| sign_in.id == id)
        {
            return false;
        }
        self.notice = None;
        let profile = &self.profiles[index];
        let signed_out = matches!(ctx.runner().sign_out(&profile.config_dir), Ok(true));
        if !signed_out {
            ctx.log(
                Level::Warn,
                "sign-out failed, profile kept",
                &[("profile", Field::Profile(id))],
            );
            self.notice = Some(format!(
                "Couldn't sign out {} · nothing was removed",
                profile.name()
            ));
            return false;
        }
        if !remove_folder(ctx, id, &profile.config_dir) {
            self.notice = Some(format!(
                "Signed out {}, but its folder couldn't be deleted",
                profile.name()
            ));
        }
        self.profiles.remove(index);
        self.save(ctx);
        ctx.log(
            Level::Info,
            "profile removed",
            &[("profile", Field::Profile(id))],
        );
        true
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
        self.profiles
            .iter()
            .filter_map(|profile| {
                let identity = profile.identity.as_ref()?;
                Some(SignedInProfile {
                    id: profile.id.clone(),
                    account_uuid: identity.account_uuid.clone(),
                    organization_uuid: identity.organization_uuid.clone(),
                    name: identity.name(),
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
            discard(ctx, &id, &config_dir);
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
            let name = existing
                .identity
                .as_ref()
                .map(Identity::name)
                .unwrap_or_default();
            self.notice = Some(if discard(ctx, &id, &config_dir) {
                format!("Already added: {name}")
            } else {
                format!("Already added: {name} · the new sign-in couldn't be undone")
            });
            return;
        }
        match ctx.runner().auth_status(&config_dir) {
            Some(status)
                if status.logged_in
                    && status.email.as_deref() == Some(identity.email.as_str())
                    && status.org_id.as_deref() == Some(identity.organization_uuid.as_str()) => {}
            status => {
                // A readable, logged-in answer naming another account means
                // Claude Code didn't use this folder's login.
                self.auth_disagreed = status.is_some_and(|status| status.logged_in);
                ctx.log(
                    Level::Warn,
                    "sign-in not confirmed by auth status",
                    &[("profile", Field::Profile(&id))],
                );
                self.notice = Some(format!(
                    "Claude Code didn't confirm the sign-in of {}",
                    identity.name()
                ));
            }
        }
        if let Some(target) = target.filter(|target| !identity.is(&target.pair)) {
            // Kept: it is a real Profile, just not the one asked for.
            ctx.log(
                Level::Info,
                "sign-in matched another account",
                &[("profile", Field::Profile(&id))],
            );
            self.notice = Some(format!(
                "Signed in as {}, not {} · to add it, pick that account and Organization in the browser",
                identity.name(),
                target.name
            ));
        }
        ctx.log(
            Level::Info,
            "profile added",
            &[("profile", Field::Profile(&id))],
        );
        // The Limit monitor reads it next, and hands the result to
        // `record_reading`.
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
            Some(expected) => expected.same_profile(&identity),
            None => self
                .find(&identity)
                .is_none_or(|existing| existing.id == id),
        };
        if !accepted {
            let used = identity.name();
            let wanted = expected.as_ref().map_or_else(
                || self.find(&identity).map(Profile::name).unwrap_or_default(),
                Identity::name,
            );
            ctx.log(
                Level::Info,
                "sign-in again as another login undone",
                &[("profile", Field::Profile(&id))],
            );
            let undone = matches!(ctx.runner().sign_out(&config_dir), Ok(true));
            if undone && let Some(account) = &previous_account {
                restore_account(ctx, &id, &config_dir, account);
            }
            self.notice = Some(match (expected.is_some(), undone) {
                (true, true) => {
                    format!("Signed in as {used}, not {wanted} · that sign-in was undone")
                }
                (true, false) => {
                    format!("Signed in as {used}, not {wanted} · that sign-in couldn't be undone")
                }
                (false, true) => format!("Already added: {wanted}"),
                (false, false) => {
                    format!("Already added: {wanted} · the new sign-in couldn't be undone")
                }
            });
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
        let email = expected.as_ref().map(|identity| identity.email.clone());
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

    /// Takes in one usage read of Profile `id` (`None` when it failed). A
    /// read saying `rate_limits_available: false`, or a failed read after
    /// which `claude auth status` says `loggedIn: false` while the folder
    /// still holds its `oauthAccount`, means the Login expired: the last
    /// reading is kept (dimmed), polling stops and one notification goes
    /// out per expiry.
    pub(crate) fn record_reading(&mut self, ctx: &Context, id: &str, reply: Option<UsageReply>) {
        let Some(index) = self.profiles.iter().position(|profile| profile.id == id) else {
            return;
        };
        let profile = &self.profiles[index];
        if profile.identity.is_none() {
            return;
        }
        let expired = match &reply {
            Some(reply) => reply.rate_limits_available == Some(false),
            None => ctx
                .runner()
                .auth_status(&profile.config_dir)
                .is_some_and(|status| !status.logged_in),
        };
        if expired {
            self.expire(ctx, index);
        }
    }

    /// Effects produced since the last call (notifications).
    pub(crate) fn take_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    fn expire(&mut self, ctx: &Context, index: usize) {
        let profile = &mut self.profiles[index];
        if profile.expired {
            return;
        }
        profile.expired = true;
        ctx.log(
            Level::Warn,
            "login expired",
            &[("profile", Field::Profile(&profile.id))],
        );
        self.effects.push(Effect::Notify {
            title: "Login expired".into(),
            body: format!(
                "{} needs signing in again. Choose Sign in again… in its menu.",
                profile.name()
            ),
        });
        self.save(ctx);
    }

    /// The Profile signed in as the same Login and Organization.
    fn find(&self, identity: &Identity) -> Option<&Profile> {
        self.profiles.iter().find(|profile| {
            profile
                .identity
                .as_ref()
                .is_some_and(|known| known.same_profile(identity))
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
    match cli_switcher::ensure_links(
        config_dir,
        &ctx.env.home.join(".claude"),
        ctx.share_transcripts,
    ) {
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

/// Kills an unfinished sign-in and, for Add Profile…, removes what it left.
fn abandon(ctx: &Context, mut sign_in: SignIn) {
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
        discard(ctx, &sign_in.id, &sign_in.config_dir);
    }
}

/// Puts a Profile's `oauthAccount` back into its `.claude.json` after another
/// Login's sign-in there was undone (sign-out deletes it), so the Profile
/// keeps its Login. Backed up first, written atomically.
fn restore_account(ctx: &Context, id: &str, config_dir: &Path, account: &Value) {
    let path = config_dir.join(".claude.json");
    let result = (|| -> io::Result<()> {
        let mut body = match fs::read(&path) {
            Ok(bytes) => {
                backups::back_up(&ctx.paths.backups(), &path, ctx.env.clock.now())?;
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Value::Null,
            Err(error) => return Err(error),
        };
        if !body.is_object() {
            body = Value::Object(serde_json::Map::new());
        }
        body.as_object_mut()
            .expect("made an object above")
            .insert("oauthAccount".into(), account.clone());
        storage::write_atomically(&path, &serde_json::to_vec_pretty(&body)?)
    })();
    if let Err(error) = result {
        let error = error.to_string();
        ctx.log(
            Level::Error,
            "account not restored",
            &[
                ("profile", Field::Profile(id)),
                ("error", Field::Text(&error)),
            ],
        );
    }
}

/// Removes a Profile folder that must not stay: signs it out first if a
/// login landed in it, and keeps the folder when that sign-out fails (its
/// Keychain item would be orphaned otherwise). Returns whether it is gone.
fn discard(ctx: &Context, id: &str, config_dir: &Path) -> bool {
    if Identity::read(config_dir).is_some() {
        match ctx.runner().sign_out(config_dir) {
            Ok(true) => {}
            Ok(false) | Err(_) => {
                ctx.log(
                    Level::Warn,
                    "sign-out failed, folder kept",
                    &[("profile", Field::Profile(id))],
                );
                return false;
            }
        }
    }
    remove_folder(ctx, id, config_dir)
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
