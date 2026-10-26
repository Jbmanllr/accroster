//! The Desktop bridge: everything about the Claude desktop app.
//!
//! - **Active Profile:** the account in the desktop's `config.json`
//!   (`lastKnownAccountUuid`) plus the `claude-code-sessions/<account>/<org>/`
//!   folder the app wrote last. `config.json` also holds token caches, so only
//!   that one key is ever deserialized, and the file is never logged.
//!
//! - **Busy signal:** the live-process files `~/.claude/sessions/<pid>.json`
//!   that the desktop's Claude Code children write (the desktop sets no
//!   `CLAUDE_CONFIG_DIR`). A file counts when its `pid` is alive with a
//!   matching `procStart` (UTC) and its `entrypoint` is `claude-desktop`;
//!   `idle` is idle, anything else (`busy`, `waiting`, unknown) is busy. A
//!   desktop child process without such a file means "can't tell", looked
//!   at again for a few seconds before the user is asked. The `.key` files
//!   next to them are never opened.
//! - **Guided switch:** guard, then (the core) sync the sidebars, then bring
//!   Claude forward with "Sign out in Claude, then sign in as …", then
//!   confirm from the Sidebar folder Claude writes (or `main.log`). Each
//!   step is one status line at the top of the menu, and each outcome a
//!   notification.
//! - **Reopen Claude:** offered after a sync added entries to the sidebar
//!   Claude is using while it runs (Claude reads it only at launch). Guard,
//!   wait while busy, then quit Claude and open it again.
//!
//! AccRoster quits and reopens Claude only for Reopen Claude, which the user
//! clicks, and only once Claude is idle (or the user chose Reopen anyway).

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::{SignedDuration, Timestamp};
use serde::Deserialize;

use crate::env::Environment;
use crate::interface::{ClaudeVersions, Command, Effect, MenuItem, ProfileKey, StatusTone};
use crate::logging::{Field, Level};
use crate::profiles::{Context, SignedInProfile};

/// A Guided switch still unconfirmed after this long is given up.
const CONFIRM_TIMEOUT: SignedDuration = SignedDuration::from_mins(30);

/// What `main.log` says when Claude finishes signing in.
const SIGNED_IN_LINE: &str = "Login-state transition (loggedOut: true → false";

/// Where the desktop app keeps the files the bridge reads, all under `~`.
pub(crate) struct DesktopPaths {
    /// `~/Library/Application Support/Claude`.
    user_data: PathBuf,
    /// `~/Library/Logs/Claude`.
    logs: PathBuf,
    /// `~/.claude`.
    claude_home: PathBuf,
}

impl DesktopPaths {
    pub(crate) fn new(home: &Path) -> DesktopPaths {
        DesktopPaths {
            user_data: home.join("Library/Application Support/Claude"),
            logs: home.join("Library/Logs/Claude"),
            claude_home: home.join(".claude"),
        }
    }

    fn config(&self) -> PathBuf {
        self.user_data.join("config.json")
    }

    fn sidebar_root(&self) -> PathBuf {
        self.user_data.join("claude-code-sessions")
    }

    /// `~/.claude/sessions`, where the desktop's children write.
    fn live_processes(&self) -> PathBuf {
        self.claude_home.join("sessions")
    }

    fn main_log(&self) -> PathBuf {
        self.logs.join("main.log")
    }
}

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
pub(crate) fn last_known_account(paths: &DesktopPaths) -> Option<String> {
    let bytes = fs::read(paths.config()).ok()?;
    let config: DesktopConfig = serde_json::from_slice(&bytes).ok()?;
    config.last_known_account_uuid.filter(|id| is_uuid(id))
}

/// The desktop's account, or `None` when its `config.json` is missing,
/// unreadable or names no account.
pub(crate) fn desktop_account(paths: &DesktopPaths) -> Option<DesktopAccount> {
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
    pub(crate) fn desktop_account(&self, paths: &DesktopPaths) -> Option<DesktopAccount> {
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
    fs::read_dir(account_dir)
        .ok()?
        .flatten()
        .filter_map(|org| {
            let name = org.file_name().into_string().ok().filter(|n| is_uuid(n))?;
            let newest = newest(&org.path())?;
            Some((newest, name))
        })
        .max()
        .map(|(_, name)| name)
}

/// The modification time of the newest `local_*.json` in a folder.
fn newest_entry(folder: &Path) -> Option<SystemTime> {
    sidebar_entries(folder)
        .into_iter()
        .map(|(_, modified)| modified)
        .max()
}

/// Every `local_*.json` in a folder, with its modification time.
fn sidebar_entries(folder: &Path) -> Vec<(String, SystemTime)> {
    let Ok(entries) = fs::read_dir(folder) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            if !(name.starts_with("local_") && name.ends_with(".json")) {
                return None;
            }
            let meta = entry.metadata().ok().filter(|meta| meta.is_file())?;
            Some((name, meta.modified().ok()?))
        })
        .collect()
}

/// A folder name the desktop app could have made from a uuid (also keeps
/// path separators and `..` out of joined paths).
pub(crate) fn is_uuid(text: &str) -> bool {
    text.len() == 36 && text.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// `main.log`'s length now, if it exists.
pub(crate) fn main_log_length(paths: &DesktopPaths) -> Option<u64> {
    fs::metadata(paths.main_log()).ok().map(|meta| meta.len())
}

/// Whether Claude signed in to an account since `main.log` was `length`
/// long (a shorter log was rotated: it counts as a sign-in, to be safe).
pub(crate) fn signed_in_since(paths: &DesktopPaths, length: u64) -> bool {
    match main_log_length(paths) {
        Some(now) if now < length => true,
        _ => log_signed_in_since(&paths.main_log(), length),
    }
}

// ----- Versions --------------------------------------------------------------------

/// The installed Claude versions, from links, folder names and the app's
/// `Info.plist`; nothing is spawned.
pub(crate) fn versions(env: &Environment) -> ClaudeVersions {
    let home = &env.home;
    let claude_code = fs::read_link(home.join(".local/bin/claude"))
        .ok()
        .and_then(|target| target.file_name()?.to_str().map(str::to_owned))
        .filter(|name| version_key(name).is_some());
    let mut desktop_claude_code: Vec<String> =
        fs::read_dir(home.join("Library/Application Support/Claude/claude-code"))
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|entry| entry.path().is_dir())
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .filter(|name| version_key(name).is_some())
                    .collect()
            })
            .unwrap_or_default();
    desktop_claude_code.sort_by_key(|name| version_key(name));
    ClaudeVersions {
        claude_code,
        desktop_claude_code,
        desktop: env.os.desktop_app_version(),
    }
}

/// `2.1.310` as numbers, for ordering; `None` for anything else.
fn version_key(name: &str) -> Option<Vec<u64>> {
    name.split('.').map(|part| part.parse().ok()).collect()
}

// ----- Busy guard ------------------------------------------------------------------

/// What the busy guard found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Guard {
    /// Every desktop Session is idle (or none runs).
    Clear,
    /// These desktop Sessions are running a turn or waiting, by title.
    Busy(Vec<String>),
    /// This many desktop Claude Code children run without a readable
    /// live-process file.
    CantTell(usize),
}

/// The fields of a live-process file the guard reads.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveProcess {
    pid: u32,
    proc_start: Option<String>,
    entrypoint: Option<String>,
    status: Option<String>,
    host_session_id: Option<String>,
    name: Option<String>,
}

/// Checks whether switching accounts now would cut a desktop turn.
pub(crate) fn guard(env: &Environment, paths: &DesktopPaths) -> Guard {
    let mut live: Vec<LiveProcess> = live_desktop_processes(env, paths);
    live.sort_by_key(|process| process.pid);
    let mut busy: Vec<String> = Vec::new();
    for process in &live {
        if process.status.as_deref() == Some("idle") {
            continue;
        }
        let title = session_title(paths, process);
        if !busy.contains(&title) {
            busy.push(title);
        }
    }
    if !busy.is_empty() {
        return Guard::Busy(busy);
    }
    let unaccounted = env
        .os
        .desktop_claude_code_children()
        .into_iter()
        .filter(|pid| !live.iter().any(|process| process.pid == *pid))
        .count();
    if unaccounted > 0 {
        Guard::CantTell(unaccounted)
    } else {
        Guard::Clear
    }
}

/// The live-process files of running desktop children. Stale files (dead pid,
/// or a pid reused by a process that started at another time) are skipped,
/// as are files of other clients. Only `<digits>.json` names are opened.
fn live_desktop_processes(env: &Environment, paths: &DesktopPaths) -> Vec<LiveProcess> {
    let Ok(entries) = fs::read_dir(paths.live_processes()) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let stem = name.strip_suffix(".json")?;
            if stem.is_empty() || !stem.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            let bytes = fs::read(entry.path()).ok()?;
            let process: LiveProcess = serde_json::from_slice(&bytes).ok()?;
            if process.entrypoint.as_deref() != Some("claude-desktop") {
                return None;
            }
            let facts = env.os.process(process.pid)?;
            let started = parse_proc_start(process.proc_start.as_deref()?)?;
            (started.as_second() == facts.started_at.as_second()).then_some(process)
        })
        .collect()
}

/// `procStart` as `ps -o lstart` prints it (`Sun Oct 25 14:02:11 2026`, the
/// day space-padded). Claude Code runs it as `LC_ALL=C TZ=UTC ps …`, so the
/// time is UTC whatever the Mac's time zone.
pub(crate) fn parse_proc_start(text: &str) -> Option<Timestamp> {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [_weekday, month, day, time, year] = parts.as_slice() else {
        return None;
    };
    let month = i8::try_from(MONTHS.iter().position(|m| m == month)? + 1).ok()?;
    let mut clock = time.split(':').map(|part| part.parse::<i8>().ok());
    let (hour, minute, second) = (clock.next()??, clock.next()??, clock.next()??);
    let date = jiff::civil::Date::new(year.parse().ok()?, month, day.parse().ok()?).ok()?;
    let time = jiff::civil::Time::new(hour, minute, second, 0).ok()?;
    let zoned = date
        .to_datetime(time)
        .to_zoned(jiff::tz::TimeZone::UTC)
        .ok()?;
    Some(zoned.timestamp())
}

/// A busy Session's name for the menu: its Sidebar entry's title, else the
/// name Claude Code recorded, quoted. Shown, never logged.
fn session_title(paths: &DesktopPaths, process: &LiveProcess) -> String {
    let from_entry = process
        .host_session_id
        .as_deref()
        .filter(|id| is_sidebar_id(id))
        .and_then(|id| sidebar_title(paths, id));
    match from_entry.or_else(|| process.name.clone()) {
        Some(title) => format!("“{title}”"),
        None => "an untitled Session".into(),
    }
}

/// The `title` of `<id>.json` in any account/Organization folder.
fn sidebar_title(paths: &DesktopPaths, id: &str) -> Option<String> {
    #[derive(Deserialize)]
    struct Entry {
        title: Option<String>,
    }
    let file = format!("{id}.json");
    for account in fs::read_dir(paths.sidebar_root()).ok()?.flatten() {
        let Ok(orgs) = fs::read_dir(account.path()) else {
            continue;
        };
        for org in orgs.flatten() {
            let Ok(bytes) = fs::read(org.path().join(&file)) else {
                continue;
            };
            if let Ok(Entry { title: Some(title) }) = serde_json::from_slice(&bytes) {
                return Some(title);
            }
        }
    }
    None
}

/// `local_<id>` as the desktop app names Sidebar entries.
fn is_sidebar_id(id: &str) -> bool {
    id.strip_prefix("local_").is_some_and(|rest| {
        !rest.is_empty()
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

// ----- Guided switch ---------------------------------------------------------------

/// The Profile a Guided switch moves Claude to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Target {
    pub profile_id: String,
    pub key: ProfileKey,
    /// Its Profile name.
    pub name: String,
    /// The Sidebar entry (`local_…`) to open once the switch is confirmed.
    pub then_open: Option<String>,
}

impl Target {
    pub(crate) fn of(profile: &SignedInProfile) -> Target {
        Target {
            profile_id: profile.id.clone(),
            key: profile.key.clone(),
            name: profile.name.clone(),
            then_open: None,
        }
    }
}

/// An `<account>/<org>` Sidebar folder.
type Folder = ProfileKey;

/// Every Sidebar entry's modification time, by folder.
type Snapshot = BTreeMap<Folder, BTreeMap<String, SystemTime>>;

fn snapshot(paths: &DesktopPaths) -> Snapshot {
    let mut folders = Snapshot::new();
    let Ok(accounts) = fs::read_dir(paths.sidebar_root()) else {
        return folders;
    };
    let uuid_name =
        |entry: &fs::DirEntry| entry.file_name().into_string().ok().filter(|n| is_uuid(n));
    for account in accounts.flatten() {
        let Some(account_uuid) = uuid_name(&account) else {
            continue;
        };
        let Ok(orgs) = fs::read_dir(account.path()) else {
            continue;
        };
        for org in orgs.flatten() {
            let Some(org_uuid) = uuid_name(&org) else {
                continue;
            };
            folders.insert(
                ProfileKey {
                    account_uuid: account_uuid.clone(),
                    organization_uuid: org_uuid,
                },
                sidebar_entries(&org.path()).into_iter().collect(),
            );
        }
    }
    folders
}

enum Step {
    /// The guard couldn't tell at first: it looks again for a few seconds
    /// (a Claude Code process just started writes its file shortly after).
    Checking { target: Target, since: Timestamp },
    /// A desktop turn is running; nothing was done.
    Blocked {
        target: Target,
        sessions: Vec<String>,
        notify_when_idle: bool,
    },
    /// The guard couldn't tell; waiting for "Switch anyway" or "Cancel".
    Unsure { target: Target, reason: String },
    /// The sidebars sync into the target's before Claude is brought forward.
    Syncing { target: Target },
    /// The sync before the switch stopped; waiting for "Switch anyway" or
    /// "Cancel".
    SyncStopped { target: Target, reason: String },
    /// Claude was brought forward; waiting to see which account it writes.
    Waiting {
        target: Target,
        started_at: Timestamp,
        /// The desktop's folder when the switch started: sign-out alone
        /// keeps saving there, so writes to it prove nothing.
        previous: Option<Folder>,
        before: Snapshot,
        /// Where `main.log` ended when the switch started.
        log_offset: u64,
    },
}

/// How long "can't tell" is looked at again before asking the user.
const CANT_TELL_GRACE: SignedDuration = SignedDuration::from_secs(5);

/// What "Switch anyway" lets through.
pub(crate) enum Confirmed {
    /// The guard couldn't tell: sync (when on), then guide.
    Unsure(Target),
    /// The sync before the switch stopped: guide without it.
    SyncStopped(Target),
}

/// The Guided switch in progress, and the outcome of the last one.
#[derive(Default)]
pub(crate) struct DesktopBridge {
    step: Option<Step>,
    outcome: Option<(String, StatusTone)>,
}

impl DesktopBridge {
    /// Whether a switch is under way (from the guard to the confirmation).
    pub(crate) fn active(&self) -> bool {
        self.step.is_some()
    }

    /// Whether a switch is checking the guard again or waiting for Claude to
    /// sign in: the core checks back every second.
    pub(crate) fn waiting(&self) -> bool {
        matches!(
            self.step,
            Some(Step::Waiting { .. } | Step::Checking { .. })
        )
    }

    /// "Switch desktop app to this Profile…": runs the guard. Returns the
    /// target when nothing stands in the way: the caller syncs the sidebars
    /// (spec step 2), then calls [`DesktopBridge::guide`].
    pub(crate) fn start(&mut self, ctx: &Context, target: Target) -> Option<Target> {
        self.outcome = None;
        let paths = DesktopPaths::new(&ctx.env.home);
        match guard(ctx.env, &paths) {
            Guard::Busy(sessions) => {
                self.block(ctx, target, sessions);
                None
            }
            Guard::CantTell(_) => {
                log(
                    ctx,
                    Level::Info,
                    "desktop switch checking",
                    &[profile(&target)],
                );
                self.step = Some(Step::Checking {
                    target,
                    since: ctx.env.clock.now(),
                });
                None
            }
            Guard::Clear => Some(target),
        }
    }

    fn block(&mut self, ctx: &Context, target: Target, sessions: Vec<String>) {
        log(
            ctx,
            Level::Info,
            "desktop switch blocked",
            &[
                profile(&target),
                ("busy_sessions", Field::Number(sessions.len() as i64)),
            ],
        );
        self.step = Some(Step::Blocked {
            target,
            sessions,
            notify_when_idle: false,
        });
    }

    /// "Switch anyway", after "can't tell" or a stopped sync.
    pub(crate) fn confirm(&mut self, ctx: &Context) -> Option<Confirmed> {
        let confirmed = match self.step.take() {
            Some(Step::Unsure { target, .. }) => Confirmed::Unsure(target),
            Some(Step::SyncStopped { target, .. }) => Confirmed::SyncStopped(target),
            other => {
                self.step = other;
                return None;
            }
        };
        log(ctx, Level::Info, "desktop switch confirmed anyway", &[]);
        Some(confirmed)
    }

    /// The sidebars sync into `target`'s before Claude is brought forward.
    pub(crate) fn syncing(&mut self, ctx: &Context, target: Target) {
        log(
            ctx,
            Level::Info,
            "desktop switch syncing",
            &[profile(&target)],
        );
        self.step = Some(Step::Syncing { target });
    }

    /// The target whose sync is under way, if the switch still waits on it.
    pub(crate) fn sync_target(&self) -> Option<&Target> {
        match &self.step {
            Some(Step::Syncing { target }) => Some(target),
            _ => None,
        }
    }

    /// The sync before the switch stopped for `reason`: asks before going on
    /// without it.
    pub(crate) fn sync_stopped(&mut self, ctx: &Context, reason: String) -> Vec<Effect> {
        let Some(Step::Syncing { target }) = self.step.take() else {
            return Vec::new();
        };
        log(
            ctx,
            Level::Warn,
            "desktop switch sync stopped",
            &[profile(&target)],
        );
        let effects = vec![Effect::Notify {
            title: format!("Switch to {} not started", target.name),
            body: format!("Sidebar sync stopped: {reason}"),
        }];
        self.step = Some(Step::SyncStopped { target, reason });
        effects
    }

    /// "Notify me when idle" after a block.
    pub(crate) fn notify_when_idle(&mut self) {
        if let Some(Step::Blocked {
            notify_when_idle, ..
        }) = &mut self.step
        {
            *notify_when_idle = true;
        }
    }

    /// "Cancel": forgets the switch. Claude is left as it is.
    pub(crate) fn cancel(&mut self, ctx: &Context) {
        if self.step.take().is_some() {
            log(ctx, Level::Info, "desktop switch cancelled", &[]);
        }
        self.outcome = None;
    }

    /// Time passed: looks again when the guard couldn't tell, re-checks a
    /// blocked switch the user wants to hear about, and looks for the
    /// account a guided Claude signed in to. Returns the effects and the
    /// target once the guard lets it through.
    pub(crate) fn poll(
        &mut self,
        ctx: &Context,
        profiles: &[SignedInProfile],
    ) -> (Vec<Effect>, Option<Target>) {
        let paths = DesktopPaths::new(&ctx.env.home);
        let now = ctx.env.clock.now();
        match &mut self.step {
            Some(Step::Checking { target, since }) => {
                let target = target.clone();
                let since = *since;
                match guard(ctx.env, &paths) {
                    Guard::Clear => {
                        self.step = None;
                        (Vec::new(), Some(target))
                    }
                    Guard::Busy(sessions) => {
                        self.block(ctx, target, sessions);
                        (Vec::new(), None)
                    }
                    Guard::CantTell(_) if now.duration_since(since) < CANT_TELL_GRACE => {
                        (Vec::new(), None)
                    }
                    Guard::CantTell(unaccounted) => (self.unsure(ctx, target, unaccounted), None),
                }
            }
            Some(Step::Blocked {
                target,
                sessions,
                notify_when_idle: true,
            }) => match guard(ctx.env, &paths) {
                Guard::Busy(now) => {
                    *sessions = now;
                    (Vec::new(), None)
                }
                Guard::Clear | Guard::CantTell(_) => {
                    let body = format!("You can switch Claude to {} now", target.name);
                    self.step = None;
                    log(ctx, Level::Info, "desktop idle", &[]);
                    let effects = vec![Effect::Notify {
                        title: "Claude is idle".into(),
                        body,
                    }];
                    (effects, None)
                }
            },
            Some(Step::Waiting {
                target,
                started_at,
                previous,
                before,
                log_offset,
            }) => {
                let outcome = confirmation(&paths, target, previous.as_ref(), before, *log_offset);
                let timed_out = now.duration_since(*started_at) >= CONFIRM_TIMEOUT;
                let target = target.clone();
                (self.finish(ctx, target, outcome, timed_out, profiles), None)
            }
            _ => (Vec::new(), None),
        }
    }

    /// The guard still can't tell: asks the user, in the menu and in a
    /// notification, so the choice can't go unnoticed.
    fn unsure(&mut self, ctx: &Context, target: Target, unaccounted: usize) -> Vec<Effect> {
        let reason = cant_tell_reason(unaccounted);
        log(
            ctx,
            Level::Info,
            "desktop switch unsure",
            &[
                profile(&target),
                ("unaccounted", Field::Number(unaccounted as i64)),
            ],
        );
        let effects = vec![Effect::Notify {
            title: format!("Switch Claude to {}?", target.name),
            body: format!(
                "Can't tell whether Claude is busy ({reason}). Choose Switch anyway or Cancel in AccRoster's menu."
            ),
        }];
        self.step = Some(Step::Unsure { target, reason });
        effects
    }

    /// The status line and its actions, at the top of the menu. `sync` is
    /// the sync's progress (sidebars written, of how many) while one runs.
    pub(crate) fn menu_items(&self, sync: Option<(usize, usize)>) -> Vec<MenuItem> {
        let status = |text: String, tone| MenuItem::Status { text, tone };
        let action = |title: &str, command| MenuItem::Action {
            title: title.into(),
            command,
        };
        let cancel = || action("Cancel", Command::CancelDesktopSwitch);
        let anyway = || action("Switch anyway", Command::ConfirmDesktopSwitch);
        match &self.step {
            None => self
                .outcome
                .iter()
                .map(|(text, tone)| status(text.clone(), *tone))
                .collect(),
            Some(Step::Checking { .. }) => vec![
                status("Checking that Claude is idle…".into(), StatusTone::Working),
                cancel(),
            ],
            Some(Step::Blocked {
                sessions,
                notify_when_idle,
                ..
            }) => {
                let busy = format!("Claude is busy: {}", sessions.join(", "));
                if *notify_when_idle {
                    vec![
                        status(
                            format!("{busy} · AccRoster will tell you when it's idle"),
                            StatusTone::Working,
                        ),
                        cancel(),
                    ]
                } else {
                    vec![
                        status(busy, StatusTone::NeedsYou),
                        action("Notify me when idle", Command::NotifyWhenDesktopIdle),
                        cancel(),
                    ]
                }
            }
            Some(Step::Unsure { reason, .. }) => vec![
                status(
                    format!("Can't tell whether Claude is busy ({reason})"),
                    StatusTone::NeedsYou,
                ),
                anyway(),
                cancel(),
            ],
            Some(Step::Syncing { target }) => {
                let progress =
                    sync.map_or(String::new(), |(done, total)| format!(" {done} of {total}"));
                vec![
                    status(
                        format!("Syncing sidebars into {}…{progress}", target.name),
                        StatusTone::Working,
                    ),
                    cancel(),
                ]
            }
            Some(Step::SyncStopped { target, reason }) => vec![
                status(
                    format!(
                        "Sidebar sync stopped: {reason} · switch to {} not started",
                        target.name
                    ),
                    StatusTone::Failed,
                ),
                anyway(),
                cancel(),
            ],
            Some(Step::Waiting { target, .. }) => {
                vec![status(instruction(target), StatusTone::NeedsYou), cancel()]
            }
        }
    }

    /// Brings Claude forward with the instruction and starts watching for
    /// the result. Claude is never quit: the user signs out and in. The
    /// snapshot it confirms from is taken here, after any pre-switch sync.
    pub(crate) fn guide(&mut self, ctx: &Context, target: Target) -> Vec<Effect> {
        let paths = DesktopPaths::new(&ctx.env.home);
        let previous = desktop_account(&paths).and_then(|account| {
            account
                .organization_uuid
                .map(|organization_uuid| ProfileKey {
                    account_uuid: account.account_uuid,
                    organization_uuid,
                })
        });
        let log_offset = fs::metadata(paths.main_log()).map_or(0, |meta| meta.len());
        log(
            ctx,
            Level::Info,
            "desktop switch guided",
            &[profile(&target)],
        );
        let effects = vec![
            Effect::BringDesktopAppForward,
            Effect::Notify {
                title: format!("Switch Claude to {}", target.name),
                body: instruction(&target),
            },
        ];
        self.outcome = None;
        self.step = Some(Step::Waiting {
            target,
            started_at: ctx.env.clock.now(),
            previous,
            before: snapshot(&paths),
            log_offset,
        });
        effects
    }

    /// Ends a waiting switch once its outcome is known (or it timed out).
    fn finish(
        &mut self,
        ctx: &Context,
        target: Target,
        outcome: Option<Outcome>,
        timed_out: bool,
        profiles: &[SignedInProfile],
    ) -> Vec<Effect> {
        let fields = [profile(&target)];
        let confirmed = matches!(outcome, Some(Outcome::Confirmed));
        let (title, text, tone) = match outcome {
            Some(Outcome::Confirmed) => {
                log(ctx, Level::Info, "desktop switch confirmed", &fields);
                (
                    "Claude switched",
                    format!("Claude now uses {}", target.name),
                    StatusTone::Done,
                )
            }
            Some(Outcome::Other(account)) => {
                log(ctx, Level::Warn, "desktop switch mismatch", &fields);
                let actual = account
                    .profile(profiles)
                    .map_or("another Profile", |profile| &profile.name);
                (
                    "Claude signed in to another Profile",
                    format!("Claude signed in as {actual}, not {}", target.name),
                    StatusTone::Failed,
                )
            }
            None if timed_out => {
                log(ctx, Level::Warn, "desktop switch unconfirmed", &fields);
                (
                    "Claude didn't switch",
                    format!(
                        "Gave up waiting after 30 min for Claude to sign in as {}",
                        target.name
                    ),
                    StatusTone::Failed,
                )
            }
            None => return Vec::new(),
        };
        self.step = None;
        self.outcome = Some((text.clone(), tone));
        let mut effects = vec![Effect::Notify {
            title: title.into(),
            body: text,
        }];
        // "Switch Claude to …, then open…": the entry is in Claude's sidebar
        // now.
        if let Some(entry) = target.then_open.as_deref().filter(|_| confirmed) {
            effects.push(Effect::OpenUrl(crate::opening::continue_url(entry)));
        }
        effects
    }
}

/// Why the guard can't tell, for the status line.
fn cant_tell_reason(unaccounted: usize) -> String {
    match unaccounted {
        1 => "a Claude Code process in Claude reports no status".to_owned(),
        n => format!("{n} Claude Code processes in Claude report no status"),
    }
}

fn profile(target: &Target) -> (&'static str, Field<'_>) {
    ("profile", Field::Profile(&target.profile_id))
}

/// What the user does in Claude.
fn instruction(target: &Target) -> String {
    format!("Sign out in Claude, then sign in as {}", target.name)
}

enum Outcome {
    Confirmed,
    /// Claude signed in elsewhere.
    Other(DesktopAccount),
}

/// What Claude's files say since the switch started, if anything yet.
///
/// The folder Claude writes decides: a new or changed Sidebar entry in a
/// folder other than the one it used before. Without one, a sign-in line in
/// `main.log` plus `lastKnownAccountUuid` can still tell a different account,
/// and confirm the target when its account has no other Organization folder.
fn confirmation(
    paths: &DesktopPaths,
    target: &Target,
    previous: Option<&Folder>,
    before: &Snapshot,
    log_offset: u64,
) -> Option<Outcome> {
    let now = snapshot(paths);
    let mut written: Vec<(SystemTime, &Folder)> = now
        .iter()
        .filter(|(folder, _)| Some(*folder) != previous)
        .filter_map(|(folder, entries)| {
            let old = before.get(folder);
            entries
                .iter()
                .filter(|(name, modified)| old.and_then(|old| old.get(*name)) != Some(*modified))
                .map(|(_, modified)| *modified)
                .max()
                .map(|newest| (newest, folder))
        })
        .collect();
    written.sort();
    if written.iter().any(|(_, folder)| **folder == target.key) {
        return Some(Outcome::Confirmed);
    }
    if let Some((_, folder)) = written.last() {
        return Some(Outcome::Other(DesktopAccount {
            account_uuid: folder.account_uuid.clone(),
            organization_uuid: Some(folder.organization_uuid.clone()),
        }));
    }
    if !log_signed_in_since(&paths.main_log(), log_offset) {
        return None;
    }
    let account = desktop_account(paths)?;
    if account.account_uuid != target.key.account_uuid {
        return Some(Outcome::Other(DesktopAccount {
            organization_uuid: None,
            ..account
        }));
    }
    let other_org = now.keys().any(|folder| {
        folder.account_uuid == target.key.account_uuid
            && folder.organization_uuid != target.key.organization_uuid
    });
    (!other_org).then_some(Outcome::Confirmed)
}

/// Whether `main.log` gained a sign-in line after `offset` (read from the
/// start when the log was rotated and is now shorter).
fn log_signed_in_since(log: &Path, offset: u64) -> bool {
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

// ----- Reopen Claude ---------------------------------------------------------------

/// How long Claude gets to quit, then to open again, after Reopen Claude.
const REOPEN_TIMEOUT: SignedDuration = SignedDuration::from_secs(30);

/// Where a Reopen Claude stands.
enum ReopenStep {
    /// A desktop turn is running: quits once the guard clears.
    Waiting { sessions: Vec<String> },
    /// The guard couldn't tell; waiting for "Reopen anyway" or "Cancel".
    Unsure { reason: String },
    /// Asked Claude to quit; waiting for it to exit.
    Quitting { since: Timestamp },
    /// Asked Claude to open; waiting for it to run.
    Opening { since: Timestamp },
}

/// Reopen Claude: the offer after a sync added entries to the sidebar Claude
/// is using while it runs, and the reopen under way.
#[derive(Default)]
pub(crate) struct Reopen {
    /// Claude runs without some Sessions sync added to its sidebar.
    offered: bool,
    step: Option<ReopenStep>,
}

/// How a Reopen Claude ended: its status line and tone.
pub(crate) type ReopenOutcome = (String, StatusTone);

impl Reopen {
    /// A sync added entries to the sidebar Claude is using while it runs.
    pub(crate) fn offer(&mut self) {
        self.offered = true;
    }

    /// Whether "Reopen Claude" is offered (no reopen under way).
    pub(crate) fn offered(&self) -> bool {
        self.offered && self.step.is_none()
    }

    /// Whether a reopen is under way.
    pub(crate) fn active(&self) -> bool {
        self.step.is_some()
    }

    /// Whether the core should check back every second: while waiting for
    /// Claude to go idle, quit or open.
    pub(crate) fn waiting(&self) -> bool {
        matches!(
            self.step,
            Some(
                ReopenStep::Waiting { .. }
                    | ReopenStep::Quitting { .. }
                    | ReopenStep::Opening { .. }
            )
        )
    }

    /// Whether Claude is being quit or opened: Sidebar sync waits, so it
    /// doesn't write while Claude shuts down or starts up.
    pub(crate) fn restarting(&self) -> bool {
        matches!(
            self.step,
            Some(ReopenStep::Quitting { .. } | ReopenStep::Opening { .. })
        )
    }

    /// "Reopen Claude": runs the busy guard and quits Claude when it is
    /// idle.
    pub(crate) fn start(&mut self, ctx: &Context) -> Vec<Effect> {
        if !self.offered() {
            return Vec::new();
        }
        log(ctx, Level::Info, "desktop reopen requested", &[]);
        let paths = DesktopPaths::new(&ctx.env.home);
        self.guarded(ctx, guard(ctx.env, &paths))
    }

    /// Acts on what the guard found: quits when clear, else waits or asks.
    fn guarded(&mut self, ctx: &Context, found: Guard) -> Vec<Effect> {
        match found {
            Guard::Clear => self.quit(ctx),
            Guard::Busy(sessions) => {
                if !matches!(self.step, Some(ReopenStep::Waiting { .. })) {
                    log(
                        ctx,
                        Level::Info,
                        "desktop reopen waiting",
                        &[("busy_sessions", Field::Number(sessions.len() as i64))],
                    );
                }
                self.step = Some(ReopenStep::Waiting { sessions });
                Vec::new()
            }
            Guard::CantTell(unaccounted) => {
                log(
                    ctx,
                    Level::Info,
                    "desktop reopen unsure",
                    &[("unaccounted", Field::Number(unaccounted as i64))],
                );
                self.step = Some(ReopenStep::Unsure {
                    reason: cant_tell_reason(unaccounted),
                });
                Vec::new()
            }
        }
    }

    /// Asks Claude to quit (or, when it already has, to open).
    fn quit(&mut self, ctx: &Context) -> Vec<Effect> {
        self.offered = false;
        let since = ctx.env.clock.now();
        if !ctx.env.os.desktop_app_running() {
            log(ctx, Level::Info, "desktop reopen opening", &[]);
            self.step = Some(ReopenStep::Opening { since });
            return vec![Effect::BringDesktopAppForward];
        }
        log(ctx, Level::Info, "desktop reopen quitting", &[]);
        self.step = Some(ReopenStep::Quitting { since });
        vec![Effect::QuitDesktopApp]
    }

    /// "Reopen anyway", after the guard couldn't tell.
    pub(crate) fn confirm(&mut self, ctx: &Context) -> Vec<Effect> {
        if !matches!(self.step, Some(ReopenStep::Unsure { .. })) {
            return Vec::new();
        }
        log(ctx, Level::Info, "desktop reopen confirmed anyway", &[]);
        self.quit(ctx)
    }

    /// "Cancel", before Claude was asked to quit: Claude is left as it is
    /// and Reopen Claude stays offered.
    pub(crate) fn cancel(&mut self, ctx: &Context) {
        if matches!(
            self.step,
            Some(ReopenStep::Waiting { .. } | ReopenStep::Unsure { .. })
        ) {
            self.step = None;
            log(ctx, Level::Info, "desktop reopen cancelled", &[]);
        }
    }

    /// Time passed: re-checks a busy Claude, and follows Claude quitting and
    /// opening. Returns the effects, and the outcome once it ended.
    pub(crate) fn poll(&mut self, ctx: &Context) -> (Vec<Effect>, Option<ReopenOutcome>) {
        let now = ctx.env.clock.now();
        match &self.step {
            None => {
                // Claude quit by other means reads its sidebar when it opens.
                if self.offered && !ctx.env.os.desktop_app_running() {
                    self.offered = false;
                }
                (Vec::new(), None)
            }
            Some(ReopenStep::Unsure { .. }) => (Vec::new(), None),
            Some(ReopenStep::Waiting { .. }) => {
                let paths = DesktopPaths::new(&ctx.env.home);
                (self.guarded(ctx, guard(ctx.env, &paths)), None)
            }
            Some(ReopenStep::Quitting { since }) => {
                if !ctx.env.os.desktop_app_running() {
                    log(ctx, Level::Info, "desktop reopen opening", &[]);
                    self.step = Some(ReopenStep::Opening { since: now });
                    return (vec![Effect::BringDesktopAppForward], None);
                }
                if now.duration_since(*since) < REOPEN_TIMEOUT {
                    return (Vec::new(), None);
                }
                log(ctx, Level::Warn, "desktop reopen: claude didn't quit", &[]);
                // Claude still runs without the Sessions: offered again.
                self.offered = true;
                self.end(
                    ("Claude didn't quit", "Try ⌘Q in Claude"),
                    "Claude didn't quit · try ⌘Q in Claude",
                    StatusTone::Failed,
                )
            }
            Some(ReopenStep::Opening { since }) => {
                if ctx.env.os.desktop_app_running() {
                    log(ctx, Level::Info, "desktop reopened", &[]);
                    return self.end(
                        (
                            "Claude reopened",
                            "Its sidebar now shows the synced Sessions",
                        ),
                        "Claude reopened · its sidebar now shows the synced Sessions",
                        StatusTone::Done,
                    );
                }
                if now.duration_since(*since) < REOPEN_TIMEOUT {
                    return (Vec::new(), None);
                }
                log(ctx, Level::Warn, "desktop reopen: claude didn't open", &[]);
                self.end(
                    (
                        "Claude didn't open",
                        "Open Claude from your Applications folder",
                    ),
                    "Claude didn't open · open it from your Applications folder",
                    StatusTone::Failed,
                )
            }
        }
    }

    /// Ends the reopen with a notification (`title`, `body`) and the status
    /// line `line`.
    fn end(
        &mut self,
        (title, body): (&str, &str),
        line: &str,
        tone: StatusTone,
    ) -> (Vec<Effect>, Option<ReopenOutcome>) {
        self.step = None;
        let effects = vec![Effect::Notify {
            title: title.into(),
            body: body.into(),
        }];
        (effects, Some((line.into(), tone)))
    }

    /// The status line of the reopen under way, if any.
    pub(crate) fn status(&self) -> Option<(String, StatusTone)> {
        Some(match self.step.as_ref()? {
            ReopenStep::Waiting { sessions } => (
                format!("Waiting for Claude to finish: {}", sessions.join(", ")),
                StatusTone::Working,
            ),
            ReopenStep::Unsure { reason } => (
                format!("Can't tell whether Claude is busy ({reason})"),
                StatusTone::NeedsYou,
            ),
            ReopenStep::Quitting { .. } | ReopenStep::Opening { .. } => {
                ("Reopening Claude…".into(), StatusTone::Working)
            }
        })
    }

    /// The status line of the reopen under way and the actions answering
    /// it, at the top of the menu.
    pub(crate) fn menu_items(&self) -> Vec<MenuItem> {
        let Some((text, tone)) = self.status() else {
            return Vec::new();
        };
        let action = |title: &str, command| MenuItem::Action {
            title: title.into(),
            command,
        };
        let mut items = vec![MenuItem::Status { text, tone }];
        match self.step {
            Some(ReopenStep::Waiting { .. }) => {
                items.push(action("Cancel", Command::CancelReopenDesktopApp));
            }
            Some(ReopenStep::Unsure { .. }) => {
                items.push(action("Reopen anyway", Command::ConfirmReopenDesktopApp));
                items.push(action("Cancel", Command::CancelReopenDesktopApp));
            }
            _ => {}
        }
        items
    }
}

fn log(ctx: &Context, level: Level, message: &'static str, fields: &[(&'static str, Field<'_>)]) {
    ctx.logger
        .write(ctx.env.clock.now(), ctx.verbose, level, message, fields);
}
