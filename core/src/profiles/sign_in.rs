//! The sign-in flows, which the registry drives: Add Profile… and "Sign in
//! again…". Each is one running `claude auth login` in a Profile folder,
//! polled until it ends, then checked against the account it was meant for.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::{SignedDuration, Timestamp};
use serde_json::Value;

use super::removal::remove_folder;
use super::{Identity, Profile, ProfileRegistry, Tag, Then, link};
use crate::backups;
use crate::claude_files;
use crate::context::Context;
use crate::env::RunningProcess;
use crate::interface::{ProfileKey, SafetyFeature};
use crate::logging::{Field, Level};
use crate::profile_folder;
use crate::runner::{AuthStatus, Finished, JobKind};
use crate::storage::{self, AppPaths};

/// A sign-in still running after this long is killed.
pub(crate) const SIGN_IN_TIMEOUT: SignedDuration = SignedDuration::from_mins(10);

/// The account a sign-in started from a Discovered Profile is meant to add.
pub(crate) struct SignInTarget {
    pub pair: ProfileKey,
    /// Passed as `--email` when known.
    pub email: Option<String>,
    /// How the menu names it ("X" in "Signed in as Y, not X").
    pub name: String,
}

/// A sign-in waiting for the user in the browser.
pub(super) struct SignIn {
    pub(super) id: String,
    pub(super) config_dir: PathBuf,
    pub(super) process: Box<dyn RunningProcess>,
    pub(super) started_at: Timestamp,
    /// The Discovered Profile Add Profile… was aimed at, if any.
    pub(super) target: Option<SignInTarget>,
    /// Set for "Sign in again…" in an existing Profile's folder.
    pub(super) again: Option<Again>,
}

/// What a sign-in in an existing Profile's folder must match or put back.
pub(super) struct Again {
    /// The Login the Profile had; `None` for a signed-out Profile.
    pub(super) expected: Option<Identity>,
    /// Its `oauthAccount`, restored when another Login's sign-in is undone
    /// so the Profile stays expired rather than signed out.
    pub(super) previous_account: Option<Value>,
}

/// Creates `profiles/<id>/` with a fresh random id.
pub(super) fn create_folder(paths: &AppPaths) -> io::Result<(String, PathBuf)> {
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

/// Why a Profile's Login wasn't put back.
pub(super) enum NotRestored {
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
    pub(super) fn reason(&self, name: &str) -> String {
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
pub(super) fn restore_account(
    ctx: &Context,
    id: &str,
    config_dir: &Path,
    account: &Value,
) -> Result<(), NotRestored> {
    let result = if ctx.is_paused(SafetyFeature::Seeding) {
        Err(NotRestored::Paused)
    } else {
        write_account(ctx, &claude_files::config_file(config_dir), account)
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

pub(super) fn write_account(
    ctx: &Context,
    path: &Path,
    account: &Value,
) -> Result<(), NotRestored> {
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

impl ProfileRegistry {
    /// A new Profile's sign-in, cross-checked: an answer that doesn't name
    /// the folder's account gets a notice, and a logged-in one naming
    /// another account counts against Claude Code honouring Profile folders.
    pub(super) fn confirm_sign_in(
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
        // A new click replaces a sign-in still waiting: its browser page
        // may be lost, and only one runs at a time.
        self.cancel(ctx);
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
        } else if let Err(error) = profile_folder::seed_claude_json(&config_dir, &ctx.env.home) {
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

    /// A sign-in exited successfully: reads who signed in, undoes a duplicate,
    /// otherwise records the Profile (the Limit monitor reads it next).
    pub(super) fn finish(&mut self, ctx: &Context, sign_in: SignIn) {
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
    pub(super) fn finish_again(&mut self, ctx: &Context, sign_in: SignIn) {
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
    /// the Login is known. Replaces a sign-in still waiting.
    pub(crate) fn sign_in_again(&mut self, ctx: &Context, id: &str) {
        if self.profile(id).is_none() {
            return;
        }
        self.cancel(ctx);
        let Some(profile) = self.profile(id) else {
            return;
        };
        let expected = profile.identity.clone();
        let config_dir = profile.config_dir.clone();
        self.notice = None;
        let previous_account = fs::read(claude_files::config_file(&config_dir))
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

    /// Removes a Profile folder that must not stay: signs it out first if a
    /// login landed in it, and keeps the folder when that sign-out fails
    /// (its Keychain item would be orphaned otherwise; `failed` is then the
    /// notice).
    pub(super) fn discard(
        &mut self,
        ctx: &Context,
        id: &str,
        config_dir: &Path,
        failed: Option<String>,
    ) {
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
    pub(super) fn abandon(&mut self, ctx: &Context, mut sign_in: SignIn) {
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
}
