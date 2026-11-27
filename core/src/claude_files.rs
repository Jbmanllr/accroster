//! Claude's files: where Claude Code and the Claude desktop app keep what the
//! app reads, and how they name it (ADR 0006).
//!
//! This is the one module that knows that layout. Every other module asks it
//! for a path or a naming rule instead of joining a literal, so when Claude
//! moves or renames something, one place changes. It lists folders, but
//! reads no file's contents: each format has its own reader ([`crate::claude_json`],
//! [`crate::live_processes`], [`crate::usage`], …).
//!
//! - **Claude Code** keeps its config in a config folder: `~/.claude` by
//!   default, or a Profile folder (`CLAUDE_CONFIG_DIR`). The default folder's
//!   config file is `~/.claude.json`, beside it; a Profile folder's is
//!   `<folder>/.claude.json`, inside it. Each config folder has its own
//!   `sessions/<pid>.json` live-process files. Transcripts are
//!   `~/.claude/projects/<project>/<id>.jsonl`, shared by every Profile.
//! - **The desktop app** keeps `config.json` and its Sidebar folders
//!   `claude-code-sessions/<account>/<org>/` under
//!   `~/Library/Application Support/Claude/`, and logs to
//!   `~/Library/Logs/Claude/main.log`. A Sidebar folder holds one
//!   `local_<id>.json` per Sidebar entry and a `deleted_<id>` tombstone per
//!   deleted Session.

use std::fs;
use std::path::{Path, PathBuf};

/// The default config folder's name under the home folder.
pub(crate) const CLAUDE_HOME: &str = ".claude";

/// The folder of a config folder that holds Transcripts, by project.
pub(crate) const PROJECTS: &str = "projects";

/// The Claude desktop app's bundle.
pub(crate) const DESKTOP_APP: &str = "/Applications/Claude.app";

/// The desktop app's executable, as a running process names it.
pub(crate) const DESKTOP_APP_EXECUTABLE: &str = "/Applications/Claude.app/Contents/MacOS/Claude";

/// What the path of a Claude Code bundled with the desktop app contains
/// (`…/Application Support/Claude/claude-code/<version>/…/claude`).
pub(crate) const BUNDLED_CLAUDE_CODE: &str = "/Library/Application Support/Claude/claude-code/";

/// Where Claude Code reads managed (administrator) settings.
pub(crate) const MANAGED_SETTINGS: &str =
    "/Library/Application Support/ClaudeCode/managed-settings.json";

/// The `entrypoint` of a Session the desktop app runs, in live-process files
/// and Transcript records.
pub(crate) const DESKTOP_ENTRYPOINT: &str = "claude-desktop";

/// The desktop app's loader skips larger Sidebar entries; so does every
/// reader here.
pub(crate) const MAX_ENTRY_BYTES: u64 = 10 * 1024 * 1024;

/// What a working folder inside one of the desktop app's scratch workspaces
/// (a "No folder" Session) contains.
const SCRATCH_WORKSPACE: &str = "/scratch-workspaces/";

/// The folder the desktop app makes scratch workspaces in, as a working
/// folder ends (`…/scratch-workspaces/<account>/<org>/<id>` is inside it).
pub(crate) const SCRATCH_WORKSPACES_FOLDER: &str =
    "/Library/Application Support/Claude/scratch-workspaces";

/// The folder the desktop app keeps side sessions' notes in.
pub(crate) const SIDE_SESSION_NOTES_FOLDER: &str =
    "/Library/Application Support/Claude/side-session-notes";

/// What starts the name of a Sidebar folder's tombstone (`deleted_<id>`).
pub(crate) const TOMBSTONE_PREFIX: &str = "deleted_";

const ENTRY_PREFIX: &str = "local_";
const JSON: &str = ".json";

/// Where Claude's files are, derived from the home-folder root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaudePaths {
    home: PathBuf,
}

impl ClaudePaths {
    pub(crate) fn new(home: &Path) -> ClaudePaths {
        ClaudePaths {
            home: home.to_path_buf(),
        }
    }

    /// `~/.claude`: Claude Code's default config folder, which every Profile
    /// folder links back to.
    pub(crate) fn claude_home(&self) -> PathBuf {
        self.home.join(CLAUDE_HOME)
    }

    /// `~/.claude.json`: the default config folder's config file (the
    /// Default login's).
    pub(crate) fn default_config_file(&self) -> PathBuf {
        self.home.join(".claude.json")
    }

    /// `~/.claude/projects`: every Transcript, by project folder.
    pub(crate) fn projects(&self) -> PathBuf {
        self.claude_home().join(PROJECTS)
    }

    /// `~/.claude/settings.json`: the user's settings, which every Profile
    /// folder links to.
    pub(crate) fn shared_settings(&self) -> PathBuf {
        self.claude_home().join("settings.json")
    }

    /// `~/.local/bin/claude`: the link Claude Code's installer manages; it
    /// points at the installed version.
    pub(crate) fn installed_claude(&self) -> PathBuf {
        self.home.join(".local/bin/claude")
    }

    /// The folders under home `claude` is usually installed in, in the
    /// order they are searched.
    pub(crate) fn usual_program_folders(&self) -> [PathBuf; 2] {
        [
            self.home.join(".local/bin"),
            self.claude_home().join("local"),
        ]
    }

    /// `~/Library/Application Support/Claude`.
    fn desktop_data(&self) -> PathBuf {
        self.home.join("Library/Application Support/Claude")
    }

    /// The desktop app's `config.json`. It also holds token caches: read
    /// one key of it, never log it.
    pub(crate) fn desktop_config(&self) -> PathBuf {
        self.desktop_data().join("config.json")
    }

    /// `claude-code-sessions/`: the Sidebar folders, by account and
    /// Organization.
    pub(crate) fn sidebar_root(&self) -> PathBuf {
        self.desktop_data().join("claude-code-sessions")
    }

    /// `claude-code/`: the Claude Code versions bundled with the desktop
    /// app, one folder each.
    pub(crate) fn bundled_claude_code(&self) -> PathBuf {
        self.desktop_data().join("claude-code")
    }

    /// `~/Library/Logs/Claude/main.log`.
    pub(crate) fn desktop_main_log(&self) -> PathBuf {
        self.home.join("Library/Logs/Claude/main.log")
    }
}

/// `<config_dir>/.claude.json`: a Profile folder's config file.
pub(crate) fn config_file(config_dir: &Path) -> PathBuf {
    config_dir.join(".claude.json")
}

/// `<config_dir>/sessions`: where the Claude Code processes running in a
/// config folder write their live-process files.
pub(crate) fn live_process_folder(config_dir: &Path) -> PathBuf {
    config_dir.join("sessions")
}

/// Whether `name` is a live-process file's: `<pid>.json` (never the `.key`
/// files beside them).
pub(crate) fn is_live_process_file(name: &str) -> bool {
    name.strip_suffix(JSON)
        .is_some_and(|stem| !stem.is_empty() && stem.bytes().all(|b| b.is_ascii_digit()))
}

/// A folder name the desktop app could have made from a uuid (also keeps
/// path separators and `..` out of joined paths).
pub(crate) fn is_uuid(text: &str) -> bool {
    text.len() == 36 && text.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

/// The Sidebar entry id (`local_<id>`) a file name (`local_<id>.json`)
/// stands for; `None` for any other file of a Sidebar folder.
pub(crate) fn sidebar_entry_id(file_name: &str) -> Option<&str> {
    file_name
        .strip_suffix(JSON)
        .filter(|stem| stem.starts_with(ENTRY_PREFIX))
}

/// The name of the file that holds the Sidebar entry `entry_id`
/// (`local_<id>.json`).
pub(crate) fn sidebar_entry_file_name(entry_id: &str) -> String {
    format!("{entry_id}{JSON}")
}

/// The Sidebar entry id of the Session with this Transcript id, when the
/// desktop app created the entry for it.
pub(crate) fn sidebar_entry_id_of(transcript_id: &str) -> String {
    format!("{ENTRY_PREFIX}{transcript_id}")
}

/// What follows `local_` in a Sidebar entry id; `None` when `id` isn't one.
pub(crate) fn sidebar_entry_suffix(id: &str) -> Option<&str> {
    id.strip_prefix(ENTRY_PREFIX)
}

/// Whether a working folder is inside one of the desktop app's scratch
/// workspaces: a "No folder" Session's.
pub(crate) fn is_scratch_workspace(cwd: &str) -> bool {
    cwd.contains(SCRATCH_WORKSPACE)
}

/// Whether `path` is named like a Transcript: `<id>.jsonl`.
pub(crate) fn is_transcript_name(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
}

/// A subagent's Transcript kept next to its Session's own (`agent-<id>.jsonl`,
/// as SSH Sessions do): part of that Session, not a Session itself.
pub(crate) fn is_subagent_transcript(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("agent-"))
}

/// Every Session's Transcript under `projects` ([`ClaudePaths::projects`]),
/// with its metadata: the regular files `<project>/<id>.jsonl`. Links
/// (folders or files) aren't followed, and subagent Transcripts, one folder
/// deeper or named `agent-*`, are skipped.
pub(crate) fn transcript_files(projects: &Path) -> Vec<(PathBuf, fs::Metadata)> {
    let real_entries = |dir: &Path| fs::read_dir(dir).into_iter().flatten().flatten();
    real_entries(projects)
        .filter(|project| project.file_type().is_ok_and(|kind| kind.is_dir()))
        .flat_map(|project| real_entries(&project.path()).collect::<Vec<_>>())
        .filter_map(|entry| {
            let path = entry.path();
            if !is_transcript_name(&path) || is_subagent_transcript(&path) {
                return None;
            }
            let meta = entry.metadata().ok().filter(|meta| meta.is_file())?;
            Some((path, meta))
        })
        .collect()
}

/// One Profile's Sidebar folder: `claude-code-sessions/<account>/<org>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SidebarFolder {
    pub account: String,
    pub organization: String,
    pub path: PathBuf,
}

/// Every Sidebar folder under `root` ([`ClaudePaths::sidebar_root`]), sorted
/// by account then Organization. A Sidebar folder is a real folder (links
/// aren't followed) named by a uuid, inside one named by a uuid.
pub(crate) fn sidebar_folders(root: &Path) -> Vec<SidebarFolder> {
    uuid_folders(root)
        .into_iter()
        .flat_map(|(account, account_dir)| {
            uuid_folders(&account_dir)
                .into_iter()
                .map(move |(organization, path)| SidebarFolder {
                    account: account.clone(),
                    organization,
                    path,
                })
        })
        .collect()
}

/// The real subfolders of `dir` named by a uuid, sorted: the account folders
/// of the Sidebar root, or the Organization folders of an account folder.
pub(crate) fn uuid_folders(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<(String, PathBuf)> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            is_uuid(&name).then(|| (name, entry.path()))
        })
        .collect();
    found.sort();
    found
}

/// Every Sidebar entry file (`local_*.json`) directly in a Sidebar folder,
/// with its metadata: regular files only, links not followed.
pub(crate) fn sidebar_entry_files(folder: &Path) -> Vec<(String, fs::Metadata)> {
    fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            sidebar_entry_id(&name)?;
            let meta = entry.metadata().ok().filter(|meta| meta.is_file())?;
            Some((name, meta))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_are_told_apart_by_claudes_own_rules() {
        assert_eq!(sidebar_entry_id("local_abc.json"), Some("local_abc"));
        assert_eq!(sidebar_entry_id("deleted_abc"), None);
        assert_eq!(sidebar_entry_id("archived-sessions.idx"), None);
        assert_eq!(sidebar_entry_id("local_abc.json.tmp"), None);
        assert!(is_live_process_file("4242.json"));
        assert!(!is_live_process_file("4242.key"));
        assert!(!is_live_process_file(".json"));
        assert!(!is_live_process_file("12a.json"));
        assert!(is_uuid("11111111-1111-4111-8111-111111111111"));
        assert!(!is_uuid("../../../../../../../../../../../etc"));
        assert!(!is_uuid("not-a-uuid"));
    }
}
