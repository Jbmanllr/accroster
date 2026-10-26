//! Sidebar sync (ADR 0003): makes every Session's Sidebar entry exist in
//! every Profile folder under `claude-code-sessions/<account>/<org>/`.
//!
//! - **Where it writes.** Every existing Profile folder (signed in to
//!   AccRoster or not: a folder is a Profile's sidebar). While Claude runs,
//!   a folder of the desktop's account only gains the entries it lacks:
//!   Claude overwrites the entries it holds from memory, and its Active
//!   Organization can't be read from disk, so no entry already in any
//!   Organization of that account is replaced, and a new one is written
//!   only if no file took its name meanwhile. Claude reads new entries at
//!   launch or an account switch. If Claude runs and its account can't be
//!   read, nothing is written. Folders are never created and files are
//!   never deleted.
//! - **What it copies.** `local_*.json` entries only (never one for a
//!   CLI-only Session), skipping WSL and scheduled-task Sessions, SSH and
//!   No-folder Sessions when "Include SSH and No-folder Sessions" is off
//!   (ADR 0003's amendments), files over 10 MB, empty and unparsable files. A copy keeps the
//!   file name and every field; `bridgeSessionIds`, `remoteMcpServersConfig`
//!   and `enabledMcpTools` are cleared when present, and `sshReattach` is
//!   dropped. Each run counts the Sessions it left out, by why. A `deleted_<id>`
//!   tombstone in a folder stops the Session being added there again.
//! - **Which copy wins.** Entries carry no per-field times, so sync keeps
//!   its own record (`state/sidebar-sync.json`, hashes only) of each copy as
//!   of its last run. Title and archive state each come from the copy that
//!   differs from its record; if several do, the desktop's Active Profile's
//!   copy, else the newest file. Every other field comes from the copy with
//!   the newest `lastActivityAt`, which holds the current `cliSessionId`.
//! - **Before writing** (ADR 0004): a target folder Claude wasn't using that
//!   changed since sync's record (and no account switch since) is the dormant
//!   multi-account mode's trace; it stops the whole run before any write, for
//!   a Safety pause. Unknown top-level fields don't: they are copied as they
//!   are. Then each target folder is backed up, each exact file is read
//!   again, and each copy is written as temp file then rename.
//! - **Reading** ([`EntryCache`]): a run plans from a slim summary of each
//!   entry, and parses only the files changed since the last run read or
//!   wrote them. A copy's source is read in full only when the copy is
//!   written, and only while it is still the file planned from.
//! - **Watching** ([`Watch`]): while sync is on, the UI watches
//!   `claude-code-sessions/`. Automatic mode runs a few seconds after the
//!   last change to the desktop's Active Profile folder. An entry Claude
//!   writes in any other folder while it runs, more than 30 s after the last
//!   account switch, is the other trace of the dormant multi-account mode.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::fs;
use std::hash::{DefaultHasher, Hasher};
use std::io::{self, Read as _};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};

use crate::backups;
use crate::desktop::{self, DesktopPaths};
use crate::env::Environment;
use crate::interface::Effect;
use crate::storage::{self, AppPaths, VersionedFile};

/// The desktop app's loader skips larger entries; so does sync.
const MAX_ENTRY_BYTES: u64 = 10 * 1024 * 1024;

/// Fields a copy clears: they tie an entry to one account's connectors and
/// remote-control bridge. The desktop app rebuilds them.
const CLEARED: [&str; 3] = [
    "bridgeSessionIds",
    "remoteMcpServersConfig",
    "enabledMcpTools",
];

/// Fields a copy drops: `sshReattach` binds an SSH Session's remote process
/// to the login that started it, so a copy under another login must not
/// claim it (ADR 0003).
const DROPPED: [&str; 1] = ["sshReattach"];

/// The fields that make up an entry's title, and its archive state.
const TITLE_FIELDS: [&str; 5] = [
    "title",
    "titleSource",
    "titleTurn",
    "previousTitles",
    "titleOffers",
];
const ARCHIVE_FIELDS: [&str; 2] = ["isArchived", "autoArchiveExempt"];

/// Entries with any of these are SSH Sessions.
const SSH_MARKERS: [&str; 3] = ["sshConfig", "sshRemoteTranscriptPath", "sshRemoteProcessId"];
/// … WSL Sessions.
const WSL_MARKERS: [&str; 1] = ["wslConfig"];
/// … scheduled-task Sessions.
const SCHEDULED_MARKERS: [&str; 2] = ["scheduledTaskId", "notifySessionId"];

/// The kind of a Session sync left out. SSH and No-folder Sessions only
/// when "Include SSH and No-folder Sessions" is off; WSL and scheduled-task
/// Sessions always: Claude runs those on one account only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum LeftOut {
    Ssh,
    /// A "No folder" Session, working in Claude's scratch workspace.
    NoFolder,
    Wsl,
    ScheduledTask,
}

impl LeftOut {
    /// How the sync report names them ("17 SSH").
    pub(crate) fn name(self) -> &'static str {
        match self {
            LeftOut::Ssh => "SSH",
            LeftOut::NoFolder => "No-folder",
            LeftOut::Wsl => "WSL",
            LeftOut::ScheduledTask => "scheduled-task",
        }
    }

    /// Why they were left out, in plain words.
    pub(crate) fn why(self) -> &'static str {
        match self {
            LeftOut::Ssh | LeftOut::NoFolder => "Include SSH and No-folder Sessions is off",
            LeftOut::Wsl | LeftOut::ScheduledTask => "Claude runs those on one account only",
        }
    }

    /// Left out even with "Include SSH and No-folder Sessions" on.
    fn always(self) -> bool {
        matches!(self, LeftOut::Wsl | LeftOut::ScheduledTask)
    }

    /// Its log field.
    pub(crate) fn log_key(self) -> &'static str {
        match self {
            LeftOut::Ssh => "skipped_ssh",
            LeftOut::NoFolder => "skipped_no_folder",
            LeftOut::Wsl => "skipped_wsl",
            LeftOut::ScheduledTask => "skipped_scheduled",
        }
    }
}

/// What one run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Run {
    Synced(Report),
    /// Not started: Sidebar sync is under a Safety pause.
    Paused,
    /// Claude runs but its account can't be read, so any folder could be
    /// the one it uses: nothing was written.
    DesktopAccountUnknown,
    /// Stopped: this many target folders changed since sync's record while
    /// Claude wasn't using them.
    FoldersChanged(usize),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Report {
    /// Entries created or updated.
    pub written: usize,
    /// Folders written into.
    pub folders: usize,
    /// Folders left unchanged because their backup failed.
    pub not_backed_up: usize,
    /// Of `written`, entries added to the folders of the account Claude
    /// runs on (which only gain entries while it runs).
    pub added_to_desktop: usize,
    /// Entries in those folders that differ but were left as they are
    /// until Claude is quit.
    pub held_for_desktop: usize,
    /// Sessions never copied, by why (each counted once, whatever the
    /// number of sidebars holding it).
    pub left_out: BTreeMap<LeftOut, usize>,
    /// Entries read and parsed: those new or changed since the last run.
    pub parsed: usize,
}

/// `state/sidebar-sync.json`: sync's record as of its last run. Hashes only
/// (no titles, no ids beyond the folder and file names).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    /// By `<account>/<org>`, then by file name: each copy as last seen.
    #[serde(default)]
    folders: BTreeMap<String, BTreeMap<String, CopyRecord>>,
    /// By file name: the title and archive state sync last decided on.
    #[serde(default)]
    sessions: BTreeMap<String, Groups>,
    /// The desktop's Active Profile folder at the last run.
    #[serde(default)]
    desktop_folder: Option<String>,
    /// `main.log`'s length at the last run, to see a sign-in since.
    #[serde(default)]
    main_log_length: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CopyRecord {
    content: String,
    #[serde(flatten)]
    groups: Groups,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Groups {
    title: String,
    archive: String,
}

impl VersionedFile for Record {
    const VERSION: u64 = 1;
}

fn record_path(paths: &AppPaths) -> PathBuf {
    paths.state().join("sidebar-sync.json")
}

/// One `<account>/<org>` folder as read.
struct Folder {
    account: String,
    /// `<account>/<org>`.
    key: String,
    path: PathBuf,
    /// Readable entries by file name.
    copies: BTreeMap<String, Arc<Entry>>,
    /// Transcript ids of `deleted_<id>` tombstones.
    tombstones: BTreeSet<String>,
}

/// What sync holds of an entry: enough to plan every write without its
/// body, which is read again only for the copies made from it.
struct Entry {
    /// [`fingerprint`] of the file as read: it is replaced, or copied from,
    /// only while it still matches.
    fingerprint: u64,
    /// Sync's record of the file, account-bound fields included.
    record: CopyRecord,
    /// `lastActivityAt`, which picks the copy every other field comes from.
    last_activity: Option<i64>,
    /// `cliSessionId`: the Transcript a tombstone names.
    transcript: Option<String>,
    /// The title fields and the archive state, as they are.
    title: Map<String, Value>,
    archive: Map<String, Value>,
    /// [`rest_hash`] of the other fields.
    rest: u64,
    /// Why sync leaves it out while SSH and No-folder Sessions are left out.
    left_out: Option<LeftOut>,
    modified: SystemTime,
}

impl Entry {
    fn new(bytes: &[u8], body: &Map<String, Value>, modified: SystemTime) -> Entry {
        Entry {
            fingerprint: fingerprint(bytes),
            record: copy_record(body),
            last_activity: body.get("lastActivityAt").and_then(Value::as_i64),
            transcript: body
                .get("cliSessionId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            title: pick(body, &TITLE_FIELDS),
            archive: pick(body, &ARCHIVE_FIELDS),
            rest: rest_hash(body),
            left_out: left_out(body),
            modified,
        }
    }
}

/// Every entry as sync last read or wrote it, by path, with the file's
/// [`Stamp`] then; `None` for a file that isn't an entry sync copies. A
/// run parses only the files whose stamp changed, so the steady state costs
/// a `stat` per entry.
#[derive(Default)]
pub(crate) struct EntryCache(HashMap<PathBuf, (Stamp, Option<Arc<Entry>>)>);

impl std::fmt::Debug for EntryCache {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "EntryCache({} files)", self.0.len())
    }
}

impl EntryCache {
    /// The body of the entry at `path`, if the file is still the one sync
    /// last read or wrote there (else the next run sees what changed).
    fn read_again(&self, path: &Path) -> Option<Map<String, Value>> {
        let expected = self.0.get(path)?.1.as_ref()?.fingerprint;
        let bytes = fs::read(path).ok()?;
        (fingerprint(&bytes) == expected)
            .then(|| parse_body(&bytes))
            .flatten()
    }
}

/// A file as `stat` sees it. Claude and sync replace an entry by renaming
/// a new file over it, which changes the inode; the change time also
/// catches an edit in place whose modification time was put back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stamp {
    inode: u64,
    len: u64,
    modified: SystemTime,
    changed: (i64, i64),
}

impl Stamp {
    fn of(meta: &fs::Metadata) -> Option<Stamp> {
        Some(Stamp {
            inode: meta.ino(),
            len: meta.len(),
            modified: meta.modified().ok()?,
            changed: (meta.ctime(), meta.ctime_nsec()),
        })
    }
}

/// Everything under `claude-code-sessions/`, plus what the checks found.
struct Read {
    folders: Vec<Folder>,
    /// File names of entries sync never copies, with why.
    skipped: BTreeMap<String, LeftOut>,
    /// Entries parsed, because they were new or changed.
    parsed: usize,
}

/// How long the desktop's folder stays quiet before Automatic mode syncs.
const SETTLE: SignedDuration = SignedDuration::from_secs(3);

/// Switching flushes the old account's Sessions into its folder: entries
/// written outside the desktop's folder this soon after a switch, or after
/// the watch started (a switch just before launch looks the same), are that.
const SWITCH_GRACE: SignedDuration = SignedDuration::from_secs(30);

/// The watch on the sidebars while sync is on: when Automatic mode runs
/// next, and the account switches that excuse writes outside the desktop's
/// Active Profile folder.
#[derive(Debug, Default)]
pub(crate) struct Watch {
    /// The UI was asked to watch `claude-code-sessions/`.
    watching: bool,
    /// When Automatic mode runs, once the desktop's folder stays quiet.
    due: Option<Timestamp>,
    /// The desktop's Active Profile folder (`<account>/<org>`) last seen.
    desktop: Option<String>,
    main_log_length: Option<u64>,
    /// When a switch was last noticed, or the watch started.
    switched_at: Timestamp,
}

impl Watch {
    /// Starts the watch while sync is `on` and the sidebars' folder exists
    /// (checked again on every input until it does); stops it when off.
    pub(crate) fn reconcile(&mut self, env: &Environment, on: bool) -> Option<Effect> {
        let root = sidebar_root(&env.home);
        if on && !self.watching && root.is_dir() {
            let desktop_paths = DesktopPaths::new(&env.home);
            *self = Watch {
                watching: true,
                desktop: active_key(&desktop_paths),
                main_log_length: desktop::main_log_length(&desktop_paths),
                switched_at: env.clock.now(),
                ..Watch::default()
            };
            return Some(Effect::WatchSidebarFolders(vec![root]));
        }
        if !on && self.watching {
            *self = Watch::default();
            return Some(Effect::StopWatchingSidebarFolders);
        }
        None
    }

    /// Notes an account switch since the last look: another desktop folder,
    /// or a sign-in line in `main.log`.
    pub(crate) fn notice_switch(&mut self, env: &Environment) {
        if !self.watching {
            return;
        }
        let desktop_paths = DesktopPaths::new(&env.home);
        let desktop = active_key(&desktop_paths);
        let signed_in = desktop::signed_in_since(&desktop_paths, self.main_log_length.unwrap_or(0));
        if desktop != self.desktop || signed_in {
            self.switched_at = env.clock.now();
        }
        self.desktop = desktop;
        self.main_log_length = desktop::main_log_length(&desktop_paths);
    }

    /// FSEvents reported `changed`. Entries that differ from sync's record
    /// were written by someone else: in the desktop's folder they schedule
    /// Automatic mode (when `automatic`); elsewhere, while Claude runs and
    /// not just after a switch, they are counted and returned (the dormant
    /// multi-account mode's trace).
    pub(crate) fn files_changed(
        &mut self,
        env: &Environment,
        paths: &AppPaths,
        changed: &[PathBuf],
        automatic: bool,
    ) -> usize {
        if !self.watching {
            return 0;
        }
        self.notice_switch(env);
        let now = env.clock.now();
        let root = sidebar_root(&env.home);
        let record = load(paths);
        let mut elsewhere = 0;
        for path in changed {
            let Some((key, name)) = entry_location(&root, path) else {
                continue;
            };
            let Some(body) = read_body(path) else {
                continue;
            };
            let recorded = record
                .folders
                .get(&key)
                .and_then(|copies| copies.get(&name))
                .map(|copy| &copy.content);
            if recorded == Some(&hash(&body)) {
                // Sync wrote it, or nothing changed.
                continue;
            }
            if Some(&key) == self.desktop.as_ref() {
                if automatic {
                    self.due = Some(now + SETTLE);
                }
            } else {
                elsewhere += 1;
            }
        }
        let settled = now.duration_since(self.switched_at) > SWITCH_GRACE;
        if self.desktop.is_some() && settled && env.os.desktop_app_running() {
            elsewhere
        } else {
            0
        }
    }

    /// Whether Automatic mode's run is due now (then it's no longer pending).
    pub(crate) fn take_due(&mut self, now: Timestamp) -> bool {
        let due = self.due.is_some_and(|due| due <= now);
        if due {
            self.due = None;
        }
        due
    }

    /// A run happened, whatever started it: nothing is pending any more.
    pub(crate) fn ran(&mut self) {
        self.due = None;
    }

    /// How long until Automatic mode's run, if one is pending.
    pub(crate) fn until_due(&self, now: Timestamp) -> Option<Duration> {
        let due = self.due?;
        Some(Duration::try_from(due.duration_since(now)).unwrap_or(Duration::ZERO))
    }
}

/// `<account>/<org>` and the file name of a `local_*.json` under `root`.
fn entry_location(root: &Path, path: &Path) -> Option<(String, String)> {
    let parts: Vec<&str> = path
        .strip_prefix(root)
        .ok()?
        .iter()
        .map(|part| part.to_str())
        .collect::<Option<_>>()?;
    let [account, org, name] = parts.as_slice() else {
        return None;
    };
    let entry = name.starts_with("local_") && name.ends_with(".json");
    (desktop::is_uuid(account) && desktop::is_uuid(org) && entry)
        .then(|| (format!("{account}/{org}"), (*name).to_owned()))
}

fn read_body(path: &Path) -> Option<Map<String, Value>> {
    let meta = fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > MAX_ENTRY_BYTES {
        return None;
    }
    parse_body(&fs::read(path).ok()?)
}

fn parse_body(bytes: &[u8]) -> Option<Map<String, Value>> {
    match serde_json::from_slice(bytes).ok()? {
        Value::Object(body) => Some(body),
        _ => None,
    }
}

fn sidebar_root(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Claude/claude-code-sessions")
}

/// The desktop's Active Profile folder as `<account>/<org>`.
fn active_key(desktop_paths: &DesktopPaths) -> Option<String> {
    let account = desktop::desktop_account(desktop_paths)?;
    let org = account.organization_uuid?;
    Some(format!("{}/{org}", account.account_uuid))
}

/// A sync under way: planned in one go, then written one sidebar (folder)
/// per [`SyncRun::step`], so the menu can show its progress. Sync's record
/// is saved after every folder, so a run cut short leaves it true to disk.
pub(crate) struct SyncRun {
    read: Read,
    /// Folder index and its writes, in folder order.
    pending: VecDeque<(usize, Vec<Write>)>,
    total: usize,
    report: Report,
    /// Sync's record so far: the old one, with each folder written since
    /// as it is now.
    record: Record,
    /// What the finished run records.
    decided: BTreeMap<String, Groups>,
    active_key: Option<String>,
    log_length: Option<u64>,
    /// Folders written into so far, by key.
    written: BTreeSet<String>,
}

/// How a "Sync now" starts.
pub(crate) enum Begun {
    /// Writing: call [`SyncRun::step`] until it ends.
    Running(Box<SyncRun>),
    /// It ended before writing anything (nothing to write, or stopped).
    Ended(Run),
}

/// "Sync now": reads every sidebar, checks it and plans every write.
/// With `include_optional` ("Include SSH and No-folder Sessions", the
/// default), SSH and No-folder Sessions are copied too; WSL and
/// scheduled-task Sessions never are. `cache` holds the entries as of the
/// last run; this one parses only those changed since.
pub(crate) fn begin(
    env: &Environment,
    paths: &AppPaths,
    include_optional: bool,
    cache: &mut EntryCache,
) -> Begun {
    let desktop_paths = DesktopPaths::new(&env.home);
    let desktop = desktop::desktop_account(&desktop_paths);
    let running = env.os.desktop_app_running();
    if running && desktop.is_none() {
        return Begun::Ended(Run::DesktopAccountUnknown);
    }
    let read = read_all(&env.home, include_optional, cache);
    let record = load(paths);
    let active_key = active_key(&desktop_paths);
    let active = read
        .folders
        .iter()
        .position(|folder| Some(&folder.key) == active_key.as_ref());
    // Claude's account's folders, while it runs: entries Claude may be
    // saving are never replaced there.
    let add_only = |folder: &Folder| {
        running
            && desktop
                .as_ref()
                .is_some_and(|account| account.account_uuid == folder.account)
    };

    let (writes, decided, held) = plan(&read, &record, active, &add_only);

    // The dormant multi-account mode: since the last run, with no account
    // switch, a folder sync may write changed although it belongs to an
    // account Claude wasn't on. Without a known account then, nothing tells.
    let log_length = desktop::main_log_length(&desktop_paths);
    let switched = record.desktop_folder != active_key
        || record
            .main_log_length
            .is_some_and(|length| desktop::signed_in_since(&desktop_paths, length));
    if let (false, Some(recorded_desktop)) = (switched, &record.desktop_folder) {
        let desktop_account = recorded_desktop.split('/').next().unwrap_or_default();
        let changed = read
            .folders
            .iter()
            .filter(|folder| folder.account != desktop_account)
            .filter(|folder| {
                record
                    .folders
                    .get(&folder.key)
                    .is_some_and(|recorded| *recorded != copy_records(folder))
            })
            .count();
        if changed > 0 {
            return Begun::Ended(Run::FoldersChanged(changed));
        }
    }

    let mut by_folder: BTreeMap<usize, Vec<Write>> = BTreeMap::new();
    for write in writes {
        by_folder.entry(write.folder).or_default().push(write);
    }
    let report = Report {
        held_for_desktop: held,
        left_out: read.left_out(),
        parsed: read.parsed,
        ..Report::default()
    };
    let mut sync = SyncRun {
        total: by_folder.len(),
        pending: by_folder.into_iter().collect(),
        read,
        report,
        record,
        decided,
        active_key,
        log_length,
        written: BTreeSet::new(),
    };
    if sync.pending.is_empty() {
        return Begun::Ended(sync.finish(paths));
    }
    Begun::Running(Box::new(sync))
}

impl SyncRun {
    /// Sidebars written so far, and how many this run writes.
    pub(crate) fn progress(&self) -> (usize, usize) {
        (self.total - self.pending.len(), self.total)
    }

    /// Writes the next sidebar, noting each copy in `cache`; `Some` once the
    /// run ended.
    pub(crate) fn step(
        &mut self,
        env: &Environment,
        paths: &AppPaths,
        cache: &mut EntryCache,
    ) -> Option<Run> {
        let Some((index, folder_writes)) = self.pending.pop_front() else {
            return Some(self.finish(paths));
        };
        let folder = &self.read.folders[index];
        if backups::back_up_folder(&paths.backups(), &folder.path, env.clock.now()).is_err() {
            self.report.not_backed_up += 1;
            return self.pending.is_empty().then(|| self.finish(paths));
        }
        let mut copies = copy_records(folder);
        let mut wrote_any = false;
        for write in folder_writes {
            let file = folder.path.join(&write.name);
            // The exact file, read again right before it's replaced.
            let now_there = match fs::read(&file) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(_) => continue,
            };
            if write.add_only && now_there.is_some() {
                // Claude saved its own entry meanwhile: it stays.
                continue;
            }
            let expected = folder
                .copies
                .get(&write.name)
                .map(|entry| entry.fingerprint);
            if now_there.as_deref().map(fingerprint) != expected {
                if wrote_any {
                    self.written.insert(folder.key.clone());
                    self.record.folders.insert(folder.key.clone(), copies);
                    save(paths, &self.record);
                }
                return Some(Run::FoldersChanged(1));
            }
            let body = if write.own_folder {
                // The file just checked is the source: it keeps its own
                // account-bound fields.
                now_there.as_deref().and_then(parse_body)
            } else {
                let source = &self.read.folders[write.source];
                cache
                    .read_again(&source.path.join(&write.name))
                    .map(|mut body| {
                        clear(&mut body);
                        body
                    })
            };
            let Some(mut body) = body else {
                continue;
            };
            take_fields(&mut body, &write.title, &TITLE_FIELDS);
            take_fields(&mut body, &write.archive, &ARCHIVE_FIELDS);
            let Ok(bytes) = serde_json::to_vec(&body) else {
                continue;
            };
            let written = if write.add_only {
                storage::write_new(&file, &bytes)
            } else {
                storage::write_atomically(&file, &bytes)
            };
            if written.is_ok() {
                if let Some(stamp) = date_copy(&file, &bytes, write.modified) {
                    let entry = Entry::new(&bytes, &body, stamp.modified);
                    cache.0.insert(file, (stamp, Some(Arc::new(entry))));
                }
                copies.insert(write.name.clone(), copy_record(&body));
                self.report.written += 1;
                if write.add_only {
                    self.report.added_to_desktop += 1;
                }
                wrote_any = true;
            }
        }
        if wrote_any {
            self.report.folders += 1;
            self.written.insert(folder.key.clone());
            self.record.folders.insert(folder.key.clone(), copies);
            save(paths, &self.record);
        }
        self.pending.is_empty().then(|| self.finish(paths))
    }

    /// Saves the finished run's record: every copy as it is now, and what
    /// was decided.
    fn finish(&mut self, paths: &AppPaths) -> Run {
        let folders = self
            .read
            .folders
            .iter()
            .map(|folder| {
                let copies = match self.record.folders.get(&folder.key) {
                    Some(copies) if self.written.contains(&folder.key) => copies.clone(),
                    _ => copy_records(folder),
                };
                (folder.key.clone(), copies)
            })
            .collect();
        self.record = Record {
            folders,
            sessions: std::mem::take(&mut self.decided),
            desktop_folder: self.active_key.clone(),
            main_log_length: self.log_length,
        };
        save(paths, &self.record);
        Run::Synced(self.report.clone())
    }
}

/// A copy is never newer than what it was made from, so the folder Claude
/// wrote last still tells its Active Profile: dates the copy just written to
/// `file` as `modified`, and returns its stamp. `None` when the file there
/// is no longer the one sync wrote.
fn date_copy(file: &Path, bytes: &[u8], modified: SystemTime) -> Option<Stamp> {
    let mut copy = fs::File::options().read(true).write(true).open(file).ok()?;
    let mut now_there = Vec::with_capacity(bytes.len());
    copy.read_to_end(&mut now_there).ok()?;
    if now_there != bytes {
        return None;
    }
    let _ = copy.set_modified(modified);
    Stamp::of(&copy.metadata().ok()?)
}

/// Resuming after a Safety pause: the user accepts the folders as they are
/// now, so the next run compares against them.
pub(crate) fn accept_current(env: &Environment, paths: &AppPaths, cache: &mut EntryCache) {
    let desktop_paths = DesktopPaths::new(&env.home);
    let read = read_all(&env.home, false, cache);
    let mut record = load(paths);
    record.folders = read
        .folders
        .iter()
        .map(|folder| (folder.key.clone(), copy_records(folder)))
        .collect();
    record.desktop_folder = active_key(&desktop_paths);
    record.main_log_length = desktop::main_log_length(&desktop_paths);
    save(paths, &record);
}

/// One entry to create or replace.
struct Write {
    folder: usize,
    name: String,
    /// The folder of the copy every field but the title and archive state
    /// comes from.
    source: usize,
    /// The title fields and archive state it gets.
    title: Map<String, Value>,
    archive: Map<String, Value>,
    /// The newest modification time among the copies it was made from.
    modified: SystemTime,
    /// Into a folder Claude may be using: only if no file has the name.
    add_only: bool,
    /// Into the source's folder, which keeps its own account-bound fields.
    own_folder: bool,
}

/// Decides every Session's entry and which folders need it written, and
/// counts the entries left as they are in `add_only` folders.
fn plan(
    read: &Read,
    record: &Record,
    active: Option<usize>,
    add_only: &impl Fn(&Folder) -> bool,
) -> (Vec<Write>, BTreeMap<String, Groups>, usize) {
    let names: BTreeSet<&String> = read
        .folders
        .iter()
        .flat_map(|folder| folder.copies.keys())
        .filter(|name| !read.skipped.contains_key(*name))
        .collect();
    let mut writes = Vec::new();
    let mut decided = BTreeMap::new();
    let mut held = 0;
    for name in names {
        let copies: Vec<Copy<'_>> = read
            .folders
            .iter()
            .enumerate()
            .filter_map(|(index, folder)| {
                let entry = folder.copies.get(name)?;
                let recorded = record.folders.get(&folder.key).and_then(|f| f.get(name));
                Some(Copy {
                    folder: index,
                    entry: entry.as_ref(),
                    recorded,
                })
            })
            .collect();
        let agreed = record.sessions.get(name);
        let source = copies
            .iter()
            .max_by_key(|copy| {
                (
                    copy.entry.last_activity,
                    Some(copy.folder) == active,
                    copy.entry.modified,
                )
            })
            .expect("a Session has a copy");
        let title = winner(&copies, active, |g| &g.title, agreed);
        let archive = winner(&copies, active, |g| &g.archive, agreed);
        let modified = [source, title, archive]
            .iter()
            .map(|copy| copy.entry.modified)
            .max()
            .expect("three copies");
        decided.insert(
            name.clone(),
            Groups {
                title: title.entry.record.groups.title.clone(),
                archive: archive.entry.record.groups.archive.clone(),
            },
        );
        // The account-bound fields alone differing (the desktop app rebuilds
        // them in a copy once it's used) leave a copy as it is.
        let holds = |entry: &Entry| {
            entry.rest == source.entry.rest
                && entry.title == title.entry.title
                && entry.archive == archive.entry.archive
        };
        let transcript = source.entry.transcript.as_deref();
        for (index, folder) in read.folders.iter().enumerate() {
            let existing = folder.copies.get(name);
            if existing.is_none() && transcript.is_some_and(|id| folder.tombstones.contains(id)) {
                continue;
            }
            if existing.is_some_and(|entry| holds(entry)) {
                continue;
            }
            let add_only = add_only(folder);
            if add_only && existing.is_some() {
                held += 1;
                continue;
            }
            writes.push(Write {
                folder: index,
                name: name.clone(),
                source: source.folder,
                title: title.entry.title.clone(),
                archive: archive.entry.archive.clone(),
                modified,
                add_only,
                own_folder: index == source.folder,
            });
        }
    }
    (writes, decided, held)
}

struct Copy<'a> {
    folder: usize,
    entry: &'a Entry,
    recorded: Option<&'a CopyRecord>,
}

/// The copy whose `fields` win: the one that changed since sync's record; if
/// several did, the desktop's Active Profile's, else the newest file. If none
/// did, the one holding what sync decided last time, else the same order.
fn winner<'a, 'b>(
    copies: &'b [Copy<'a>],
    active: Option<usize>,
    group: impl Fn(&Groups) -> &String,
    agreed: Option<&Groups>,
) -> &'b Copy<'a> {
    let changed: Vec<&Copy<'a>> = copies
        .iter()
        .filter(|copy| {
            copy.recorded.map(|r| group(&r.groups)) != Some(group(&copy.entry.record.groups))
        })
        .collect();
    let preferred = |among: Vec<&'b Copy<'a>>| {
        among
            .iter()
            .find(|copy| Some(copy.folder) == active)
            .copied()
            .or_else(|| among.iter().max_by_key(|copy| copy.entry.modified).copied())
    };
    match changed.len() {
        0 => agreed
            .and_then(|agreed| {
                copies
                    .iter()
                    .find(|copy| group(&copy.entry.record.groups) == group(agreed))
            })
            .or_else(|| preferred(copies.iter().collect())),
        1 => Some(changed[0]),
        _ => preferred(changed),
    }
    .expect("a Session has a copy")
}

fn pick(body: &Map<String, Value>, fields: &[&str]) -> Map<String, Value> {
    fields
        .iter()
        .filter_map(|field| Some(((*field).to_owned(), body.get(*field)?.clone())))
        .collect()
}

/// Replaces `fields` in `body` with `from`'s (removing those `from` lacks).
fn take_fields(body: &mut Map<String, Value>, from: &Map<String, Value>, fields: &[&str]) {
    for field in fields {
        match from.get(*field) {
            Some(value) => body.insert((*field).to_owned(), value.clone()),
            None => body.remove(*field),
        };
    }
}

/// Drops [`DROPPED`] and empties the account-bound fields that are present: lists to `[]`,
/// anything else to `{}`.
fn clear(body: &mut Map<String, Value>) {
    for field in DROPPED {
        body.remove(field);
    }
    for field in CLEARED {
        if let Some(value) = body.get_mut(field) {
            *value = match value {
                Value::Null => Value::Null,
                Value::Array(_) => Value::Array(Vec::new()),
                _ => Value::Object(Map::new()),
            };
        }
    }
}

/// A hash of every field but the title, the archive state and the
/// account-bound ones: with the title and archive state, it tells whether a
/// copy already holds the entry sync decided on.
fn rest_hash(body: &Map<String, Value>) -> u64 {
    struct Rest<'a>(&'a Map<String, Value>);
    impl Serialize for Rest<'_> {
        fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            let rest = |key: &String| {
                !CLEARED
                    .iter()
                    .chain(&DROPPED)
                    .chain(&TITLE_FIELDS)
                    .chain(&ARCHIVE_FIELDS)
                    .any(|field| key == field)
            };
            serializer.collect_map(self.0.iter().filter(|(key, _)| rest(key)))
        }
    }
    let mut hasher = StdHash(DefaultHasher::new());
    serde_json::to_writer(&mut hasher, &Rest(body)).expect("a JSON map always serializes");
    hasher.0.finish()
}

fn groups(body: &Map<String, Value>) -> Groups {
    Groups {
        title: hash(&pick(body, &TITLE_FIELDS)),
        archive: hash(&pick(body, &ARCHIVE_FIELDS)),
    }
}

fn copy_record(body: &Map<String, Value>) -> CopyRecord {
    CopyRecord {
        content: hash(body),
        groups: groups(body),
    }
}

fn copy_records(folder: &Folder) -> BTreeMap<String, CopyRecord> {
    folder
        .copies
        .iter()
        .map(|(name, entry)| (name.clone(), entry.record.clone()))
        .collect()
}

/// A 64-bit FNV-1a hash of the body's compact JSON (keys sorted), in hex,
/// hashed as it is written out rather than from a copy of the text.
fn hash(body: &Map<String, Value>) -> String {
    let mut hasher = Fnv::default();
    serde_json::to_writer(&mut hasher, body).expect("a JSON map always serializes");
    format!("{:016x}", hasher.0)
}

/// A file's bytes, hashed to tell within one run whether it changed. Never
/// stored, so it uses the fast standard hasher rather than FNV.
fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hasher = DefaultHasher::new();
    hasher.write(bytes);
    hasher.finish()
}

/// Feeds what is written to the standard hasher, for hashes never stored.
struct StdHash(DefaultHasher);

impl io::Write for StdHash {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.write(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl io::Write for Fnv {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        for byte in bytes {
            self.0 ^= u64::from(*byte);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// Reads every `<account>/<org>/` folder (real folders with uuid names),
/// parsing only the entries changed since `cache`, which then holds every
/// entry as read now.
fn read_all(home: &Path, include_optional: bool, cache: &mut EntryCache) -> Read {
    let root = sidebar_root(home);
    let mut read = Read {
        folders: Vec::new(),
        skipped: BTreeMap::new(),
        parsed: 0,
    };
    let mut before = std::mem::take(&mut cache.0);
    for (account, account_path) in uuid_folders(&root) {
        for (org, path) in uuid_folders(&account_path) {
            let mut folder = Folder {
                key: format!("{account}/{org}"),
                account: account.clone(),
                path,
                copies: BTreeMap::new(),
                tombstones: BTreeSet::new(),
            };
            read_folder(&mut folder, &mut read, include_optional, &mut before, cache);
            read.folders.push(folder);
        }
    }
    read
}

fn read_folder(
    folder: &mut Folder,
    read: &mut Read,
    include_optional: bool,
    before: &mut HashMap<PathBuf, (Stamp, Option<Arc<Entry>>)>,
    cache: &mut EntryCache,
) {
    let Ok(entries) = fs::read_dir(&folder.path) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if let Some(id) = name.strip_prefix("deleted_") {
            folder.tombstones.insert(id.to_owned());
            continue;
        }
        let Some(stem) = name
            .strip_suffix(".json")
            .filter(|stem| stem.starts_with("local_"))
        else {
            continue;
        };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_ENTRY_BYTES {
            continue;
        }
        let Some(stamp) = Stamp::of(&meta) else {
            continue;
        };
        let path = entry.path();
        let summary = match before.remove(&path) {
            Some((seen, summary)) if seen == stamp => summary,
            _ => {
                let Ok(bytes) = fs::read(&path) else {
                    continue;
                };
                read.parsed += 1;
                parse_body(&bytes)
                    // A copy under another name would be a second entry in Claude.
                    .filter(|body| body.get("sessionId").and_then(Value::as_str) == Some(stem))
                    .map(|body| Arc::new(Entry::new(&bytes, &body, stamp.modified)))
            }
        };
        cache.0.insert(path, (stamp, summary.clone()));
        let Some(summary) = summary else {
            continue;
        };
        if let Some(why) = summary
            .left_out
            .filter(|why| !include_optional || why.always())
        {
            read.skipped.insert(name.clone(), why);
        }
        folder.copies.insert(name, summary);
    }
}

/// Why sync doesn't copy this entry, if it doesn't: WSL and scheduled-task
/// Sessions, and SSH and No-folder ones unless "Include SSH and No-folder
/// Sessions" is on ([`LeftOut::always`]). A No-folder Session's copy keeps
/// pointing at its scratch workspace
/// (`scratch-workspaces/<account>/<org>/<id>`), which stays where it is.
fn left_out(body: &Map<String, Value>) -> Option<LeftOut> {
    let has = |markers: &[&str]| {
        markers
            .iter()
            .any(|marker| body.get(*marker).is_some_and(|value| !value.is_null()))
    };
    let scratch = |field: &str| {
        body.get(field)
            .and_then(Value::as_str)
            .is_some_and(|cwd| cwd.contains("/scratch-workspaces/"))
    };
    if has(&WSL_MARKERS) {
        Some(LeftOut::Wsl)
    } else if has(&SCHEDULED_MARKERS) {
        Some(LeftOut::ScheduledTask)
    } else if has(&SSH_MARKERS) {
        Some(LeftOut::Ssh)
    } else if scratch("cwd") || scratch("originCwd") {
        Some(LeftOut::NoFolder)
    } else {
        None
    }
}

impl Read {
    /// The Sessions left out, counted by why.
    fn left_out(&self) -> BTreeMap<LeftOut, usize> {
        let mut counts = BTreeMap::new();
        for why in self.skipped.values() {
            *counts.entry(*why).or_insert(0) += 1;
        }
        counts
    }
}

/// Real subfolders whose names are uuids, sorted.
fn uuid_folders(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let uuid = name.len() == 36 && name.chars().all(|c| c.is_ascii_hexdigit() || c == '-');
            uuid.then(|| (name, entry.path()))
        })
        .collect();
    found.sort();
    found
}

fn load(paths: &AppPaths) -> Record {
    storage::read_versioned::<Record>(&record_path(paths))
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn save(paths: &AppPaths, record: &Record) {
    // A lost record only means the next run treats every copy as changed
    // and falls back to the Active Profile's or the newest copy.
    let _ = storage::write_versioned(&record_path(paths), record);
}
