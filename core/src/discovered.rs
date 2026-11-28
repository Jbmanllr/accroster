//! Discovered Profiles: (account uuid, Organization uuid) pairs the Mac
//! already has evidence of, offered greyed with a one-click sign-in.
//!
//! Evidence is only:
//!
//! - a desktop Sidebar folder `claude-code-sessions/<account>/<org>/` holding
//!   at least one readable Sidebar entry (`local_*.json`), and
//! - `oauthAccount` in `~/.claude.json` (today's default CLI login).
//!
//! Nothing else counts: not `~/.claude/.claude.json`, backups, id-only caches
//! or logs. Only `oauthAccount` carries names, so most Sidebar pairs are an
//! "Unknown Login" shown with their Session count and last activity. The
//! desktop's `config.json` adds one fact, `lastKnownAccountUuid`, read on its
//! own (the same file holds tokens, which are never touched).
//!
//! The menu lists a Sidebar pair only while its account has no signed-in
//! Profile: another Organization of a signed-in account is the same Login.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;

use crate::claude_files::{self, ClaudePaths, MAX_ENTRY_BYTES};
use crate::claude_json;
use crate::desktop_account;
use crate::interface::{Command, DiscoveredRow, MenuItem, NicknameTarget, ProfileKey};
use crate::names::LoginNames;
use crate::stamp::Stamp;
use crate::times;

/// One discovered pair and what the Mac says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Evidence {
    pub pair: ProfileKey,
    /// Names from an `oauthAccount` for this exact pair, if any.
    login: Option<Login>,
    /// Readable Sidebar entries in the pair's folder.
    sessions: usize,
    /// The newest `lastActivityAt` (else `createdAt`) among those entries.
    last_activity: Option<Timestamp>,
    /// The account is the desktop's `lastKnownAccountUuid`.
    desktop_last: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Login {
    names: LoginNames,
    /// When Claude Code last refreshed these names (`profileFetchedAt`), else
    /// when `~/.claude.json` was last written.
    as_of: Option<Timestamp>,
}

impl Evidence {
    /// The names of the default CLI login (`~/.claude.json`'s
    /// `oauthAccount`), when this pair is it: no other evidence carries
    /// names.
    pub fn default_login(&self) -> Option<&LoginNames> {
        self.login.as_ref().map(|login| &login.names)
    }

    /// The email to pass to the sign-in, when known.
    pub fn email(&self) -> Option<&str> {
        self.login
            .as_ref()
            .map(|login| login.names.email.as_str())
            .filter(|email| !email.is_empty())
    }

    /// The Login's Profile name, else "Unknown Login". A uuid appears only
    /// in hover detail.
    pub fn known_name(&self) -> String {
        match &self.login {
            Some(login) => login.names.name(),
            None => "Unknown Login".to_owned(),
        }
    }

    /// The nickname, else the Profile name, else "Unknown Login".
    pub fn name(&self, nickname: Option<&str>) -> String {
        nickname.map_or_else(|| self.known_name(), str::to_owned)
    }

    /// The Settings > Hidden entry's title: the name, with what tells an
    /// Unknown Login apart ("Unknown Login · 3 Sessions · last active Oct
    /// 24"), dates in `tz`.
    pub fn hidden_title(&self, nickname: Option<&str>, tz: &TimeZone) -> String {
        match (nickname, self.evidence_line(tz)) {
            (None, Some(line)) => format!("{} · {line}", self.known_name()),
            _ => self.name(nickname),
        }
    }

    /// What tells an Unknown Login apart: its Sessions, last activity and
    /// whether it is the desktop's last Login. `None` for a named Login.
    fn evidence_line(&self, tz: &TimeZone) -> Option<String> {
        self.login.is_none().then(|| {
            let mut parts = vec![match self.sessions {
                1 => "1 Session".to_owned(),
                n => format!("{n} Sessions"),
            }];
            if let Some(last) = self.last_activity {
                parts.push(format!("last active {}", times::date(last, tz)));
            }
            if self.desktop_last {
                parts.push("Desktop's last Login".to_owned());
            }
            parts.join(" · ")
        })
    }

    /// The menu row, with dates in `tz`.
    pub fn row(&self, nickname: Option<&str>, tz: &TimeZone) -> DiscoveredRow {
        let name = self.name(nickname);
        let title = match (nickname, &self.login) {
            (Some(_), Some(login)) => {
                format!(
                    "Sign in to {name} ({})…",
                    login.names.email_and_organization()
                )
            }
            _ => format!("Sign in to {name}…"),
        };
        let subtitle = self.evidence_line(tz);
        let mut submenu = Vec::new();
        if let Some(as_of) = self.login.as_ref().and_then(|login| login.as_of) {
            submenu.push(MenuItem::Detail(format!(
                "as of {}",
                times::date_and_clock(as_of, tz)
            )));
        }
        submenu.push(MenuItem::Detail(format!(
            "Account {} · Organization {}",
            short(&self.pair.account_uuid),
            short(&self.pair.organization_uuid)
        )));
        submenu.extend([
            MenuItem::Separator,
            MenuItem::Action {
                title: "Rename…".into(),
                command: Command::Rename(NicknameTarget::Discovered(self.pair.clone())),
            },
            MenuItem::Action {
                title: "Hide".into(),
                command: Command::HideDiscovered(self.pair.clone()),
            },
        ]);
        DiscoveredRow {
            title,
            subtitle,
            command: Command::SignInDiscovered(self.pair.clone()),
            submenu,
        }
    }
}

/// What the Sidebar folders held at the last scan, so a scan reads only what
/// changed since. Claude writes entries temp-file-then-rename, so any write
/// changes its folder's mtime: a folder whose mtime is unchanged is not
/// listed again, and in a changed one only new or rewritten entries (by
/// their [`Stamp`]) are read.
#[derive(Debug, Default)]
pub(crate) struct Scanner {
    folders: BTreeMap<PathBuf, Folder>,
}

#[derive(Debug)]
struct Folder {
    /// The folder's mtime when it was listed.
    modified: SystemTime,
    /// Every `local_*.json` by name: its stamp, and whether it was readable
    /// with its newest activity.
    entries: BTreeMap<OsString, (Stamp, Option<Activity>)>,
}

/// A readable entry's `lastActivityAt` (else `createdAt`).
type Activity = Option<Timestamp>;

impl Scanner {
    /// Reads every discovered pair under `home`: the `~/.claude.json` login
    /// first, then the most recently active.
    pub(crate) fn scan(&mut self, home: &Path) -> Vec<Evidence> {
        let root = ClaudePaths::new(home).sidebar_root();
        let pairs = self.sidebar_pairs(&root);
        evidence(home, pairs)
    }

    /// `<account>/<org>/` folders holding at least one readable Sidebar
    /// entry, with their entry count and newest activity.
    fn sidebar_pairs(&mut self, root: &Path) -> Vec<(ProfileKey, usize, Option<Timestamp>)> {
        let mut known = std::mem::take(&mut self.folders);
        let mut pairs = Vec::new();
        for found in claude_files::sidebar_folders(root) {
            let path = found.path;
            // Taken before listing: a write during the listing changes
            // it, so the next scan lists again.
            let Some(modified) = fs::metadata(&path).and_then(|m| m.modified()).ok() else {
                continue;
            };
            let folder = match known.remove(&path) {
                Some(folder) if folder.modified == modified => folder,
                previous => Folder {
                    modified,
                    entries: list(&path, previous.map(|f| f.entries).unwrap_or_default()),
                },
            };
            let readable: Vec<Activity> = folder
                .entries
                .values()
                .filter_map(|(_, activity)| *activity)
                .collect();
            if !readable.is_empty() {
                pairs.push((
                    ProfileKey {
                        account_uuid: found.account,
                        organization_uuid: found.organization,
                    },
                    readable.len(),
                    readable.into_iter().flatten().max(),
                ));
            }
            self.folders.insert(path, folder);
        }
        pairs
    }
}

/// A folder's `local_*.json` entries, reusing `known` for those whose stamp
/// is unchanged and reading the rest.
fn list(
    folder: &Path,
    mut known: BTreeMap<OsString, (Stamp, Option<Activity>)>,
) -> BTreeMap<OsString, (Stamp, Option<Activity>)> {
    claude_files::sidebar_entry_files(folder)
        .into_iter()
        .filter_map(|(name, meta)| {
            let stamp = Stamp::of(&meta)?;
            let path = folder.join(&name);
            let name = OsString::from(name);
            let activity = match known.remove(&name) {
                Some((was, activity)) if was == stamp => activity,
                _ => read_activity(&path, &meta),
            };
            Some((name, (stamp, activity)))
        })
        .collect()
}

/// A readable entry's newest activity; `None` when it isn't readable.
fn read_activity(path: &Path, meta: &fs::Metadata) -> Option<Activity> {
    if meta.len() == 0 || meta.len() > MAX_ENTRY_BYTES {
        return None;
    }
    let body: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    Some(
        ["lastActivityAt", "createdAt"]
            .iter()
            .find_map(|key| body.get(key).and_then(Value::as_i64))
            .and_then(|ms| Timestamp::from_millisecond(ms).ok()),
    )
}

/// The Sidebar `pairs` plus the `~/.claude.json` login, ordered.
fn evidence(home: &Path, pairs: Vec<(ProfileKey, usize, Option<Timestamp>)>) -> Vec<Evidence> {
    let desktop_last = desktop_account::last_known_account(&ClaudePaths::new(home));
    let mut found: BTreeMap<ProfileKey, Evidence> = BTreeMap::new();
    for (pair, sessions, last_activity) in pairs {
        found.insert(
            pair.clone(),
            Evidence {
                desktop_last: desktop_last.as_deref() == Some(pair.account_uuid.as_str()),
                pair,
                login: None,
                sessions,
                last_activity,
            },
        );
    }
    let cli = cli_login(&ClaudePaths::new(home).default_config_file());
    if let Some((pair, login)) = &cli {
        found
            .entry(pair.clone())
            .or_insert_with(|| Evidence {
                desktop_last: desktop_last.as_deref() == Some(pair.account_uuid.as_str()),
                pair: pair.clone(),
                login: None,
                sessions: 0,
                last_activity: None,
            })
            .login = Some(login.clone());
    }
    let cli_pair = cli.map(|(pair, _)| pair);
    let mut all: Vec<Evidence> = found.into_values().collect();
    all.sort_by(|a, b| {
        let first = |e: &Evidence| cli_pair.as_ref() != Some(&e.pair);
        first(a)
            .cmp(&first(b))
            .then(b.last_activity.cmp(&a.last_activity))
            .then(a.pair.cmp(&b.pair))
    });
    all
}

/// `oauthAccount` in `~/.claude.json`.
fn cli_login(path: &Path) -> Option<(ProfileKey, Login)> {
    let account = claude_json::read_account(path)?;
    let as_of = account.profile_fetched_at.or_else(|| {
        let modified = fs::metadata(path).ok()?.modified().ok()?;
        Timestamp::try_from(modified).ok()
    });
    Some((
        account.key,
        Login {
            names: account.names,
            as_of,
        },
    ))
}

/// The first 8 characters of a uuid.
fn short(uuid: &str) -> &str {
    uuid.get(..8).unwrap_or(uuid)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(ms: i64) -> Option<Activity> {
        Some(Some(Timestamp::from_millisecond(ms).unwrap()))
    }

    /// A menu open must stay cheap with thousands of entries: an entry
    /// whose stamp is unchanged keeps what was read from it, and only new
    /// or rewritten ones are read.
    #[test]
    fn a_listing_reads_only_new_and_rewritten_entries() {
        let dir = tempfile::tempdir().unwrap();
        let folder = dir.path();
        fs::write(folder.join("local_a.json"), r#"{"lastActivityAt":1000}"#).unwrap();
        fs::write(folder.join("local_b.json"), r#"{"lastActivityAt":2000}"#).unwrap();
        let mut known = list(folder, BTreeMap::new());
        assert_eq!(known[&OsString::from("local_a.json")].1, at(1000));
        // What the files don't hold: reading one again would lose this.
        known.get_mut(&OsString::from("local_a.json")).unwrap().1 = at(5);
        known.get_mut(&OsString::from("local_b.json")).unwrap().1 = at(5);

        let rewritten = folder.join("local_b.json.tmp");
        fs::write(&rewritten, r#"{"lastActivityAt":3000}"#).unwrap();
        fs::rename(&rewritten, folder.join("local_b.json")).unwrap();
        fs::write(folder.join("local_c.json"), r#"{"createdAt":4000}"#).unwrap();
        let listed = list(folder, known);

        assert_eq!(listed[&OsString::from("local_a.json")].1, at(5));
        assert_eq!(listed[&OsString::from("local_b.json")].1, at(3000));
        assert_eq!(listed[&OsString::from("local_c.json")].1, at(4000));
        assert_eq!(listed.len(), 3);
    }
}
