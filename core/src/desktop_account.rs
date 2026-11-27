//! The desktop app's account, as far as its files tell (the desktop's
//! Active Profile).
//!
//! - **The account** is `lastKnownAccountUuid` in the desktop's
//!   `config.json`. That file also holds token caches, so only that one key
//!   is ever deserialized, and the file is never logged.
//! - **The Organization** isn't recorded anywhere: it is the
//!   `claude-code-sessions/<account>/<org>/` folder the app wrote last.
//! - **An account switch** shows as a sign-in line in `main.log`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Deserialize;

use crate::claude_files::{self, ClaudePaths};
use crate::interface::ProfileKey;
use crate::profiles::SignedInProfile;

/// What `main.log` says when Claude finishes signing in.
const SIGNED_IN_LINE: &str = "Login-state transition (loggedOut: true → false";

/// The desktop's account, as far as its files tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DesktopAccount {
    pub account_uuid: String,
    /// The Organization whose folder was written last, if any folder exists.
    pub organization_uuid: Option<String>,
}

impl DesktopAccount {
    /// The Profile the desktop uses, when its Organization is known.
    pub(crate) fn key(&self) -> Option<ProfileKey> {
        Some(ProfileKey {
            account_uuid: self.account_uuid.clone(),
            organization_uuid: self.organization_uuid.clone()?,
        })
    }

    /// Whether the desktop uses Profile `key`.
    pub(crate) fn is(&self, key: &ProfileKey) -> bool {
        self.account_uuid == key.account_uuid
            && self.organization_uuid.as_deref() == Some(key.organization_uuid.as_str())
    }

    /// The signed-in Profile this is, if any. With no folder written yet, an
    /// account that only one Profile has is enough.
    pub(crate) fn profile<'a>(
        &self,
        profiles: &'a [SignedInProfile],
    ) -> Option<&'a SignedInProfile> {
        let same_account = || {
            profiles
                .iter()
                .filter(|profile| profile.key.account_uuid == self.account_uuid)
        };
        match &self.organization_uuid {
            Some(org) => same_account().find(|profile| &profile.key.organization_uuid == org),
            None => {
                let mut matches = same_account();
                let only = matches.next()?;
                matches.next().is_none().then_some(only)
            }
        }
    }
}

/// Only the key the bridge needs; serde skips the rest (token caches
/// included) without keeping them.
#[derive(Deserialize)]
struct DesktopConfig {
    #[serde(rename = "lastKnownAccountUuid")]
    last_known_account_uuid: Option<String>,
}

/// `lastKnownAccountUuid` in the desktop's `config.json`: the one reader of
/// it. `None` when the file is missing or unreadable, or the value isn't a
/// uuid.
pub(crate) fn last_known_account(paths: &ClaudePaths) -> Option<String> {
    let bytes = fs::read(paths.desktop_config()).ok()?;
    let config: DesktopConfig = serde_json::from_slice(&bytes).ok()?;
    config
        .last_known_account_uuid
        .filter(|id| claude_files::is_uuid(id))
}

/// The desktop's account, or `None` when its `config.json` is missing,
/// unreadable or names no account.
pub(crate) fn desktop_account(paths: &ClaudePaths) -> Option<DesktopAccount> {
    NewestEntries::default().desktop_account(paths)
}

/// Each Sidebar folder's newest entry, remembered with the folder's mtime.
/// Entries are written temp-file-then-rename (by Claude and by Sidebar
/// sync), so a folder whose mtime is unchanged has the same newest entry:
/// it isn't listed again. Finding the desktop's Organization otherwise
/// stats every entry in its account's folders, thousands on a busy Mac.
#[derive(Debug, Default)]
pub(crate) struct NewestEntries(RefCell<HashMap<PathBuf, (SystemTime, Option<SystemTime>)>>);

impl NewestEntries {
    /// [`desktop_account`], listing only the folders that changed since the
    /// last call.
    pub(crate) fn desktop_account(&self, paths: &ClaudePaths) -> Option<DesktopAccount> {
        let account_uuid = last_known_account(paths)?;
        let organization_uuid =
            newest_org_folder(&paths.sidebar_root().join(&account_uuid), |folder| {
                self.newest_entry(folder)
            });
        Some(DesktopAccount {
            account_uuid,
            organization_uuid,
        })
    }

    fn newest_entry(&self, folder: &Path) -> Option<SystemTime> {
        // Taken before listing: a write during the listing changes it.
        let Ok(modified) = fs::metadata(folder).and_then(|meta| meta.modified()) else {
            return newest_entry(folder);
        };
        if let Some((at, newest)) = self.0.borrow().get(folder)
            && *at == modified
        {
            return *newest;
        }
        let newest = newest_entry(folder);
        self.0
            .borrow_mut()
            .insert(folder.to_path_buf(), (modified, newest));
        newest
    }
}

/// The Organization folder under an account folder holding the most recently
/// written Sidebar entry, by `newest`.
fn newest_org_folder(
    account_dir: &Path,
    newest: impl Fn(&Path) -> Option<SystemTime>,
) -> Option<String> {
    claude_files::uuid_folders(account_dir)
        .into_iter()
        .filter_map(|(name, folder)| Some((newest(&folder)?, name)))
        .max()
        .map(|(_, name)| name)
}

/// The modification time of the newest `local_*.json` in a folder.
fn newest_entry(folder: &Path) -> Option<SystemTime> {
    claude_files::sidebar_entry_files(folder)
        .into_iter()
        .filter_map(|(_, meta)| meta.modified().ok())
        .max()
}

/// `main.log`'s length now, if it exists.
pub(crate) fn main_log_length(paths: &ClaudePaths) -> Option<u64> {
    fs::metadata(paths.desktop_main_log())
        .ok()
        .map(|meta| meta.len())
}

/// Whether Claude signed in to an account since `main.log` was `length`
/// long (a shorter log was rotated: it counts as a sign-in, to be safe).
pub(crate) fn signed_in_since(paths: &ClaudePaths, length: u64) -> bool {
    match main_log_length(paths) {
        Some(now) if now < length => true,
        _ => log_signed_in_since(&paths.desktop_main_log(), length),
    }
}

/// Whether `main.log` gained a sign-in line after `offset` (read from the
/// start when the log was rotated and is now shorter).
pub(crate) fn log_signed_in_since(log: &Path, offset: u64) -> bool {
    let Ok(mut file) = fs::File::open(log) else {
        return false;
    };
    let len = file.metadata().map_or(0, |meta| meta.len());
    let start = if len < offset { 0 } else { offset };
    let mut appended = Vec::new();
    if file.seek(SeekFrom::Start(start)).is_err() || file.read_to_end(&mut appended).is_err() {
        return false;
    }
    String::from_utf8_lossy(&appended).contains(SIGNED_IN_LINE)
}
