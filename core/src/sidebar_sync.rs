//! Sidebar sync (ADR 0003): makes every Session's Sidebar entry exist in
//! every Profile folder under `claude-code-sessions/<account>/<org>/`.
//!
//! - **Where it writes.** Every existing Profile folder (signed in to
//!   AccRoster or not: a folder is a Profile's sidebar). While Claude runs,
//!   never into a folder of the desktop's account: Claude reads and
//!   overwrites its own folder from memory, and its Active Organization
//!   can't be read from disk, so every Organization of that account is left
//!   alone. If Claude runs and its account can't be read, nothing is written.
//!   Folders are never created and files are never deleted.
//! - **What it copies.** `local_*.json` entries only (never one for a
//!   CLI-only Session), skipping scratch, SSH/WSL and scheduled-task
//!   Sessions, files over 10 MB, empty and unparsable files. A copy keeps the
//!   file name and every field; `bridgeSessionIds`, `remoteMcpServersConfig`
//!   and `enabledMcpTools` are cleared when present. A `deleted_<id>`
//!   tombstone in a folder stops the Session being added there again.
//! - **Which copy wins.** Entries carry no per-field times, so sync keeps
//!   its own record (`state/sidebar-sync.json`, hashes only) of each copy as
//!   of its last run. Title and archive state each come from the copy that
//!   differs from its record; if several do, the desktop's Active Profile's
//!   copy, else the newest file. Every other field comes from the copy with
//!   the newest `lastActivityAt`, which holds the current `cliSessionId`.
//! - **Before writing** (ADR 0004): every entry read is checked for unknown
//!   top-level fields; a target folder Claude wasn't using that changed since
//!   sync's record (and no account switch since) is the dormant multi-account
//!   mode's trace. Either stops the whole run before any write, for a Safety
//!   pause. Then each target folder is backed up, each exact file is read
//!   again, and each copy is written as temp file then rename.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::backups;
use crate::checks::SIDEBAR_ENTRY_FIELDS;
use crate::desktop::{self, DesktopPaths};
use crate::env::Environment;
use crate::safety::reportable;
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

/// The fields that make up an entry's title, and its archive state.
const TITLE_FIELDS: [&str; 5] = [
    "title",
    "titleSource",
    "titleTurn",
    "previousTitles",
    "titleOffers",
];
const ARCHIVE_FIELDS: [&str; 2] = ["isArchived", "autoArchiveExempt"];

/// Entries with any of these are SSH, WSL or scheduled-task Sessions.
const SKIP_MARKERS: [&str; 6] = [
    "sshConfig",
    "sshRemoteTranscriptPath",
    "sshRemoteProcessId",
    "wslConfig",
    "scheduledTaskId",
    "notifySessionId",
];

/// What one run did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Run {
    Synced(Report),
    /// Claude runs but its account can't be read, so any folder could be
    /// the one it uses: nothing was written.
    DesktopAccountUnknown,
    /// Stopped before any write: entries carry fields the app doesn't know
    /// (reportable names).
    UnknownFields(BTreeSet<String>),
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
    /// Claude was running, so its account's folders were left alone.
    pub left_desktop_account: bool,
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
    copies: BTreeMap<String, Entry>,
    /// Transcript ids of `deleted_<id>` tombstones.
    tombstones: BTreeSet<String>,
}

struct Entry {
    bytes: Vec<u8>,
    body: Map<String, Value>,
    modified: SystemTime,
}

/// Everything under `claude-code-sessions/`, plus what the checks found.
struct Read {
    folders: Vec<Folder>,
    /// File names of entries sync never copies (scratch, SSH/WSL, scheduled).
    skipped: BTreeSet<String>,
    unknown_fields: BTreeSet<String>,
}

/// "Sync now".
pub(crate) fn run(env: &Environment, paths: &AppPaths) -> Run {
    let desktop_paths = DesktopPaths::new(&env.home);
    let desktop = desktop::desktop_account(&desktop_paths);
    let running = env.os.desktop_app_running();
    if running && desktop.is_none() {
        return Run::DesktopAccountUnknown;
    }
    let read = read_all(&env.home);
    if !read.unknown_fields.is_empty() {
        return Run::UnknownFields(read.unknown_fields);
    }
    let record = load(paths);
    let active_key = desktop.as_ref().and_then(|account| {
        let org = account.organization_uuid.as_ref()?;
        Some(format!("{}/{org}", account.account_uuid))
    });
    let active = read
        .folders
        .iter()
        .position(|folder| Some(&folder.key) == active_key.as_ref());
    let writable = |folder: &Folder| {
        !running
            || desktop
                .as_ref()
                .is_some_and(|account| account.account_uuid != folder.account)
    };

    let (writes, decided) = plan(&read, &record, active, &writable);

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
            .filter(|folder| writable(folder) && folder.account != desktop_account)
            .filter(|folder| {
                record
                    .folders
                    .get(&folder.key)
                    .is_some_and(|recorded| *recorded != copy_records(folder))
            })
            .count();
        if changed > 0 {
            return Run::FoldersChanged(changed);
        }
    }

    let mut report = Report {
        left_desktop_account: running,
        ..Report::default()
    };
    let mut written: BTreeMap<(usize, String), Map<String, Value>> = BTreeMap::new();
    let now = env.clock.now();
    let by_folder: BTreeMap<usize, Vec<&Write>> =
        writes.iter().fold(BTreeMap::new(), |mut map, write| {
            map.entry(write.folder).or_insert_with(Vec::new).push(write);
            map
        });
    for (index, folder_writes) in by_folder {
        let folder = &read.folders[index];
        if backups::back_up_folder(&paths.backups(), &folder.path, now).is_err() {
            report.not_backed_up += 1;
            continue;
        }
        let mut wrote_any = false;
        for write in folder_writes {
            let file = folder.path.join(&write.name);
            // The exact file, read again right before it's replaced.
            let now_there = match fs::read(&file) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(_) => continue,
            };
            let expected = folder.copies.get(&write.name).map(|entry| &entry.bytes);
            if now_there.as_ref() != expected {
                return Run::FoldersChanged(1);
            }
            let Ok(bytes) = serde_json::to_vec(&Value::Object(write.body.clone())) else {
                continue;
            };
            if storage::write_atomically(&file, &bytes).is_ok() {
                // A copy is never newer than what it was made from, so the
                // folder Claude wrote last still tells its Active Profile.
                let _ = fs::File::options()
                    .write(true)
                    .open(&file)
                    .and_then(|copy| copy.set_modified(write.modified));
                written.insert((index, write.name.clone()), write.body.clone());
                report.written += 1;
                wrote_any = true;
            }
        }
        if wrote_any {
            report.folders += 1;
        }
    }

    // The new record: every copy as it is now, and what was decided.
    let mut new_record = Record {
        folders: BTreeMap::new(),
        sessions: decided,
        desktop_folder: active_key,
        main_log_length: log_length,
    };
    for (index, folder) in read.folders.iter().enumerate() {
        let mut copies = copy_records(folder);
        for ((written_in, name), body) in &written {
            if *written_in == index {
                copies.insert(name.clone(), copy_record(body));
            }
        }
        new_record.folders.insert(folder.key.clone(), copies);
    }
    save(paths, &new_record);
    Run::Synced(report)
}

/// Resuming after a Safety pause: the user accepts the folders as they are
/// now, so the next run compares against them.
pub(crate) fn accept_current(env: &Environment, paths: &AppPaths) {
    let desktop_paths = DesktopPaths::new(&env.home);
    let read = read_all(&env.home);
    let mut record = load(paths);
    record.folders = read
        .folders
        .iter()
        .map(|folder| (folder.key.clone(), copy_records(folder)))
        .collect();
    record.desktop_folder = desktop::desktop_account(&desktop_paths).and_then(|account| {
        let org = account.organization_uuid?;
        Some(format!("{}/{org}", account.account_uuid))
    });
    record.main_log_length = desktop::main_log_length(&desktop_paths);
    save(paths, &record);
}

/// One entry to create or replace.
struct Write {
    folder: usize,
    name: String,
    body: Map<String, Value>,
    /// The newest modification time among the copies it was made from.
    modified: SystemTime,
}

/// Decides every Session's entry and which folders need it written.
fn plan(
    read: &Read,
    record: &Record,
    active: Option<usize>,
    writable: &impl Fn(&Folder) -> bool,
) -> (Vec<Write>, BTreeMap<String, Groups>) {
    let names: BTreeSet<&String> = read
        .folders
        .iter()
        .flat_map(|folder| folder.copies.keys())
        .filter(|name| !read.skipped.contains(*name))
        .collect();
    let mut writes = Vec::new();
    let mut decided = BTreeMap::new();
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
                    entry,
                    recorded,
                })
            })
            .collect();
        let agreed = record.sessions.get(name);
        let source = copies
            .iter()
            .max_by_key(|copy| {
                (
                    copy.entry
                        .body
                        .get("lastActivityAt")
                        .and_then(Value::as_i64),
                    Some(copy.folder) == active,
                    copy.entry.modified,
                )
            })
            .expect("a Session has a copy");
        let title = winner(&copies, &TITLE_FIELDS, active, |g| &g.title, agreed);
        let archive = winner(&copies, &ARCHIVE_FIELDS, active, |g| &g.archive, agreed);
        let modified = [source, title, archive]
            .iter()
            .map(|copy| copy.entry.modified)
            .max()
            .expect("three copies");
        let mut body = source.entry.body.clone();
        take_fields(&mut body, &title.entry.body, &TITLE_FIELDS);
        take_fields(&mut body, &archive.entry.body, &ARCHIVE_FIELDS);
        decided.insert(name.clone(), groups(&body));
        let transcript = body.get("cliSessionId").and_then(Value::as_str);
        for (index, folder) in read.folders.iter().enumerate() {
            if !writable(folder) {
                continue;
            }
            let existing = folder.copies.get(name);
            if existing.is_none() && transcript.is_some_and(|id| folder.tombstones.contains(id)) {
                continue;
            }
            let mut wanted = body.clone();
            if index != source.folder {
                clear(&mut wanted);
            }
            if existing.is_some_and(|entry| same_but_cleared(&entry.body, &wanted)) {
                continue;
            }
            writes.push(Write {
                folder: index,
                name: name.clone(),
                body: wanted,
                modified,
            });
        }
    }
    (writes, decided)
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
    fields: &[&str],
    active: Option<usize>,
    group: impl Fn(&Groups) -> &String,
    agreed: Option<&Groups>,
) -> &'b Copy<'a> {
    let value = |copy: &Copy<'_>| hash(&Value::Object(pick(&copy.entry.body, fields)));
    let changed: Vec<&Copy<'a>> = copies
        .iter()
        .filter(|copy| copy.recorded.map(|r| group(&r.groups)) != Some(&value(copy)))
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
            .and_then(|agreed| copies.iter().find(|copy| value(copy) == *group(agreed)))
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

/// Empties the account-bound fields that are present: lists to `[]`,
/// anything else to `{}`.
fn clear(body: &mut Map<String, Value>) {
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

/// Equal apart from the cleared fields, which the desktop app rebuilds in a
/// copy once it's used: those differences alone are left as they are.
fn same_but_cleared(a: &Map<String, Value>, b: &Map<String, Value>) -> bool {
    let strip = |body: &Map<String, Value>| {
        let mut body = body.clone();
        for field in CLEARED {
            body.remove(field);
        }
        body
    };
    strip(a) == strip(b)
}

fn groups(body: &Map<String, Value>) -> Groups {
    Groups {
        title: hash(&Value::Object(pick(body, &TITLE_FIELDS))),
        archive: hash(&Value::Object(pick(body, &ARCHIVE_FIELDS))),
    }
}

fn copy_record(body: &Map<String, Value>) -> CopyRecord {
    CopyRecord {
        content: hash(&Value::Object(body.clone())),
        groups: groups(body),
    }
}

fn copy_records(folder: &Folder) -> BTreeMap<String, CopyRecord> {
    folder
        .copies
        .iter()
        .map(|(name, entry)| (name.clone(), copy_record(&entry.body)))
        .collect()
}

/// A 64-bit FNV-1a hash of the value's JSON (keys sorted), in hex.
fn hash(value: &Value) -> String {
    let text = value.to_string();
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in text.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// Reads every `<account>/<org>/` folder (real folders with uuid names).
fn read_all(home: &Path) -> Read {
    let root = home.join("Library/Application Support/Claude/claude-code-sessions");
    let mut read = Read {
        folders: Vec::new(),
        skipped: BTreeSet::new(),
        unknown_fields: BTreeSet::new(),
    };
    for (account, account_path) in uuid_folders(&root) {
        for (org, path) in uuid_folders(&account_path) {
            let mut folder = Folder {
                key: format!("{account}/{org}"),
                account: account.clone(),
                path,
                copies: BTreeMap::new(),
                tombstones: BTreeSet::new(),
            };
            read_folder(&mut folder, &mut read);
            read.folders.push(folder);
        }
    }
    read
}

fn read_folder(folder: &mut Folder, read: &mut Read) {
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
        let Ok(bytes) = fs::read(entry.path()) else {
            continue;
        };
        let Ok(Value::Object(body)) = serde_json::from_slice(&bytes) else {
            continue;
        };
        // A copy under another name would be a second entry in Claude.
        if body.get("sessionId").and_then(Value::as_str) != Some(stem) {
            continue;
        }
        for key in body.keys() {
            if !SIDEBAR_ENTRY_FIELDS.contains(&key.as_str()) {
                read.unknown_fields.insert(reportable(key));
            }
        }
        if never_copied(&body) {
            read.skipped.insert(name.clone());
        }
        let Ok(modified) = meta.modified() else {
            continue;
        };
        folder.copies.insert(
            name,
            Entry {
                bytes,
                body,
                modified,
            },
        );
    }
}

/// Scratch, SSH/WSL and scheduled-task Sessions.
fn never_copied(body: &Map<String, Value>) -> bool {
    let scratch = |field: &str| {
        body.get(field)
            .and_then(Value::as_str)
            .is_some_and(|cwd| cwd.contains("/scratch-workspaces/"))
    };
    scratch("cwd")
        || scratch("originCwd")
        || SKIP_MARKERS
            .iter()
            .any(|marker| body.get(*marker).is_some_and(|value| !value.is_null()))
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
