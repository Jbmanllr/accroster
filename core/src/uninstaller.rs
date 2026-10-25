//! The uninstaller's own rules: what the confirmation says, and deleting the
//! app's folder. [`crate::Core`] runs the steps in order, reusing CLI
//! switching off, Profile removal and the login item:
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

use crate::storage::AppPaths;

/// The confirmation's title.
pub(crate) const TITLE: &str = "Uninstall AccRoster?";

/// The confirmation's text: what is removed, what stays, and what may not
/// have happened. `profiles` is how many Profiles will be signed out.
pub(crate) fn confirmation(profiles: usize) -> String {
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
pub(crate) fn delete_app_folder(paths: &AppPaths) -> io::Result<()> {
    match fs::remove_dir_all(paths.root()) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
        _ => Ok(()),
    }
}
