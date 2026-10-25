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

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use jiff::Timestamp;
use jiff::tz::TimeZone;
use serde_json::Value;

use crate::interface::{Command, DiscoveredPair, DiscoveredRow, MenuItem, NicknameTarget};

/// The desktop loader skips Sidebar entries over this size.
const MAX_ENTRY_BYTES: u64 = 10 * 1024 * 1024;

/// One discovered pair and what the Mac says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Evidence {
    pub pair: DiscoveredPair,
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
    email: String,
    organization_name: String,
    /// When Claude Code last refreshed these names (`profileFetchedAt`), else
    /// when `~/.claude.json` was last written.
    as_of: Option<Timestamp>,
}

impl Evidence {
    /// The email to pass to the sign-in, when known.
    pub fn email(&self) -> Option<&str> {
        self.login
            .as_ref()
            .map(|login| login.email.as_str())
            .filter(|email| !email.is_empty())
    }

    /// `email · Organization`, else "Unknown Login (account 55555555)".
    pub fn known_name(&self) -> String {
        match &self.login {
            Some(login) => format!("{} · {}", login.email, login.organization_name),
            None => format!("Unknown Login (account {})", short(&self.pair.account_uuid)),
        }
    }

    /// The nickname, else `email · Organization`, else "Unknown Login".
    fn name(&self, nickname: Option<&str>) -> String {
        match (nickname, &self.login) {
            (Some(nickname), _) => nickname.to_owned(),
            (None, Some(login)) => format!("{} · {}", login.email, login.organization_name),
            (None, None) => "Unknown Login".to_owned(),
        }
    }

    /// The Settings > Hidden entry's title.
    pub fn hidden_title(&self, nickname: Option<&str>) -> String {
        match nickname {
            Some(nickname) => nickname.to_owned(),
            None => self.known_name(),
        }
    }

    /// The menu row, with dates in `tz`.
    pub fn row(&self, nickname: Option<&str>, tz: &TimeZone) -> DiscoveredRow {
        let name = self.name(nickname);
        let title = match (nickname, &self.login) {
            (Some(_), Some(login)) => format!(
                "Sign in to {name} ({} · {})…",
                login.email, login.organization_name
            ),
            _ => format!("Sign in to {name}…"),
        };
        let subtitle = self.login.is_none().then(|| {
            let mut parts = vec![match self.sessions {
                1 => "1 Session".to_owned(),
                n => format!("{n} Sessions"),
            }];
            if let Some(last) = self.last_activity {
                parts.push(format!("last active {}", local(last, tz, "%b %-d")));
            }
            if self.desktop_last {
                parts.push("Desktop's last account".to_owned());
            }
            parts.join(" · ")
        });
        let mut submenu = Vec::new();
        if let Some(as_of) = self.login.as_ref().and_then(|login| login.as_of) {
            submenu.push(MenuItem::Detail(format!(
                "as of {}",
                local(as_of, tz, "%b %-d, %H:%M")
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

/// Reads every discovered pair under `home`: the `~/.claude.json` login first,
/// then the most recently active.
pub(crate) fn scan(home: &Path) -> Vec<Evidence> {
    let desktop = home.join("Library/Application Support/Claude");
    let desktop_last = last_known_account(&desktop.join("config.json"));
    let mut found: BTreeMap<DiscoveredPair, Evidence> = BTreeMap::new();
    for (pair, sessions, last_activity) in sidebar_pairs(&desktop.join("claude-code-sessions")) {
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
    let cli = cli_login(&home.join(".claude.json"));
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

/// `<account>/<org>/` folders holding at least one readable Sidebar entry,
/// with their entry count and newest activity.
fn sidebar_pairs(root: &Path) -> Vec<(DiscoveredPair, usize, Option<Timestamp>)> {
    let mut pairs = Vec::new();
    for account in subfolders(root) {
        for org in subfolders(&root.join(&account)) {
            let folder = root.join(&account).join(&org);
            let entries: Vec<Value> = fs::read_dir(&folder)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|item| {
                    let name = item.file_name();
                    let name = name.to_string_lossy();
                    name.starts_with("local_") && name.ends_with(".json")
                })
                .filter_map(|item| {
                    let meta = item.metadata().ok()?;
                    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_ENTRY_BYTES {
                        return None;
                    }
                    serde_json::from_slice(&fs::read(item.path()).ok()?).ok()
                })
                .collect();
            if entries.is_empty() {
                continue;
            }
            let last_activity = entries
                .iter()
                .filter_map(|body| {
                    ["lastActivityAt", "createdAt"]
                        .iter()
                        .find_map(|key| body.get(key).and_then(Value::as_i64))
                })
                .filter_map(|ms| Timestamp::from_millisecond(ms).ok())
                .max();
            pairs.push((
                DiscoveredPair {
                    account_uuid: account.clone(),
                    organization_uuid: org,
                },
                entries.len(),
                last_activity,
            ));
        }
    }
    pairs
}

fn subfolders(path: &Path) -> Vec<String> {
    fs::read_dir(path)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|item| item.file_name().to_string_lossy().into_owned())
        .collect()
}

/// `oauthAccount` in `~/.claude.json`.
fn cli_login(path: &Path) -> Option<(DiscoveredPair, Login)> {
    let bytes = fs::read(path).ok()?;
    let body: Value = serde_json::from_slice(&bytes).ok()?;
    let account = body.get("oauthAccount")?;
    let text = |key: &str| account.get(key).and_then(Value::as_str).map(str::to_owned);
    let as_of = account
        .get("profileFetchedAt")
        .and_then(Value::as_i64)
        .and_then(|ms| Timestamp::from_millisecond(ms).ok())
        .or_else(|| {
            let modified = fs::metadata(path).ok()?.modified().ok()?;
            Timestamp::try_from(modified).ok()
        });
    Some((
        DiscoveredPair {
            account_uuid: text("accountUuid")?,
            organization_uuid: text("organizationUuid")?,
        },
        Login {
            email: text("emailAddress").unwrap_or_default(),
            organization_name: text("organizationName").unwrap_or_default(),
            as_of,
        },
    ))
}

/// `lastKnownAccountUuid` from the desktop's `config.json`. Only that key is
/// read; the file also holds tokens.
fn last_known_account(path: &Path) -> Option<String> {
    let body: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    body.get("lastKnownAccountUuid")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The first 8 characters of a uuid.
fn short(uuid: &str) -> &str {
    uuid.get(..8).unwrap_or(uuid)
}

fn local(time: Timestamp, tz: &TimeZone, format: &str) -> String {
    time.to_zoned(tz.clone()).strftime(format).to_string()
}
