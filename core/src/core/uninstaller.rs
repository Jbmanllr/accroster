//! The uninstaller: what the confirmation says, and the steps it runs in
//! order, reusing CLI switching off, Profile removal and the login item:
//!
//! 1. Unhook the CLI (as CLI switching off): new and open terminals reach
//!    the real `claude` while every Profile folder still exists.
//! 2. Remove each Profile (sign out, then delete). A failed sign-out keeps
//!    that folder and stops the uninstall here, so no login is orphaned.
//! 3. Unregister launch at login.
//! 4. Delete the app's folder permanently.
//! 5. Move the app to the Trash (the UI, [`crate::Effect::TrashAppAndQuit`]).
//!
//! Each step does nothing when already done, so running it again after an
//! interruption finishes the job.

use std::fs;
use std::io;

use super::Core;
use crate::cli_switcher;
use crate::env::LoginItemStatus;
use crate::interface::Effect;
use crate::logging::{Field, Level};
use crate::storage::AppPaths;

/// The confirmation's title.
pub(super) const TITLE: &str = "Uninstall AccRoster?";

/// The confirmation's text: what is removed, what stays, and what may not
/// have happened. `profiles` is how many Profiles will be signed out.
pub(super) fn confirmation(profiles: usize) -> String {
    let profiles = match profiles {
        0 => "No Profiles to sign out".to_owned(),
        1 => "1 Profile".to_owned(),
        n => format!("{n} Profiles"),
    };
    format!(
        "Removes, in this order:\n\
         • CLI switching: the claude shim and AccRoster's block in ~/.zshrc. claude then runs as it did before.\n\
         • {profiles}: each is signed out, then its folder is deleted, with its own prompt history and the MCP servers and folder trust added after it was set up. Transcripts in ~/.claude stay.\n\
         • Launch at login.\n\
         • AccRoster's folder (settings, logs, backups, the Session index), deleted permanently.\n\
         • The app, moved to the Trash.\n\
         \n\
         Stays:\n\
         • Sidebar entries that Sidebar sync copied into other Profiles' sidebars.\n\
         • Folders AccRoster created in ~/.claude.\n\
         • The notification permission (System Settings → Notifications).\n\
         \n\
         Signing out asks Anthropic to revoke each login; offline, that revoke may not have happened, though the login is deleted from this Mac. \
         If a sign-out fails, that Profile's folder is kept and Uninstall stops so you can retry."
    )
}

/// Deletes the app's folder and everything in it. Already gone is done.
fn delete_app_folder(paths: &AppPaths) -> io::Result<()> {
    match fs::remove_dir_all(paths.root()) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}

impl Core {
    /// Runs the uninstall's steps in order (see this module), each
    /// skipped when already done. Stops, with a note and Retry in Settings,
    /// where going on could break `claude` or orphan a login. Steps 3 to 5
    /// wait for step 2's sign-outs ([`Core::continue_uninstall`]).
    pub(super) fn uninstall(&mut self) -> Vec<Effect> {
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
    pub(super) fn continue_uninstall(&mut self) -> Vec<Effect> {
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
        if let Err(error) = delete_app_folder(&self.paths) {
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
    pub(super) fn stop_uninstall(&mut self, note: String) -> Vec<Effect> {
        self.log(Level::Warn, "uninstall stopped", &[]);
        self.uninstall_note = Some(note);
        vec![Effect::ShowSettings]
    }
}
