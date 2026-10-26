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
//! the core keeps handling input. While the All Sessions window is open,
//! FSEvents paths ([`SessionIndex::files_changed`]) re-stat only their
//! sources; while it is closed nothing is watched, and a menu open does a
//! stat-only [`SessionIndex::rescan`]. Either way only changed files are read. Nothing here writes outside the app's own
//! `index/` folder, and titles and prompts are never logged.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::desktop::{self, DesktopAccount, DesktopPaths, NewestEntries};
use crate::env::Environment;
use crate::interface::{
    AllSessionsView, ProfileKey, RunningDot, SessionDetail, SessionFilter, SessionRow,
    SidebarGroup, SidebarItem, SidebarPresence,
};
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
    is_archived: Option<bool>,
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
    pair: ProfileKey,
    /// The entry's `local_…` id (its file name, which is always its
    /// `sessionId`).
    entry_id: String,
    cli_session_id: String,
    title: Option<String>,
    /// `titleSource` is set and isn't `auto` (an entry with no `titleSource`
    /// counts as automatic).
    user_title: bool,
    archived: bool,
    last_activity_at: i64,
    cwd: Option<String>,
}

/// A live-process file's fields.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LiveFile {
    pid: u32,
    session_id: Option<String>,
    proc_start: Option<String>,
    entrypoint: Option<String>,
    status: Option<String>,
}

/// A file's size and mtime (ns): when either changes, it is read again.
type Stamp = (u64, u64);

/// A small source file as last read: its stamp and what it held (`None`
/// when unreadable or skipped).
type Cached<T> = BTreeMap<PathBuf, (Stamp, Option<T>)>;

/// What a finished build found, for the log.
pub(crate) struct Built {
    pub transcripts: usize,
    pub read: usize,
}

/// A Profile's Sidebar as the All Sessions window names it.
pub(crate) struct SidebarProfile {
    pub pair: ProfileKey,
    /// The nickname, else the Profile name, else "Unknown Login …".
    pub name: String,
    /// It is a signed-in Profile in AccRoster.
    pub signed_in: bool,
}

/// The Session index and the All Sessions window's state.
#[derive(Default)]
pub(crate) struct SessionIndex {
    home: PathBuf,
    /// By path. Loaded from `index/sessions.json` on the first rescan.
    transcripts: BTreeMap<PathBuf, Transcript>,
    loaded: bool,
    /// Transcripts still to read, with their size and mtime.
    pending: Vec<(PathBuf, u64, u64)>,
    /// A full rescan is reading its batches (the list is filling in).
    building: bool,
    /// Transcripts read since the index was last saved.
    read: usize,
    entries: Cached<SidebarEntry>,
    /// Each Sidebar folder's mtime when its entries were last stat'ed.
    entry_folders: BTreeMap<PathBuf, u64>,
    live: Cached<LiveFile>,
    running: HashMap<String, Running>,
    /// The desktop's account and Organization, for title precedence and
    /// "desktop now".
    desktop: Option<DesktopAccount>,
    /// What finding the desktop's Organization last read.
    newest_entries: NewestEntries,
    /// The All Sessions window is open (its folders are watched).
    window_open: bool,
    search: String,
    filter: SessionFilter,
    selected: Option<String>,
}

impl SessionIndex {
    /// A stat-only rescan of every source: re-reads only the Sidebar entries
    /// and live-process files that changed, and queues every new or changed
    /// Transcript for [`SessionIndex::step`], which saves the index once the
    /// last batch is read.
    pub(crate) fn rescan(&mut self, env: &Environment, paths: &AppPaths) {
        self.home = env.home.clone();
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
        self.restat_transcripts();
        self.restat_entries();
        self.restat_live(env, paths);
        self.building = true;
    }

    /// The rescan at launch, so the first menu's Recent Sessions is filled:
    /// batches follow only when a Transcript is new or changed.
    pub(crate) fn warm_up(&mut self, env: &Environment, paths: &AppPaths) {
        self.rescan(env, paths);
        self.building = !self.pending.is_empty();
    }

    /// Opens the All Sessions window: a rescan, then the folders to watch
    /// while it stays open (only the first open asks for them).
    pub(crate) fn open(&mut self, env: &Environment, paths: &AppPaths) -> Option<Vec<PathBuf>> {
        self.rescan(env, paths);
        if self.window_open {
            return None;
        }
        self.window_open = true;
        Some(watched_folders(&env.home, paths))
    }

    /// The window closed: nothing is watched any more. Saves what was read
    /// since the last save.
    pub(crate) fn close(&mut self, paths: &AppPaths) -> io::Result<bool> {
        self.window_open = false;
        self.selected = None;
        self.save_if_read(paths)
    }

    pub(crate) fn window_open(&self) -> bool {
        self.window_open
    }

    /// FSEvents reported `changed`: re-stats only the sources those paths
    /// belong to (a path outside every source re-stats them all). A
    /// Transcript path is re-stated alone; the changed ones are read from
    /// their stored offset by the next [`SessionIndex::step`].
    pub(crate) fn files_changed(
        &mut self,
        env: &Environment,
        paths: &AppPaths,
        changed: &[PathBuf],
    ) {
        let projects = projects_root(&env.home);
        let sidebar = sidebar_root(&env.home);
        let live_roots = [env.home.join(".claude").join("sessions"), paths.profiles()];
        let mut transcripts = Vec::new();
        let (mut all_transcripts, mut entries, mut live) = (false, false, false);
        for path in changed {
            if let Ok(inside) = path.strip_prefix(&projects) {
                if inside.components().count() == 2 && is_jsonl(path) {
                    if !is_subagent(path) {
                        transcripts.push(path.clone());
                    }
                } else {
                    all_transcripts = true;
                }
            } else if path.starts_with(&sidebar) {
                entries = true;
            } else if live_roots.iter().any(|root| path.starts_with(root)) {
                live = true;
            } else {
                (all_transcripts, entries, live) = (true, true, true);
            }
        }
        if all_transcripts {
            self.restat_transcripts();
        } else {
            for path in transcripts {
                self.restat_transcript(path);
            }
        }
        if entries {
            self.restat_entries();
        }
        if live {
            self.restat_live(env, paths);
        }
    }

    /// Reads the next batch of Transcripts. Returns what was built once the
    /// last batch of a full rescan is read and the index is saved.
    pub(crate) fn step(&mut self, paths: &AppPaths) -> Option<io::Result<Built>> {
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
        if !self.pending.is_empty() || !self.building {
            return None;
        }
        self.building = false;
        let read = self.read;
        Some(self.save_if_read(paths).map(|_| Built {
            transcripts: self.transcripts.len(),
            read,
        }))
    }

    /// Transcripts are waiting to be read.
    pub(crate) fn busy(&self) -> bool {
        self.building || !self.pending.is_empty()
    }

    pub(crate) fn set_search(&mut self, search: String) {
        self.search = search;
    }

    pub(crate) fn set_filter(&mut self, filter: SessionFilter) {
        self.filter = filter;
    }

    pub(crate) fn select(&mut self, id: Option<String>) {
        self.selected = id;
    }

    fn save_if_read(&mut self, paths: &AppPaths) -> io::Result<bool> {
        if self.read == 0 {
            return Ok(false);
        }
        self.save(paths).map(|()| true)
    }

    fn save(&mut self, paths: &AppPaths) -> io::Result<()> {
        self.read = 0;
        let file = IndexFile {
            transcripts: self.transcripts.values().cloned().collect(),
        };
        storage::write_versioned(&index_file(paths), &file)
    }

    /// Stats every Transcript: drops vanished ones, queues new or changed
    /// ones.
    fn restat_transcripts(&mut self) {
        let found = transcript_files(&self.home);
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
    }

    /// Stats one Transcript path FSEvents named.
    fn restat_transcript(&mut self, path: PathBuf) {
        self.pending.retain(|(queued, ..)| *queued != path);
        let Some((size, mtime)) = regular_file_stamp(&path) else {
            self.transcripts.remove(&path);
            return;
        };
        let known = self
            .transcripts
            .get(&path)
            .is_some_and(|t| t.size == size && t.mtime_ns == mtime);
        if !known {
            self.pending.push((path, size, mtime));
        }
    }

    /// Re-reads the Sidebar entries whose files changed, and the desktop's
    /// account (it follows the Sidebar folder written last).
    ///
    /// Claude writes entries temp-file-then-rename, so every write changes
    /// its folder's mtime: a folder whose mtime is unchanged since its last
    /// listing is skipped without a stat per entry.
    fn restat_entries(&mut self) {
        let folders = entry_folders(&self.home);
        let listed: BTreeMap<PathBuf, u64> = folders
            .iter()
            .map(|(dir, _, _, modified)| (dir.clone(), *modified))
            .collect();
        if !listed.keys().eq(self.entry_folders.keys()) {
            self.entries
                .retain(|path, _| path.parent().is_some_and(|dir| listed.contains_key(dir)));
        }
        for (dir, account, org, modified) in folders {
            if self.entry_folders.get(&dir) == Some(&modified) {
                continue;
            }
            let files = entry_files(&dir, &account, &org);
            refresh_folder(&mut self.entries, &dir, files, |path, inside| {
                let [account, org, name] = inside else {
                    return None;
                };
                let entry = read_entry(path)?;
                Some(SidebarEntry {
                    pair: ProfileKey {
                        account_uuid: account.clone(),
                        organization_uuid: org.clone(),
                    },
                    entry_id: name.strip_suffix(".json")?.to_owned(),
                    cli_session_id: entry.cli_session_id.filter(|id| !id.is_empty())?,
                    title: entry.title.filter(|title| !title.trim().is_empty()),
                    user_title: entry.title_source.is_some_and(|source| source != "auto"),
                    archived: entry.is_archived == Some(true),
                    last_activity_at: entry.last_activity_at.unwrap_or(0),
                    cwd: entry.cwd,
                })
            });
        }
        self.entry_folders = listed;
        self.desktop = self
            .newest_entries
            .desktop_account(&DesktopPaths::new(&self.home));
    }

    /// Re-reads the live-process files that changed, then which of their
    /// processes still run.
    fn restat_live(&mut self, env: &Environment, paths: &AppPaths) {
        let files = live_files(&env.home, paths);
        refresh_cached(&mut self.live, files, |path, _| {
            serde_json::from_slice::<LiveFile>(&fs::read(path).ok()?).ok()
        });
        self.running = running(
            env,
            self.live.values().filter_map(|(_, live)| live.as_ref()),
        );
    }

    /// The All Sessions window as of `now`, with `profiles` naming the
    /// Sidebars (signed-in Profiles first, in the user's order).
    pub(crate) fn view(&self, now: Timestamp, profiles: &[SidebarProfile]) -> AllSessionsView {
        let sessions = self.sessions();
        let profiles = self.sidebar_profiles(profiles);
        let words: Vec<String> = self
            .search
            .split_whitespace()
            .map(str::to_lowercase)
            .collect();
        let rows: Vec<SessionRow> = sessions
            .iter()
            .filter(|session| self.filter_admits(session))
            .filter(|session| {
                words
                    .iter()
                    .all(|word| session.haystack.contains(word.as_str()))
            })
            .map(|session| session.row(now))
            .collect();
        let detail = self
            .selected
            .as_deref()
            .filter(|id| rows.iter().any(|row| row.id == *id))
            .and_then(|id| sessions.iter().find(|session| session.id == id))
            .map(|session| self.detail(session, now, &profiles));
        AllSessionsView {
            rows,
            search: self.search.clone(),
            loading: self.building,
            sidebar: sidebar(&sessions, &profiles),
            filter: self.filter.clone(),
            detail,
        }
    }

    /// Every Session, newest first: one per Transcript id (the newest file
    /// when an id is in two project folders), plus one per id that only
    /// Sidebar entries still name.
    fn sessions(&self) -> Vec<Session<'_>> {
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
        for entry in self
            .entries
            .values()
            .filter_map(|(_, entry)| entry.as_ref())
        {
            entries
                .entry(entry.cli_session_id.as_str())
                .or_default()
                .push(entry);
        }
        let mut ids: Vec<&str> = by_id.keys().copied().collect();
        ids.extend(entries.keys().filter(|id| !by_id.contains_key(*id)));
        let mut sessions: Vec<Session> = ids
            .into_iter()
            .map(|id| {
                let joined = entries.remove(id).unwrap_or_default();
                self.session(id, by_id.get(id).copied(), joined)
            })
            .collect();
        sessions.sort_by(|a, b| b.last_ms.cmp(&a.last_ms).then_with(|| a.id.cmp(b.id)));
        sessions
    }

    fn session<'a>(
        &self,
        id: &'a str,
        transcript: Option<&'a Transcript>,
        entries: Vec<&'a SidebarEntry>,
    ) -> Session<'a> {
        let title = self.title(transcript, &entries);
        let newest_entry = entries.iter().max_by_key(|entry| entry.last_activity_at);
        let cwd = match transcript {
            Some(transcript) => transcript.cwd.as_deref(),
            None => newest_entry.and_then(|entry| entry.cwd.as_deref()),
        };
        let last_ms = match transcript {
            Some(t) => t
                .last_activity_ms
                .unwrap_or((t.mtime_ns / 1_000_000) as i64),
            None => newest_entry.map_or(0, |entry| entry.last_activity_at),
        };
        let project = project(cwd);
        let haystack = [
            title.as_str(),
            transcript
                .and_then(|t| t.first_prompt.as_deref())
                .unwrap_or(""),
            project,
            transcript.and_then(|t| t.branch.as_deref()).unwrap_or(""),
        ]
        .join("\n")
        .to_lowercase();
        Session {
            id,
            transcript,
            archived: !entries.is_empty() && entries.iter().all(|entry| entry.archived),
            entries,
            title,
            cwd,
            last_ms,
            haystack,
            running: transcript.and_then(|_| self.running.get(id).copied()),
        }
    }

    fn filter_admits(&self, session: &Session) -> bool {
        match &self.filter {
            SessionFilter::All => !session.archived,
            SessionFilter::Running => session.running.is_some(),
            SessionFilter::Project(folder) => {
                session.cwd.map(project_folder) == Some(folder.as_str())
            }
            SessionFilter::Sidebar(pair) => session.entries.iter().any(|e| e.pair == *pair),
            SessionFilter::CliOnly => session.entries.is_empty(),
        }
    }

    /// The Sidebars to list: `known` (each marked when not signed in), then
    /// any other account/Organization folder holding an entry.
    fn sidebar_profiles(&self, known: &[SidebarProfile]) -> Vec<(ProfileKey, String)> {
        let mut listed: Vec<(ProfileKey, String)> = known
            .iter()
            .map(|profile| {
                let name = if profile.signed_in {
                    profile.name.clone()
                } else {
                    format!("{} {NOT_SIGNED_IN}", profile.name)
                };
                (profile.pair.clone(), name)
            })
            .collect();
        for entry in self
            .entries
            .values()
            .filter_map(|(_, entry)| entry.as_ref())
        {
            if !listed.iter().any(|(pair, _)| *pair == entry.pair) {
                // A uuid appears only in hover detail.
                let name = format!("Unknown Login {NOT_SIGNED_IN}");
                listed.push((entry.pair.clone(), name));
            }
        }
        listed
    }

    fn detail(
        &self,
        session: &Session,
        now: Timestamp,
        profiles: &[(ProfileKey, String)],
    ) -> SessionDetail {
        let transcript = session.transcript;
        let client = transcript
            .and_then(|t| t.entrypoint.as_deref())
            .map(|e| match e {
                DESKTOP_ENTRYPOINT => "Claude desktop",
                "cli" => "the CLI",
                other => other,
            });
        let age = age(now, session.last_ms);
        let last_used = match client {
            Some(client) => format!("Last used in {client}, {age}"),
            None => format!("Last active {age}"),
        };
        let sidebars = profiles
            .iter()
            .map(|(pair, name)| {
                let entry = session.entries.iter().find(|entry| entry.pair == *pair);
                SidebarPresence {
                    profile: name.clone(),
                    has_entry: entry.is_some(),
                    archived: entry.is_some_and(|entry| entry.archived),
                    desktop_now: self
                        .desktop
                        .as_ref()
                        .is_some_and(|desktop| desktop.is(pair)),
                }
            })
            .collect();
        SessionDetail {
            id: session.id.to_owned(),
            title: session.title.clone(),
            folder: session.cwd.map(|cwd| self.tilde(cwd)),
            branch: transcript.and_then(|t| t.branch.clone()),
            last_used,
            turns: transcript.map(|t| t.turns),
            first_prompt: transcript.and_then(|t| t.first_prompt.clone()),
            dot: session.dot(),
            sidebars,
            deleted: transcript.is_none().then(|| {
                "The Transcript was deleted (Claude Code removes old ones after \
                 cleanupPeriodDays). Its Sidebar entry is still listed, but there is \
                 nothing to open or resume."
                    .to_owned()
            }),
            open: Vec::new(),
        }
    }

    /// What opening Session `id` depends on, when it is listed.
    pub(crate) fn facts(&self, id: &str) -> Option<SessionFacts> {
        let sessions = self.sessions();
        let session = sessions.iter().find(|session| session.id == id)?;
        Some(SessionFacts {
            id: session.id.to_owned(),
            cwd: session.cwd.map(PathBuf::from),
            entrypoint: session.transcript.and_then(|t| t.entrypoint.clone()),
            deleted: session.transcript.is_none(),
            running: session.running,
            entries: session
                .entries
                .iter()
                .map(|entry| EntryFacts {
                    pair: entry.pair.clone(),
                    entry_id: entry.entry_id.clone(),
                    archived: entry.archived,
                    last_activity_at: entry.last_activity_at,
                })
                .collect(),
        })
    }

    /// The newest `count` Sessions All Sessions lists that can be opened
    /// (Transcript still there), as `(id, "title · project")`.
    pub(crate) fn recent(&self, count: usize) -> Vec<(String, String)> {
        self.sessions()
            .iter()
            .filter(|session| session.transcript.is_some() && !session.archived)
            .take(count)
            .map(|session| {
                let line = format!("{} · {}", session.title, project(session.cwd));
                (session.id.to_owned(), line)
            })
            .collect()
    }

    /// `cwd` with the home folder as `~`.
    fn tilde(&self, cwd: &str) -> String {
        match Path::new(cwd).strip_prefix(&self.home) {
            Ok(rest) if rest.as_os_str().is_empty() => "~".to_owned(),
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => cwd.to_owned(),
        }
    }

    /// A user-set title beats an automatic one: Sidebar entries' user titles
    /// (the desktop's Active Profile's entry first, then the newest), then
    /// the Transcript's `custom-title`, then entries' automatic titles (same
    /// order), the Transcript's `ai-title`, and the first prompt in quotes.
    fn title(&self, transcript: Option<&Transcript>, entries: &[&SidebarEntry]) -> String {
        let best = |user: bool| {
            entries
                .iter()
                .filter(|entry| entry.user_title == user)
                .filter_map(|entry| Some((entry, entry.title.as_deref()?)))
                .max_by_key(|(entry, _)| (self.is_desktop(entry), entry.last_activity_at))
                .map(|(_, title)| title.to_owned())
        };
        best(true)
            .or_else(|| transcript.and_then(|t| t.custom_title.clone()))
            .or_else(|| best(false))
            .or_else(|| transcript.and_then(|t| t.ai_title.clone()))
            .or_else(|| {
                transcript
                    .and_then(|t| t.first_prompt.as_deref())
                    .map(quoted)
            })
            .unwrap_or_else(|| "Untitled Session".into())
    }

    fn is_desktop(&self, entry: &SidebarEntry) -> bool {
        self.desktop
            .as_ref()
            .is_some_and(|desktop| desktop.is(&entry.pair))
    }
}

/// Marks a Sidebar whose account and Organization isn't a signed-in Profile.
const NOT_SIGNED_IN: &str = "(not signed in)";

/// One Session as the window lists it.
struct Session<'a> {
    /// The Transcript id.
    id: &'a str,
    /// `None` once the Transcript is deleted.
    transcript: Option<&'a Transcript>,
    entries: Vec<&'a SidebarEntry>,
    /// Archived in every Profile that has an entry (never without entries).
    archived: bool,
    title: String,
    cwd: Option<&'a str>,
    last_ms: i64,
    /// Lowercased title, first prompt, project and branch, for search.
    haystack: String,
    running: Option<Running>,
}

impl Session<'_> {
    fn dot(&self) -> Option<RunningDot> {
        self.running.map(|running| running.dot)
    }

    fn row(&self, now: Timestamp) -> SessionRow {
        let mut subtitle = format!("{} · {}", project(self.cwd), age(now, self.last_ms));
        if self.transcript.is_none() {
            subtitle.push_str(" · transcript deleted");
        }
        if self.archived {
            subtitle.push_str(" · archived");
        }
        SessionRow {
            id: self.id.to_owned(),
            title: self.title.clone(),
            subtitle,
            dot: self.dot(),
            deleted: self.transcript.is_none(),
        }
    }
}

/// The sidebar: All Sessions and Running; each working folder; each
/// Profile's Sidebar, then CLI only.
fn sidebar(sessions: &[Session], profiles: &[(ProfileKey, String)]) -> Vec<SidebarGroup> {
    let item = |title: &str, filter| SidebarItem {
        title: title.to_owned(),
        filter,
    };
    let mut folders: Vec<&str> = sessions
        .iter()
        .filter_map(|session| session.cwd.map(project_folder))
        .collect();
    folders.sort_by_key(|cwd| (project(Some(cwd)).to_lowercase(), *cwd));
    folders.dedup();
    let mut groups = vec![SidebarGroup {
        title: None,
        items: vec![
            item("All Sessions", SessionFilter::All),
            item("Running", SessionFilter::Running),
        ],
    }];
    if !folders.is_empty() {
        groups.push(SidebarGroup {
            title: Some("Projects".into()),
            items: folders
                .into_iter()
                .map(|cwd| item(project(Some(cwd)), SessionFilter::Project(cwd.to_owned())))
                .collect(),
        });
    }
    let mut sidebars: Vec<SidebarItem> = profiles
        .iter()
        .map(|(pair, name)| item(name, SessionFilter::Sidebar(pair.clone())))
        .collect();
    sidebars.push(item("CLI only (no Sidebar entry)", SessionFilter::CliOnly));
    groups.push(SidebarGroup {
        title: Some("Sidebars".into()),
        items: sidebars,
    });
    groups
}

/// Brings `cache` up to date with `files` (path, stamp, path components
/// below the source's root): drops vanished files and re-reads only new or
/// changed ones with `read`.
fn refresh_cached<T>(
    cache: &mut Cached<T>,
    files: Vec<(PathBuf, Stamp, Vec<String>)>,
    read: impl Fn(&Path, &[String]) -> Option<T>,
) {
    let mut fresh = BTreeMap::new();
    for (path, stamp, inside) in files {
        let value = match cache.remove(&path) {
            Some((known, value)) if known == stamp => value,
            _ => read(&path, &inside),
        };
        fresh.insert(path, (stamp, value));
    }
    *cache = fresh;
}

/// The folders FSEvents watches while the window is open: `projects/`,
/// every `sessions/` and `claude-code-sessions/`, those that exist.
fn watched_folders(home: &Path, paths: &AppPaths) -> Vec<PathBuf> {
    let mut folders = vec![projects_root(home), home.join(".claude").join("sessions")];
    folders.extend(
        real_children(&paths.profiles())
            .into_iter()
            .filter(|(_, kind)| kind.is_dir())
            .map(|(profile, _)| profile.join("sessions")),
    );
    folders.push(sidebar_root(home));
    folders.retain(|folder| folder.is_dir());
    folders
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

/// Folders the Claude desktop app makes for its own Sessions, each listed as
/// one project: "No folder" Sessions' scratch workspaces, and the notes
/// folders of side sessions.
const CLAUDE_FOLDERS: [(&str, &str); 2] = [
    (
        "/Library/Application Support/Claude/scratch-workspaces",
        "No folder",
    ),
    (
        "/Library/Application Support/Claude/side-session-notes",
        "Side-session notes",
    ),
];

/// The project: the name of the Session's project folder.
fn project(cwd: Option<&str>) -> &str {
    let Some(folder) = cwd.map(project_folder) else {
        return "unknown folder";
    };
    CLAUDE_FOLDERS
        .iter()
        .find(|(marker, _)| folder.ends_with(marker))
        .map(|(_, name)| *name)
        .or_else(|| Path::new(folder).file_name()?.to_str())
        .unwrap_or("unknown folder")
}

/// The folder a working directory belongs to: one of [`CLAUDE_FOLDERS`],
/// the repository for a Claude worktree (`<repo>/.claude/worktrees/<name>/…`),
/// else the directory itself.
fn project_folder(cwd: &str) -> &str {
    for (marker, _) in CLAUDE_FOLDERS {
        if let Some(at) = cwd.find(marker) {
            let end = at + marker.len();
            if cwd[end..].starts_with('/') {
                return &cwd[..end];
            }
        }
    }
    cwd.split_once("/.claude/worktrees/")
        .map_or(cwd, |(repository, _)| repository)
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

fn projects_root(home: &Path) -> PathBuf {
    home.join(".claude").join("projects")
}

fn is_jsonl(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
}

/// A subagent's Transcript kept next to its Session's own (`agent-<id>.jsonl`,
/// as SSH Sessions do): part of that Session, not a Session itself.
fn is_subagent(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("agent-"))
}

/// The size and mtime of a regular file that isn't a link.
fn regular_file_stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::symlink_metadata(path).ok()?;
    if !meta.file_type().is_file() {
        return None;
    }
    let mtime = meta
        .modified()
        .ok()
        .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |since| since.as_nanos() as u64);
    Some((meta.len(), mtime))
}

/// Every top-level `~/.claude/projects/<project>/<id>.jsonl`, with its size
/// and mtime. Links (folders or files) aren't followed, and subagent
/// Transcripts, one folder deeper or named `agent-*`, are skipped.
fn transcript_files(home: &Path) -> BTreeMap<PathBuf, Stamp> {
    let mut found = BTreeMap::new();
    for (project, kind) in real_children(&projects_root(home)) {
        if !kind.is_dir() {
            continue;
        }
        for (path, kind) in real_children(&project) {
            if !kind.is_file() || !is_jsonl(&path) || is_subagent(&path) {
                continue;
            }
            if let Some(stamp) = regular_file_stamp(&path) {
                found.insert(path, stamp);
            }
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

/// Every `claude-code-sessions/<account>/<org>/` folder, with its account,
/// Organization and mtime. The mtime is taken before the folder is listed:
/// a write during the listing changes it, so the next rescan lists again.
fn entry_folders(home: &Path) -> Vec<(PathBuf, String, String, u64)> {
    let mut found = Vec::new();
    for (account_dir, kind) in real_children(&sidebar_root(home)) {
        if !kind.is_dir() {
            continue;
        }
        let account = file_name(&account_dir);
        for (org_dir, kind) in real_children(&account_dir) {
            if !kind.is_dir() {
                continue;
            }
            if let Some(modified) = modified_ns(&org_dir) {
                let org = file_name(&org_dir);
                found.push((org_dir, account.clone(), org, modified));
            }
        }
    }
    found
}

/// Every `local_*.json` in one Sidebar folder, with its stamp and `[account,
/// org, file name]`.
fn entry_files(dir: &Path, account: &str, org: &str) -> Vec<(PathBuf, Stamp, Vec<String>)> {
    real_children(dir)
        .into_iter()
        .filter_map(|(path, _)| {
            let name = file_name(&path);
            if !name.starts_with("local_") || !name.ends_with(".json") {
                return None;
            }
            let stamp = regular_file_stamp(&path)?;
            Some((path, stamp, vec![account.to_owned(), org.to_owned(), name]))
        })
        .collect()
}

/// [`refresh_cached`] for the files directly in `dir`, leaving the rest of
/// `cache` alone.
fn refresh_folder<T>(
    cache: &mut Cached<T>,
    dir: &Path,
    files: Vec<(PathBuf, Stamp, Vec<String>)>,
    read: impl Fn(&Path, &[String]) -> Option<T>,
) {
    let listed: std::collections::HashSet<&PathBuf> = files.iter().map(|(path, ..)| path).collect();
    let vanished: Vec<PathBuf> = cache
        .range(dir.to_path_buf()..)
        .take_while(|(path, _)| path.starts_with(dir))
        .filter(|(path, _)| !listed.contains(path))
        .map(|(path, _)| path.clone())
        .collect();
    for path in vanished {
        cache.remove(&path);
    }
    for (path, stamp, inside) in files {
        if cache.get(&path).is_none_or(|(known, _)| *known != stamp) {
            let value = read(&path, &inside);
            cache.insert(path, (stamp, value));
        }
    }
}

/// A file's or folder's mtime in nanoseconds, without following a link.
fn modified_ns(path: &Path) -> Option<u64> {
    let modified = fs::symlink_metadata(path).ok()?.modified().ok()?;
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_nanos() as u64)
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

/// Every `<digits>.json` in `~/.claude/sessions/` and every Profile's
/// `sessions/` (never the `.key` files beside them).
fn live_files(home: &Path, paths: &AppPaths) -> Vec<(PathBuf, Stamp, Vec<String>)> {
    let mut folders = vec![home.join(".claude").join("sessions")];
    folders.extend(
        real_children(&paths.profiles())
            .into_iter()
            .filter(|(_, kind)| kind.is_dir())
            .map(|(profile, _)| profile.join("sessions")),
    );
    folders
        .iter()
        .flat_map(|folder| real_children(folder))
        .filter_map(|(path, _)| {
            let name = file_name(&path);
            let is_pid_file = name
                .strip_suffix(".json")
                .is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit()));
            let stamp = regular_file_stamp(&path).filter(|_| is_pid_file)?;
            Some((path, stamp, Vec::new()))
        })
        .collect()
}

/// A running Session: its dot and the process that runs it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Running {
    pub dot: RunningDot,
    pub pid: u32,
}

/// What opening one Session depends on.
#[derive(Debug, Clone)]
pub(crate) struct SessionFacts {
    pub id: String,
    pub cwd: Option<PathBuf>,
    /// The client the Transcript was last used in.
    pub entrypoint: Option<String>,
    /// The Transcript is gone; only Sidebar entries remain.
    pub deleted: bool,
    pub running: Option<Running>,
    pub entries: Vec<EntryFacts>,
}

/// One Profile's Sidebar entry for a Session.
#[derive(Debug, Clone)]
pub(crate) struct EntryFacts {
    pub pair: ProfileKey,
    /// `local_…`.
    pub entry_id: String,
    pub archived: bool,
    pub last_activity_at: i64,
}

/// Running dots by Transcript id, from live-process files. A file whose pid
/// is dead, or whose pid now belongs to a process started at another time,
/// is stale and ignored.
fn running<'a>(
    env: &Environment,
    lives: impl Iterator<Item = &'a LiveFile>,
) -> HashMap<String, Running> {
    let mut dots = HashMap::new();
    for live in lives {
        let Some(session_id) = live.session_id.clone() else {
            continue;
        };
        let alive = env.os.process(live.pid).is_some_and(|facts| {
            live.proc_start
                .as_deref()
                .and_then(desktop::parse_proc_start)
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
        let running = Running { dot, pid: live.pid };
        let slot = dots.entry(session_id).or_insert(running);
        if dot == RunningDot::Terminal {
            *slot = running;
        }
    }
    dots
}
