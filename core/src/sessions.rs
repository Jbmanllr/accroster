//! The Session index: the Unified session view's model.
//!
//! Every Session on the Mac is one row, keyed by its Transcript id. The
//! sources, all read-only:
//!
//! - top-level Transcripts `~/.claude/projects/*/*.jsonl` (subagent
//!   Transcripts and links skipped), read incrementally and remembered in the
//!   disposable `index/sessions.json`, keyed by path, size and mtime;
//! - every Sidebar entry in every `claude-code-sessions/<account>/<org>/`
//!   folder, joined to its Transcript by `cliSessionId` (scratch, SSH/WSL and
//!   scheduled-task entries skipped);
//! - live-process files `sessions/<pid>.json` in `~/.claude` and every Profile
//!   folder, for the running dots.
//!
//! Building runs in batches ([`SessionIndex::step`]) so the list fills in while
//! the core keeps handling input. Nothing here writes outside the app's own
//! `index/` folder, and titles and prompts are never logged.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::env::Environment;
use crate::interface::{AllSessionsView, RunningDot, SessionRow};
use crate::storage::{self, AppPaths, VersionedFile};

/// Transcripts read per [`SessionIndex::step`].
const BATCH: usize = 20;

/// The longest first prompt kept for the title and for search, in characters.
const PROMPT_KEPT: usize = 500;

/// The longest first prompt shown as a title, in characters.
const PROMPT_SHOWN: usize = 48;

/// Sidebar entries over this size are skipped, as the desktop app does.
const ENTRY_MAX_BYTES: u64 = 10 * 1024 * 1024;

/// The desktop app's entrypoint in live-process files and Transcripts.
const DESKTOP_ENTRYPOINT: &str = "claude-desktop";

/// What one Transcript contributes, as of `offset` bytes read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Transcript {
    path: PathBuf,
    size: u64,
    mtime_ns: u64,
    /// Where the next read starts: just after the last complete line.
    offset: u64,
    /// The Transcript id (the file name without `.jsonl`).
    id: String,
    /// The last `custom-title`.
    custom_title: Option<String>,
    /// The last `ai-title`.
    ai_title: Option<String>,
    first_prompt: Option<String>,
    cwd: Option<String>,
    branch: Option<String>,
    /// The last `entrypoint` seen (the client it was last used in).
    entrypoint: Option<String>,
    /// The newest record `timestamp`, epoch ms.
    last_activity_ms: Option<i64>,
    /// Real prompts (not tool results, meta or compact summaries).
    turns: u64,
}

impl Transcript {
    fn new(path: PathBuf, id: String) -> Transcript {
        Transcript {
            path,
            size: 0,
            mtime_ns: 0,
            offset: 0,
            id,
            custom_title: None,
            ai_title: None,
            first_prompt: None,
            cwd: None,
            branch: None,
            entrypoint: None,
            last_activity_ms: None,
            turns: 0,
        }
    }

    /// Reads the complete lines after `offset`. A partly written last line is
    /// left for the next read.
    fn read_on(&mut self) -> io::Result<()> {
        let mut file = File::open(&self.path)?;
        file.seek(SeekFrom::Start(self.offset))?;
        let mut reader = BufReader::new(file);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = reader.read_until(b'\n', &mut line)?;
            if read == 0 || line.last() != Some(&b'\n') {
                return Ok(());
            }
            self.offset += read as u64;
            if let Ok(record) = serde_json::from_slice::<Value>(&line) {
                self.take(&record);
            }
        }
    }

    /// Folds one record in. Unknown types and fields are ignored.
    fn take(&mut self, record: &Value) {
        let text = |name: &str| {
            record
                .get(name)
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
                .map(str::to_owned)
        };
        match record.get("type").and_then(Value::as_str) {
            Some("custom-title") => {
                self.custom_title = text("customTitle").or(self.custom_title.take())
            }
            Some("ai-title") => self.ai_title = text("aiTitle").or(self.ai_title.take()),
            Some("relocated") => self.cwd = text("relocatedCwd").or(self.cwd.take()),
            Some("user") => {
                if let Some(prompt) = prompt_text(record) {
                    self.turns += 1;
                    if self.first_prompt.is_none() {
                        self.first_prompt = Some(prompt.chars().take(PROMPT_KEPT).collect());
                    }
                }
            }
            _ => {}
        }
        if let Some(cwd) = text("cwd") {
            self.cwd = Some(cwd);
        }
        if let Some(branch) = text("gitBranch") {
            self.branch = Some(branch);
        }
        if let Some(entrypoint) = text("entrypoint") {
            self.entrypoint = Some(entrypoint);
        }
        let at = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(|at| at.parse::<Timestamp>().ok());
        if let Some(at) = at {
            let ms = at.as_millisecond();
            self.last_activity_ms = Some(self.last_activity_ms.map_or(ms, |last| last.max(ms)));
        }
    }
}

/// The text of a `user` record that is a real prompt: not meta, not a
/// compact summary, not a tool result, not a sidechain, and not a local
/// slash-command wrapper (`<command-name>…`, `<local-command-…>`).
fn prompt_text(record: &Value) -> Option<String> {
    let flag = |name: &str| record.get(name).and_then(Value::as_bool) == Some(true);
    if flag("isMeta") || flag("isCompactSummary") || flag("isSidechain") {
        return None;
    }
    if record.get("toolUseResult").is_some() || record.get("sourceToolUseID").is_some() {
        return None;
    }
    let text = match record.pointer("/message/content")? {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => {
            let mut parts = Vec::new();
            for block in blocks {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => parts.extend(block.get("text").and_then(Value::as_str)),
                    Some("tool_result") => return None,
                    _ => {}
                }
            }
            parts.join(" ")
        }
        _ => return None,
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() || text.starts_with("<command-") || text.starts_with("<local-command-") {
        return None;
    }
    Some(text)
}

/// `index/sessions.json`.
#[derive(Debug, Default, Serialize, Deserialize)]
struct IndexFile {
    transcripts: Vec<Transcript>,
}

impl VersionedFile for IndexFile {
    const VERSION: u64 = 1;
}

/// The fields of a Sidebar entry the index uses. A field of the wrong type
/// makes the entry unreadable, and it is skipped.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EntryFile {
    cli_session_id: Option<String>,
    title: Option<String>,
    title_source: Option<String>,
    last_activity_at: Option<i64>,
    cwd: Option<String>,
    ssh_config: Option<Value>,
    ssh_remote_transcript_path: Option<Value>,
    ssh_remote_process_id: Option<Value>,
    wsl_config: Option<Value>,
    scheduled_task_id: Option<Value>,
    notify_session_id: Option<Value>,
}

impl EntryFile {
    /// Scratch, SSH/WSL and scheduled-task entries aren't Sessions of this
    /// Mac's Transcripts.
    fn skipped(&self) -> bool {
        self.ssh_config.is_some()
            || self.ssh_remote_transcript_path.is_some()
            || self.ssh_remote_process_id.is_some()
            || self.wsl_config.is_some()
            || self.scheduled_task_id.is_some()
            || self.notify_session_id.is_some()
            || self
                .cwd
                .as_deref()
                .is_some_and(|cwd| cwd.contains("/scratch-workspaces/"))
    }
}

/// One Sidebar entry, in one Profile's folder.
#[derive(Debug, Clone)]
struct SidebarEntry {
    account: String,
    org: String,
    cli_session_id: String,
    title: Option<String>,
    /// `titleSource` is set and isn't `auto` (an entry with no `titleSource`
    /// counts as automatic).
    user_title: bool,
    last_activity_at: i64,
}

/// A live-process file's fields.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveFile {
    pid: u32,
    session_id: Option<String>,
    proc_start: Option<String>,
    entrypoint: Option<String>,
    status: Option<String>,
}

/// What a finished build found, for the log.
pub(crate) struct Built {
    pub transcripts: usize,
    pub read: usize,
}

/// The Session index and the All Sessions list's state.
#[derive(Default)]
pub(crate) struct SessionIndex {
    /// By path. Loaded from `index/sessions.json` on first open.
    transcripts: BTreeMap<PathBuf, Transcript>,
    loaded: bool,
    /// Transcripts still to read in this build, with their size and mtime.
    pending: Vec<(PathBuf, u64, u64)>,
    building: bool,
    read: usize,
    entries: Vec<SidebarEntry>,
    running: HashMap<String, RunningDot>,
    /// The desktop's Active Profile as (account, Organization).
    desktop: Option<(String, Option<String>)>,
    search: String,
}

impl SessionIndex {
    /// Starts a build: rereads Sidebar entries, live processes and the
    /// desktop's account, and queues every new or changed Transcript.
    pub(crate) fn open(&mut self, env: &Environment, paths: &AppPaths) {
        if !self.loaded {
            self.loaded = true;
            // Disposable: an unreadable index is rebuilt from scratch.
            if let Ok(Some(file)) = storage::read_versioned::<IndexFile>(&index_file(paths)) {
                self.transcripts = file
                    .transcripts
                    .into_iter()
                    .map(|t| (t.path.clone(), t))
                    .collect();
            }
        }
        let found = transcript_files(&env.home);
        self.transcripts.retain(|path, _| found.contains_key(path));
        self.pending = found
            .into_iter()
            .filter(|(path, (size, mtime))| {
                self.transcripts
                    .get(path)
                    .is_none_or(|t| t.size != *size || t.mtime_ns != *mtime)
            })
            .map(|(path, (size, mtime))| (path, size, mtime))
            .collect();
        self.building = true;
        self.read = 0;
        self.entries = sidebar_entries(&env.home);
        self.running = running(env, paths);
        self.desktop = desktop_account(&env.home);
    }

    /// Reads the next batch of Transcripts. Returns what was built once the
    /// last batch is done and the index is saved.
    pub(crate) fn step(&mut self, paths: &AppPaths) -> Option<io::Result<Built>> {
        if !self.building {
            return None;
        }
        let split = self.pending.len().saturating_sub(BATCH);
        for (path, size, mtime) in self.pending.split_off(split) {
            let Some(id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .map(str::to_owned)
            else {
                continue;
            };
            let mut transcript = match self.transcripts.remove(&path) {
                // Appended to: read on from where the last read stopped.
                Some(known) if size >= known.offset => known,
                _ => Transcript::new(path.clone(), id),
            };
            if transcript.read_on().is_err() {
                continue;
            }
            transcript.size = size;
            transcript.mtime_ns = mtime;
            self.read += 1;
            self.transcripts.insert(path, transcript);
        }
        if !self.pending.is_empty() {
            return None;
        }
        self.building = false;
        let file = IndexFile {
            transcripts: self.transcripts.values().cloned().collect(),
        };
        Some(
            storage::write_versioned(&index_file(paths), &file).map(|()| Built {
                transcripts: self.transcripts.len(),
                read: self.read,
            }),
        )
    }

    pub(crate) fn building(&self) -> bool {
        self.building
    }

    pub(crate) fn set_search(&mut self, search: String) {
        self.search = search;
    }

    /// The All Sessions list as of `now`.
    pub(crate) fn view(&self, now: Timestamp) -> AllSessionsView {
        let mut by_id: HashMap<&str, &Transcript> = HashMap::new();
        for transcript in self.transcripts.values() {
            let newer = by_id
                .get(transcript.id.as_str())
                .is_none_or(|other| other.last_activity_ms < transcript.last_activity_ms);
            if newer {
                by_id.insert(&transcript.id, transcript);
            }
        }
        let mut entries: HashMap<&str, Vec<&SidebarEntry>> = HashMap::new();
        for entry in &self.entries {
            entries
                .entry(entry.cli_session_id.as_str())
                .or_default()
                .push(entry);
        }
        let words: Vec<String> = self
            .search
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let mut rows: Vec<(i64, SessionRow)> = by_id
            .into_values()
            .filter_map(|transcript| {
                let joined = entries
                    .get(transcript.id.as_str())
                    .map_or(&[][..], Vec::as_slice);
                let title = self.title(transcript, joined);
                let project = project(transcript);
                let haystack = [
                    title.as_str(),
                    transcript.first_prompt.as_deref().unwrap_or(""),
                    project,
                    transcript.branch.as_deref().unwrap_or(""),
                ]
                .join("\n")
                .to_lowercase();
                if !words.iter().all(|word| haystack.contains(word.as_str())) {
                    return None;
                }
                let last = transcript
                    .last_activity_ms
                    .unwrap_or((transcript.mtime_ns / 1_000_000) as i64);
                Some((
                    last,
                    SessionRow {
                        id: transcript.id.clone(),
                        subtitle: format!("{project} · {}", age(now, last)),
                        title,
                        dot: self.running.get(&transcript.id).copied(),
                    },
                ))
            })
            .collect();
        rows.sort_by(|(a_at, a), (b_at, b)| b_at.cmp(a_at).then_with(|| a.id.cmp(&b.id)));
        AllSessionsView {
            rows: rows.into_iter().map(|(_, row)| row).collect(),
            search: self.search.clone(),
            loading: self.building,
        }
    }

    /// A user-set title beats an automatic one: Sidebar entries' user titles
    /// (the desktop's Active Profile's entry first, then the newest), then
    /// the Transcript's `custom-title`, then entries' automatic titles (same
    /// order), the Transcript's `ai-title`, and the first prompt in quotes.
    fn title(&self, transcript: &Transcript, entries: &[&SidebarEntry]) -> String {
        let best = |user: bool| {
            entries
                .iter()
                .filter(|entry| entry.user_title == user)
                .filter_map(|entry| Some((entry, entry.title.as_deref()?)))
                .max_by_key(|(entry, _)| (self.is_desktop(entry), entry.last_activity_at))
                .map(|(_, title)| title.to_owned())
        };
        best(true)
            .or_else(|| transcript.custom_title.clone())
            .or_else(|| best(false))
            .or_else(|| transcript.ai_title.clone())
            .or_else(|| transcript.first_prompt.as_deref().map(quoted))
            .unwrap_or_else(|| "Untitled Session".into())
    }

    fn is_desktop(&self, entry: &SidebarEntry) -> bool {
        self.desktop.as_ref().is_some_and(|(account, org)| {
            *account == entry.account && org.as_deref() == Some(entry.org.as_str())
        })
    }
}

fn index_file(paths: &AppPaths) -> PathBuf {
    paths.index().join("sessions.json")
}

/// The first prompt as a one-line title.
fn quoted(prompt: &str) -> String {
    let mut shown: String = prompt.chars().take(PROMPT_SHOWN).collect();
    if prompt.chars().count() > PROMPT_SHOWN {
        shown = format!("{}…", shown.trim_end());
    }
    format!("“{shown}”")
}

/// The project: the last folder of the Session's working directory.
fn project(transcript: &Transcript) -> &str {
    transcript
        .cwd
        .as_deref()
        .and_then(|cwd| Path::new(cwd).file_name())
        .and_then(|name| name.to_str())
        .unwrap_or("unknown folder")
}

/// `5 min ago`, `3 h ago`, `yesterday`, `4 days ago`.
fn age(now: Timestamp, at_ms: i64) -> String {
    let at = Timestamp::from_millisecond(at_ms).unwrap_or(now);
    let elapsed = now.duration_since(at).max(SignedDuration::ZERO);
    let minutes = elapsed.as_mins();
    match minutes {
        0 => "just now".into(),
        1..60 => format!("{minutes} min ago"),
        60..1440 => format!("{} h ago", minutes / 60),
        1440..2880 => "yesterday".into(),
        _ => format!("{} days ago", minutes / 1440),
    }
}

/// Every top-level `~/.claude/projects/<project>/<id>.jsonl`, with its size
/// and mtime. Links (folders or files) aren't followed, and subagent
/// Transcripts, one folder deeper, aren't reached.
fn transcript_files(home: &Path) -> BTreeMap<PathBuf, (u64, u64)> {
    let mut found = BTreeMap::new();
    for project in real_children(&home.join(".claude").join("projects")) {
        if !project.1.is_dir() {
            continue;
        }
        for (path, kind) in real_children(&project.0) {
            if !kind.is_file() || path.extension().is_none_or(|ext| ext != "jsonl") {
                continue;
            }
            let Ok(meta) = fs::symlink_metadata(&path) else {
                continue;
            };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |since| since.as_nanos() as u64);
            found.insert(path, (meta.len(), mtime));
        }
    }
    found
}

/// A folder's children with their own type (links stay links).
fn real_children(dir: &Path) -> Vec<(PathBuf, fs::FileType)> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|entry| Some((entry.path(), entry.file_type().ok()?)))
        .filter(|(_, kind)| !kind.is_symlink())
        .collect()
}

fn sidebar_root(home: &Path) -> PathBuf {
    home.join("Library/Application Support/Claude/claude-code-sessions")
}

/// Every readable Sidebar entry in every account/Organization folder.
fn sidebar_entries(home: &Path) -> Vec<SidebarEntry> {
    let mut found = Vec::new();
    for (account_dir, kind) in real_children(&sidebar_root(home)) {
        if !kind.is_dir() {
            continue;
        }
        for (org_dir, kind) in real_children(&account_dir) {
            if !kind.is_dir() {
                continue;
            }
            for (path, kind) in real_children(&org_dir) {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if !kind.is_file() || !name.starts_with("local_") || !name.ends_with(".json") {
                    continue;
                }
                let Some(entry) = read_entry(&path) else {
                    continue;
                };
                found.push(SidebarEntry {
                    account: file_name(&account_dir),
                    org: file_name(&org_dir),
                    cli_session_id: entry.cli_session_id.clone().unwrap_or_default(),
                    title: entry.title.filter(|title| !title.trim().is_empty()),
                    user_title: entry.title_source.is_some_and(|source| source != "auto"),
                    last_activity_at: entry.last_activity_at.unwrap_or(0),
                });
            }
        }
    }
    found.retain(|entry| !entry.cli_session_id.is_empty());
    found
}

/// A Sidebar entry the index uses: non-empty, at most 10 MB, parsable, and
/// not skipped.
fn read_entry(path: &Path) -> Option<EntryFile> {
    let size = fs::metadata(path).ok()?.len();
    if size == 0 || size > ENTRY_MAX_BYTES {
        return None;
    }
    let entry: EntryFile = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
    (!entry.skipped()).then_some(entry)
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Running dots by Transcript id, from the live-process files of live
/// processes in `~/.claude/sessions/` and every Profile's `sessions/`. A
/// file whose pid is dead, or whose pid now belongs to a process started at
/// another time, is stale and ignored.
fn running(env: &Environment, paths: &AppPaths) -> HashMap<String, RunningDot> {
    let mut folders = vec![env.home.join(".claude").join("sessions")];
    folders.extend(
        real_children(&paths.profiles())
            .into_iter()
            .filter(|(_, kind)| kind.is_dir())
            .map(|(profile, _)| profile.join("sessions")),
    );
    let time_zone = env.clock.time_zone();
    let mut dots = HashMap::new();
    for (path, kind) in folders.iter().flat_map(|folder| real_children(folder)) {
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let is_pid_file = name
            .strip_suffix(".json")
            .is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit()));
        if !kind.is_file() || !is_pid_file {
            continue;
        }
        let Some(live) = fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<LiveFile>(&bytes).ok())
        else {
            continue;
        };
        let Some(session_id) = live.session_id else {
            continue;
        };
        let alive = env.os.process(live.pid).is_some_and(|facts| {
            live.proc_start
                .as_deref()
                .and_then(|text| parse_proc_start(text, &time_zone))
                .is_some_and(|started| started.as_second() == facts.started_at.as_second())
        });
        if !alive {
            continue;
        }
        let dot = if live.entrypoint.as_deref() != Some(DESKTOP_ENTRYPOINT) {
            RunningDot::Terminal
        } else if live.status.as_deref() == Some("idle") {
            continue;
        } else {
            // Busy, waiting, or a status we don't know: fail closed.
            RunningDot::Desktop
        };
        let slot = dots.entry(session_id).or_insert(dot);
        if dot == RunningDot::Terminal {
            *slot = dot;
        }
    }
    dots
}

/// `procStart` as `ps -o lstart` prints it (`Sun Oct 25 14:02:11 2026`, the
/// day space-padded), in the Mac's local time.
fn parse_proc_start(text: &str, time_zone: &jiff::tz::TimeZone) -> Option<Timestamp> {
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

/// The desktop's Active Profile as far as its files tell: `config.json`'s
/// `lastKnownAccountUuid` (the only key read), and the Organization folder
/// under it holding the most recently written Sidebar entry.
fn desktop_account(home: &Path) -> Option<(String, Option<String>)> {
    /// Only the key needed; serde skips the rest (token caches included)
    /// without keeping them.
    #[derive(Deserialize)]
    struct DesktopConfig {
        #[serde(rename = "lastKnownAccountUuid")]
        last_known_account_uuid: Option<String>,
    }
    let config: DesktopConfig = serde_json::from_slice(
        &fs::read(home.join("Library/Application Support/Claude/config.json")).ok()?,
    )
    .ok()?;
    let account = config.last_known_account_uuid?;
    if account.is_empty() || account.contains('/') || account.starts_with('.') {
        return None;
    }
    let org = real_children(&sidebar_root(home).join(&account))
        .into_iter()
        .filter(|(_, kind)| kind.is_dir())
        .filter_map(|(org_dir, _)| {
            let newest = real_children(&org_dir)
                .into_iter()
                .filter(|(path, _)| {
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("local_") && n.ends_with(".json"))
                })
                .filter_map(|(path, _)| fs::metadata(path).ok()?.modified().ok())
                .max()?;
            Some((newest, file_name(&org_dir)))
        })
        .max()
        .map(|(_, org)| org);
    Some((account, org))
}
