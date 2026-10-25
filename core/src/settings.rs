//! `settings.json`: the user's choices.
//!
//! Profile folders decide which Profiles exist; settings only add choices on
//! top (Profile order, nicknames, thresholds and mutes, …). Losing
//! this file therefore costs at most those choices, never a Profile or a
//! login.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::interface::SidebarSyncMode;
use crate::storage::{self, ReadError, VersionedFile};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Settings {
    /// While set and in the future, logging is verbose. Turning Verbose
    /// logging on sets it 24 hours ahead, so it turns itself off.
    #[serde(default)]
    pub verbose_logging_until: Option<Timestamp>,
    /// The user's last launch-at-login choice in the app. `None` until the
    /// first-run offer is answered, so the offer is made once.
    #[serde(default)]
    pub launch_at_login: Option<bool>,
    /// Profile nicknames, by Profile id.
    #[serde(default)]
    pub nicknames: BTreeMap<String, String>,
    /// Profile ids, top first. Profiles missing from it follow, in the order
    /// they were added.
    #[serde(default)]
    pub profile_order: Vec<String>,
    /// Nicknames and Hide choices for Discovered Profiles, by pair. Kept
    /// while the pair is off the Mac, so a returning pair stays hidden.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub discovered_profiles: Vec<DiscoveredChoice>,
    /// Sidebar sync's mode; Off until chosen.
    #[serde(default)]
    pub sidebar_sync: SidebarSyncMode,
    /// The global notification thresholds, in whole percent, lowest first.
    #[serde(default)]
    pub thresholds: Thresholds,
    /// Notify when a limit that warned resets. Off by default.
    #[serde(default)]
    pub reset_notification: bool,
    /// Profile ids whose limit notifications are muted.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub muted_profiles: Vec<String>,
}

/// Notification thresholds in whole percent (1–100), sorted, without
/// repeats. Default 80% and 95%.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Thresholds(Vec<u32>);

impl Thresholds {
    /// Keeps the values in 1–100, sorted, without repeats.
    pub fn new(mut values: Vec<u32>) -> Thresholds {
        values.retain(|value| (1..=100).contains(value));
        values.sort_unstable();
        values.dedup();
        Thresholds(values)
    }

    pub fn values(&self) -> &[u32] {
        &self.0
    }
}

impl Default for Thresholds {
    fn default() -> Thresholds {
        Thresholds(vec![80, 95])
    }
}

/// The user's choices for one Discovered Profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DiscoveredChoice {
    pub account_uuid: String,
    pub organization_uuid: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nickname: Option<String>,
    #[serde(default)]
    pub hidden: bool,
}

impl VersionedFile for Settings {
    const VERSION: u64 = 1;
}

/// The outcome of loading settings at launch.
pub(crate) struct Loaded {
    pub settings: Settings,
    /// Set when an unparsable file was renamed aside; the app warns once.
    pub set_aside: Option<SetAside>,
}

/// An unparsable `settings.json` that was renamed `settings.corrupt-<time>.json`.
pub(crate) struct SetAside {
    pub path: PathBuf,
    /// What was wrong with it (may quote the file's contents; log it redacted).
    pub reason: String,
}

/// Loads `settings.json`. A missing file gives defaults (first run). An
/// unparsable one is renamed `settings.corrupt-<time>.json` and defaults are
/// written in its place, so the next launch starts clean and doesn't warn.
pub(crate) fn load(path: &Path, now: Timestamp) -> io::Result<Loaded> {
    match storage::read_versioned::<Settings>(path) {
        Ok(Some(settings)) => Ok(Loaded {
            settings,
            set_aside: None,
        }),
        Ok(None) => {
            let settings = Settings::default();
            storage::write_versioned(path, &settings)?;
            Ok(Loaded {
                settings,
                set_aside: None,
            })
        }
        Err(ReadError::Io(error)) => Err(error),
        Err(ReadError::Unparsable(reason)) => {
            let aside = corrupt_path(path, now);
            fs::rename(path, &aside)?;
            let settings = Settings::default();
            storage::write_versioned(path, &settings)?;
            Ok(Loaded {
                settings,
                set_aside: Some(SetAside {
                    path: aside,
                    reason,
                }),
            })
        }
    }
}

/// `settings.corrupt-20261025T140200Z.json`, with `-2`, `-3`, … if taken.
fn corrupt_path(path: &Path, now: Timestamp) -> PathBuf {
    let stamp = now.strftime("%Y%m%dT%H%M%SZ").to_string();
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut candidate = dir.join(format!("settings.corrupt-{stamp}.json"));
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!("settings.corrupt-{stamp}-{n}.json"));
        n += 1;
    }
    candidate
}
