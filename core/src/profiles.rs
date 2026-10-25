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

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::cli_switcher;
use crate::env::{Clock, Environment, RunningProcess};
use crate::interface::{Command, DiscoveredPair, LimitBar, MenuItem, ProfileRow};
use crate::limit_offers;
use crate::logging::{Field, Level, Logger};
use crate::runner::Runner;
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
}

impl Context<'_> {
    fn log(&self, level: Level, message: &'static str, fields: &[(&'static str, Field<'_>)]) {
        self.logger
            .write(self.env.clock.now(), self.verbose, level, message, fields);
    }

    fn runner(&self) -> Runner<'_> {
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
    /// The menu bar's short name for it (`Perso`).
    pub short_label: String,
    pub row: ProfileRow,
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
}

impl VersionedFile for RegistryFile {
    const VERSION: u64 = 1;
}

/// The outcome of the latest usage read.
#[derive(Debug, Clone, PartialEq)]
enum Reading {
    Read(UsageReply),
    Failed,
}

struct Profile {
    id: String,
    config_dir: PathBuf,
    identity: Option<Identity>,
    reading: Option<Reading>,
}

impl Profile {
    fn row(&self, identity: &Identity, now: Timestamp, tz: &TimeZone) -> ProfileRow {
        let mut limits = Vec::new();
        let mut note = None;
        let mut offers = Vec::new();
        let mut plan = identity
            .organization_type
            .as_deref()
            .map(|kind| kind.strip_prefix("claude_").unwrap_or(kind).to_owned());
        match &self.reading {
            None => {}
            Some(Reading::Read(reply)) => {
                offers = limit_offers::lines(
                    reply.banked_reset.as_ref(),
                    reply.session_refill.as_ref(),
                    now,
                    tz,
                );
                if let Some(kind) = &reply.subscription_type {
                    plan = Some(kind.clone());
                }
                match &reply.limits {
                    Some(read) => {
                        limits.extend(read.all().map(|(label, percent)| LimitBar {
                            label: label.to_owned(),
                            percent: percent.max(0.0).floor() as u32,
                        }));
                    }
                    None => note = Some("Couldn't read limits".to_owned()),
                }
            }
            Some(Reading::Failed) => note = Some("Couldn't read limits".to_owned()),
        }
        let mut detail = identity.name();
        if let Some(plan) = plan.filter(|plan| !plan.is_empty()) {
            detail.push_str(" · ");
            detail.push_str(&capitalized(&plan));
        }
        ProfileRow {
            title: identity.name(),
            badges: Vec::new(),
            command: None,
            limits,
            note,
            offers,
            submenu: vec![
                MenuItem::Detail(detail),
                MenuItem::Action {
                    title: "Limit resets → claude.ai".to_owned(),
                    command: Command::OpenLimitResets,
                },
            ],
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
    target: Option<SignInTarget>,
}

/// Every Profile, plus the sign-in in progress, if any.
#[derive(Default)]
pub(crate) struct ProfileRegistry {
    profiles: Vec<Profile>,
    sign_in: Option<SignIn>,
    /// The outcome of the last sign-in, shown as a menu line until the next.
    notice: Option<String>,
}

impl ProfileRegistry {
    /// Loads the Profiles at launch: recorded folders that still exist, plus
    /// any signed-in folder missing from the record (recorded now). A folder
    /// with no record and no sign-in is what an interrupted Add Profile left
    /// behind, and is deleted. Missing links are made again.
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
                    reading: None,
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
                        reading: None,
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
        for profile in &registry.profiles {
            link(ctx, &profile.id, &profile.config_dir);
        }
        registry
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
            abandon(ctx, sign_in);
            self.notice = Some("Sign-in timed out after 10 minutes · nothing was added".into());
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
                if output.succeeded() {
                    self.finish(ctx, sign_in);
                } else {
                    discard(ctx, &sign_in.id, &sign_in.config_dir);
                    self.notice = Some("Sign-in didn't complete · nothing was added".into());
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

    /// Every signed-in Profile, in order, with its row.
    pub(crate) fn listed(&self, clock: &dyn Clock) -> Vec<Listed<'_>> {
        let (now, tz) = (clock.now(), clock.time_zone());
        self.profiles
            .iter()
            .filter_map(|profile| {
                let identity = profile.identity.as_ref()?;
                Some(Listed {
                    id: &profile.id,
                    config_dir: &profile.config_dir,
                    short_label: identity.short_label(),
                    row: profile.row(identity, now, &tz),
                })
            })
            .collect()
    }

    /// A sign-in exited successfully: reads who signed in, undoes a duplicate,
    /// otherwise records the Profile and reads its limits.
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
            _ => {
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
        let reading = match ctx.runner().read_usage(&config_dir) {
            Some(reply) => Reading::Read(reply),
            None => {
                ctx.log(
                    Level::Warn,
                    "limits unreadable",
                    &[("profile", Field::Profile(&id))],
                );
                Reading::Failed
            }
        };
        ctx.log(
            Level::Info,
            "profile added",
            &[("profile", Field::Profile(&id))],
        );
        self.profiles.push(Profile {
            id,
            config_dir,
            identity: Some(identity),
            reading: Some(reading),
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

/// Kills an unfinished sign-in and removes what it left.
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
    discard(ctx, &sign_in.id, &sign_in.config_dir);
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
