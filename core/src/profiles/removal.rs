//! Remove…: signs the Profile out, then deletes its folder and forgets it.
//! Also removes the folders under `profiles/` that aren't Profiles
//! (Uninstall…).

use std::fs;
use std::path::{Path, PathBuf};

use super::{Identity, ProfileRegistry, Tag, Then};
use crate::context::Context;
use crate::interface::{Command, Effect};
use crate::logging::{Field, Level};
use crate::runner::{Finished, JobKind};

/// Whether a sign-out run succeeded; logs a failure.
pub(super) fn signed_out(ctx: &Context, id: &str, finished: &Finished) -> bool {
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

/// Deletes a Profile folder without following its links.
pub(super) fn remove_folder(ctx: &Context, id: &str, config_dir: &Path) -> bool {
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

impl ProfileRegistry {
    /// Whether Remove… is signing Profile `id` out.
    pub(super) fn removing(&self, id: &str) -> bool {
        self.jobs
            .queued()
            .any(|(_, tag)| tag.id == id && matches!(tag.then, Then::Remove))
    }

    /// Remove…'s sign-out ended: on success the folder is deleted and the
    /// Profile forgotten; otherwise both stay.
    pub(super) fn removal_ended(&mut self, ctx: &Context, id: &str, signed_out: bool) {
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
}
