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
//!   matching `procStart` and its `entrypoint` is `claude-desktop`; `idle` is
//!   idle, anything else (`busy`, `waiting`, unknown) is busy. A desktop child
//!   process without such a file means "can't tell". The `.key` files next to
//!   them are never opened.
//! - **Guided switch:** guard, then bring Claude forward with "Sign out, then
//!   sign in as …", then confirm from the Sidebar folder Claude writes (or
//!   `main.log`).
//!
//! AccRoster never quits or relaunches Claude: nothing here can.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use jiff::{SignedDuration, Timestamp};
use serde::Deserialize;

use crate::env::Environment;
use crate::interface::{ClaudeVersions, Command, Effect, MenuItem};
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
    /// The signed-in Profile this is, if any. With no folder written yet, an
    /// account that only one Profile has is enough.
    pub(crate) fn profile<'a>(
        &self,
        profiles: &'a [SignedInProfile],
    ) -> Option<&'a SignedInProfile> {
        let same_account = || {
            profiles
                .iter()
                .filter(|profile| profile.account_uuid == self.account_uuid)
        };
        match &self.organization_uuid {
            Some(org) => same_account().find(|profile| &profile.organization_uuid == org),
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

/// The desktop's account, or `None` when its `config.json` is missing,
/// unreadable or names no account.
pub(crate) fn desktop_account(paths: &DesktopPaths) -> Option<DesktopAccount> {
    let bytes = fs::read(paths.config()).ok()?;
    let config: DesktopConfig = serde_json::from_slice(&bytes).ok()?;
    let account_uuid = config.last_known_account_uuid.filter(|id| is_uuid(id))?;
    let organization_uuid = newest_org_folder(&paths.sidebar_root().join(&account_uuid));
    Some(DesktopAccount {
        account_uuid,
        organization_uuid,
    })
}

/// The Organization folder under an account folder holding the most recently
/// written Sidebar entry.
fn newest_org_folder(account_dir: &Path) -> Option<String> {
    fs::read_dir(account_dir)
        .ok()?
        .flatten()
        .filter_map(|org| {
            let name = org.file_name().into_string().ok().filter(|n| is_uuid(n))?;
            let newest = newest_entry(&org.path())?;
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
    /// A desktop Claude Code child runs without a readable live-process file.
    CantTell,
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
        .any(|pid| !live.iter().any(|process| process.pid == pid));
    if unaccounted {
        Guard::CantTell
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
    let time_zone = env.clock.time_zone();
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
            let started = parse_proc_start(process.proc_start.as_deref()?, &time_zone)?;
            (started.as_second() == facts.started_at.as_second()).then_some(process)
        })
        .collect()
}

/// `procStart` as `ps -o lstart` prints it (`Sun Oct 25 14:02:11 2026`, the
/// day space-padded), in the Mac's local time.
pub(crate) fn parse_proc_start(text: &str, time_zone: &jiff::tz::TimeZone) -> Option<Timestamp> {
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
    let zoned = date.to_datetime(time).to_zoned(time_zone.clone()).ok()?;
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
    pub account_uuid: String,
    pub organization_uuid: String,
    /// `email · Organization`.
    pub name: String,
    /// The Sidebar entry (`local_…`) to open once the switch is confirmed.
    pub then_open: Option<String>,
}

impl Target {
    pub(crate) fn of(profile: &SignedInProfile) -> Target {
        Target {
            profile_id: profile.id.clone(),
            account_uuid: profile.account_uuid.clone(),
            organization_uuid: profile.organization_uuid.clone(),
            name: profile.name.clone(),
            then_open: None,
        }
    }
}

/// An `<account>/<org>` Sidebar folder.
type Folder = (String, String);

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
                (account_uuid.clone(), org_uuid),
                sidebar_entries(&org.path()).into_iter().collect(),
            );
        }
    }
    folders
}

enum Step {
    /// A desktop turn is running; nothing was done.
    Blocked {
        target: Target,
        sessions: Vec<String>,
        notify_when_idle: bool,
    },
    /// The guard couldn't tell; waiting for "Switch anyway" or "Cancel".
    Unsure { target: Target },
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

/// The Guided switch in progress, and the outcome of the last one.
#[derive(Default)]
pub(crate) struct DesktopBridge {
    step: Option<Step>,
    notice: Option<String>,
}

impl DesktopBridge {
    /// Whether a switch is waiting for Claude to sign in.
    pub(crate) fn waiting(&self) -> bool {
        matches!(self.step, Some(Step::Waiting { .. }))
    }

    /// "Switch desktop app to this Profile…": runs the guard. Returns the
    /// target when nothing stands in the way: the caller syncs the sidebars
    /// (spec step 2), then calls [`DesktopBridge::guide`].
    pub(crate) fn start(&mut self, ctx: &Context, target: Target) -> Option<Target> {
        self.notice = None;
        let paths = DesktopPaths::new(&ctx.env.home);
        match guard(ctx.env, &paths) {
            Guard::Busy(sessions) => {
                log(
                    ctx,
                    Level::Info,
                    "desktop switch blocked",
                    &[
                        ("profile", Field::Profile(&target.profile_id)),
                        ("busy_sessions", Field::Number(sessions.len() as i64)),
                    ],
                );
                self.step = Some(Step::Blocked {
                    target,
                    sessions,
                    notify_when_idle: false,
                });
                None
            }
            Guard::CantTell => {
                log(
                    ctx,
                    Level::Info,
                    "desktop switch unsure",
                    &[("profile", Field::Profile(&target.profile_id))],
                );
                self.step = Some(Step::Unsure { target });
                None
            }
            Guard::Clear => Some(target),
        }
    }

    /// "Switch anyway" after "can't tell": the target, as for
    /// [`DesktopBridge::start`].
    pub(crate) fn confirm(&mut self) -> Option<Target> {
        match self.step.take() {
            Some(Step::Unsure { target }) => Some(target),
            other => {
                self.step = other;
                None
            }
        }
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

    /// "Cancel switch": forgets the switch. Claude is left as it is.
    pub(crate) fn cancel(&mut self, ctx: &Context) {
        if self.step.take().is_some() {
            log(ctx, Level::Info, "desktop switch cancelled", &[]);
        }
        self.notice = None;
    }

    /// Time passed: re-checks a blocked switch the user wants to hear about,
    /// and looks for the account a guided Claude signed in to.
    pub(crate) fn poll(&mut self, ctx: &Context, profiles: &[SignedInProfile]) -> Vec<Effect> {
        let paths = DesktopPaths::new(&ctx.env.home);
        match &mut self.step {
            Some(Step::Blocked {
                target,
                sessions,
                notify_when_idle: true,
            }) => match guard(ctx.env, &paths) {
                Guard::Busy(now) => {
                    *sessions = now;
                    Vec::new()
                }
                Guard::Clear | Guard::CantTell => {
                    let body = format!("You can switch Claude to {} now", target.name);
                    self.step = None;
                    log(ctx, Level::Info, "desktop idle", &[]);
                    vec![Effect::Notify {
                        title: "Claude is idle".into(),
                        body,
                    }]
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
                let timed_out = ctx.env.clock.now().duration_since(*started_at) >= CONFIRM_TIMEOUT;
                let target = target.clone();
                self.finish(ctx, target, outcome, timed_out, profiles)
            }
            _ => Vec::new(),
        }
    }

    /// The bridge's menu lines, shown with the other warnings.
    pub(crate) fn menu_items(&self) -> Vec<MenuItem> {
        let cancel = || MenuItem::Action {
            title: "Cancel switch".into(),
            command: Command::CancelDesktopSwitch,
        };
        let mut items = Vec::new();
        match &self.step {
            None => {}
            Some(Step::Blocked {
                target,
                sessions,
                notify_when_idle,
            }) => {
                let running = format!("Claude is running {}", sessions.join(", "));
                if *notify_when_idle {
                    items.push(MenuItem::Warning(format!(
                        "{running} · AccRoster will tell you when it's idle"
                    )));
                } else {
                    items.push(MenuItem::Warning(format!(
                        "{running} · switch to {} not started",
                        target.name
                    )));
                    items.push(MenuItem::Action {
                        title: "Notify me when idle".into(),
                        command: Command::NotifyWhenDesktopIdle,
                    });
                }
                items.push(cancel());
            }
            Some(Step::Unsure { .. }) => {
                items.push(MenuItem::Warning(
                    "Can't tell whether Claude is running a turn · switching ends every running turn"
                        .into(),
                ));
                items.push(MenuItem::Action {
                    title: "Switch anyway".into(),
                    command: Command::ConfirmDesktopSwitch,
                });
                items.push(cancel());
            }
            Some(Step::Waiting { target, .. }) => {
                items.push(MenuItem::Warning(format!(
                    "In Claude: {}",
                    instruction(target)
                )));
                items.push(cancel());
            }
        }
        if let Some(notice) = &self.notice {
            items.push(MenuItem::Warning(notice.clone()));
        }
        items
    }

    /// Brings Claude forward with the instruction and starts watching for
    /// the result. Claude is never quit: the user signs out and in. The
    /// snapshot it confirms from is taken here, after any pre-switch sync.
    pub(crate) fn guide(&mut self, ctx: &Context, target: Target) -> Vec<Effect> {
        let paths = DesktopPaths::new(&ctx.env.home);
        let previous = desktop_account(&paths).and_then(|account| {
            account
                .organization_uuid
                .map(|org| (account.account_uuid, org))
        });
        let log_offset = fs::metadata(paths.main_log()).map_or(0, |meta| meta.len());
        log(
            ctx,
            Level::Info,
            "desktop switch guided",
            &[("profile", Field::Profile(&target.profile_id))],
        );
        let effects = vec![
            Effect::BringDesktopAppForward,
            Effect::Notify {
                title: format!("Switch Claude to {}", target.name),
                body: instruction(&target),
            },
        ];
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
        let profile = [("profile", Field::Profile(&target.profile_id))];
        let confirmed = matches!(outcome, Some(Outcome::Confirmed));
        let (title, text) = match outcome {
            Some(Outcome::Confirmed) => {
                log(ctx, Level::Info, "desktop switch confirmed", &profile);
                (
                    "Claude switched",
                    format!("Claude now uses {}", target.name),
                )
            }
            Some(Outcome::Other(account)) => {
                log(ctx, Level::Warn, "desktop switch mismatch", &profile);
                let actual = account
                    .profile(profiles)
                    .map_or("another account", |profile| &profile.name);
                (
                    "Claude signed in to another account",
                    format!("Claude signed in as {actual}, not {}", target.name),
                )
            }
            None if timed_out => {
                log(ctx, Level::Warn, "desktop switch unconfirmed", &profile);
                self.step = None;
                self.notice = Some(format!(
                    "Couldn't confirm Claude switched to {} · check its account menu",
                    target.name
                ));
                return Vec::new();
            }
            None => return Vec::new(),
        };
        self.step = None;
        self.notice = Some(text.clone());
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

/// What the user does in Claude.
fn instruction(target: &Target) -> String {
    format!("Sign out, then sign in as {}", target.name)
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
    let target_folder = (
        target.account_uuid.clone(),
        target.organization_uuid.clone(),
    );
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
    if written.iter().any(|(_, folder)| **folder == target_folder) {
        return Some(Outcome::Confirmed);
    }
    if let Some((_, (account, org))) = written.last() {
        return Some(Outcome::Other(DesktopAccount {
            account_uuid: account.clone(),
            organization_uuid: Some(org.clone()),
        }));
    }
    if !log_signed_in_since(&paths.main_log(), log_offset) {
        return None;
    }
    let account = desktop_account(paths)?;
    if account.account_uuid != target.account_uuid {
        return Some(Outcome::Other(DesktopAccount {
            organization_uuid: None,
            ..account
        }));
    }
    let other_org = now
        .keys()
        .any(|(acc, org)| *acc == target.account_uuid && *org != target.organization_uuid);
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

fn log(ctx: &Context, level: Level, message: &'static str, fields: &[(&'static str, Field<'_>)]) {
    ctx.logger
        .write(ctx.env.clock.now(), ctx.verbose, level, message, fields);
}
