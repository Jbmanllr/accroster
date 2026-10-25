//! The CLI switcher's Profile-folder half (ADR 0002): the links from each
//! Profile folder back to `~/.claude`, and the one-time `.claude.json` seed.
//!
//! Links give every Profile the same Transcripts, `--resume` data and user
//! configuration. Everything else (`.claude.json`, `history.jsonl`, caches,
//! logs, `sessions/`, …) stays per Profile.

use std::fs;
use std::io;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::storage;

/// Folders linked to `~/.claude/<name>`; a missing target is created.
const LINKED_DIRS: [&str; 11] = [
    "projects",
    "file-history",
    "tasks",
    "plans",
    "session-env",
    "skills",
    "agents",
    "commands",
    "hooks",
    "output-styles",
    "plugins",
];

/// Files linked to `~/.claude/<name>`, only when the target exists.
const LINKED_FILES: [&str; 3] = ["settings.json", "CLAUDE.md", "keybindings.json"];

/// Per-project keys of `~/.claude.json` that carry trust and MCP approvals.
const SEEDED_PROJECT_KEYS: [&str; 9] = [
    "hasTrustDialogAccepted",
    "allowedTools",
    "enabledMcpjsonServers",
    "disabledMcpjsonServers",
    "disabledMcpServers",
    "mcpServers",
    "hasClaudeMdExternalIncludesApproved",
    "hasClaudeMdExternalIncludesWarningShown",
    "remoteTrustGrant",
];

/// A link that couldn't be made because something else sits in its place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BlockedLink {
    pub path: PathBuf,
}

/// Makes every missing link from `profile_dir` to `claude_home`. Never
/// replaces what is already there: a regular file or folder in a link's place
/// is reported, not overwritten. A link that exists is left as it is.
pub(crate) fn ensure_links(profile_dir: &Path, claude_home: &Path) -> io::Result<Vec<BlockedLink>> {
    let mut blocked = Vec::new();
    let mut link = |name: &str| -> io::Result<()> {
        let path = profile_dir.join(name);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => Ok(()),
            Ok(_) => {
                blocked.push(BlockedLink { path });
                Ok(())
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                symlink(claude_home.join(name), &path)
            }
            Err(error) => Err(error),
        }
    };
    for name in LINKED_DIRS {
        fs::create_dir_all(claude_home.join(name))?;
        link(name)?;
    }
    for name in LINKED_FILES {
        if claude_home.join(name).exists() {
            link(name)?;
        }
    }
    Ok(blocked)
}

/// Writes `profile_dir/.claude.json` from `~/.claude.json`, once: user-scope
/// `mcpServers`, per-project trust and MCP approvals, `theme` and
/// `hasCompletedOnboarding`. Never `oauthAccount`, ids or caches. Does
/// nothing if the Profile already has a `.claude.json`. A missing or
/// unreadable `~/.claude.json` seeds an empty object.
pub(crate) fn seed_claude_json(profile_dir: &Path, home: &Path) -> io::Result<()> {
    let target = profile_dir.join(".claude.json");
    if fs::symlink_metadata(&target).is_ok() {
        return Ok(());
    }
    let source: Value = fs::read(home.join(".claude.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or(Value::Null);
    let mut seed = Map::new();
    for key in ["mcpServers", "theme", "hasCompletedOnboarding"] {
        if let Some(value) = source.get(key) {
            seed.insert(key.into(), value.clone());
        }
    }
    if let Some(projects) = source.get("projects").and_then(Value::as_object) {
        let seeded: Map<String, Value> = projects
            .iter()
            .filter_map(|(path, project)| {
                let kept: Map<String, Value> = project
                    .as_object()?
                    .iter()
                    .filter(|(key, _)| SEEDED_PROJECT_KEYS.contains(&key.as_str()))
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect();
                (!kept.is_empty()).then(|| (path.clone(), Value::Object(kept)))
            })
            .collect();
        if !seeded.is_empty() {
            seed.insert("projects".into(), Value::Object(seeded));
        }
    }
    let mut bytes = serde_json::to_vec_pretty(&Value::Object(seed)).map_err(io::Error::other)?;
    bytes.push(b'\n');
    storage::write_atomically(&target, &bytes)
}

/// The CLI's Active Profile folder: the path the pointer
/// `bin/cli-active-profile` holds, verbatim. `None` without a pointer.
pub(crate) fn cli_active_config_dir(paths: &storage::AppPaths) -> Option<PathBuf> {
    let bytes = fs::read(paths.bin().join("cli-active-profile")).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let path = text.strip_suffix('\n').unwrap_or(&text);
    (!path.is_empty()).then(|| PathBuf::from(path))
}
