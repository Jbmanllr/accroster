//! What kind of Session a Sidebar entry is: the one rule, read from the
//! entry's own fields (ADR 0006).
//!
//! The Session index leaves these entries out and Sidebar sync counts them,
//! so both ask [`kind`] and agree on every entry.

use serde_json::{Map, Value};

use crate::claude_files;

/// Entries with any of these are SSH Sessions.
const SSH_MARKERS: [&str; 3] = ["sshConfig", "sshRemoteTranscriptPath", "sshRemoteProcessId"];
/// … WSL Sessions.
const WSL_MARKERS: [&str; 1] = ["wslConfig"];
/// … scheduled-task Sessions.
const SCHEDULED_MARKERS: [&str; 2] = ["scheduledTaskId", "notifySessionId"];

/// A Session that isn't an ordinary one of this Mac's Transcripts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Kind {
    Ssh,
    /// A "No folder" Session, working in Claude's scratch workspace.
    NoFolder,
    Wsl,
    ScheduledTask,
}

/// The entry's kind, or `None` for an ordinary Session. A marker that is
/// JSON `null` counts as absent. The first that applies wins: WSL, then
/// scheduled-task, then SSH, then No-folder (its `cwd` or its `originCwd` in
/// a scratch workspace).
pub(crate) fn kind(entry: &Map<String, Value>) -> Option<Kind> {
    let has = |markers: &[&str]| {
        markers
            .iter()
            .any(|marker| entry.get(*marker).is_some_and(|value| !value.is_null()))
    };
    let scratch = |field: &str| {
        entry
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(claude_files::is_scratch_workspace)
    };
    if has(&WSL_MARKERS) {
        Some(Kind::Wsl)
    } else if has(&SCHEDULED_MARKERS) {
        Some(Kind::ScheduledTask)
    } else if has(&SSH_MARKERS) {
        Some(Kind::Ssh)
    } else if scratch("cwd") || scratch("originCwd") {
        Some(Kind::NoFolder)
    } else {
        None
    }
}
