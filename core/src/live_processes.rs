//! Live-process files: the one reader of `<config folder>/sessions/<pid>.json`
//! (ADR 0006).
//!
//! Each running Claude Code process writes one in its config folder: the
//! desktop app's children in `~/.claude` (the desktop sets no
//! `CLAUDE_CONFIG_DIR`), a Terminal's in whichever folder it runs with. The
//! busy guard and the running dots both read them here, so they agree on
//! which files count, when one is stale and what its `status` means. The
//! `.key` files beside them are never opened.

use std::fs;
use std::path::{Path, PathBuf};

use jiff::Timestamp;
use serde::{Deserialize, Deserializer};
use serde_json::Value;

use crate::claude_files::{self, DESKTOP_ENTRYPOINT};
use crate::env::OsFacts;

/// The fields of a live-process file the app reads. A file whose `pid`,
/// `procStart`, `entrypoint` or `status` has the wrong type is unreadable:
/// what it says about a running process can't be trusted.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LiveProcess {
    pub pid: u32,
    /// As `ps -o lstart` prints it, in UTC ([`parse_proc_start`]).
    proc_start: Option<String>,
    entrypoint: Option<String>,
    status: Option<String>,
    /// The Transcript id of the Session the process runs.
    #[serde(default, deserialize_with = "text_or_none")]
    pub session_id: Option<String>,
    /// The Sidebar entry id (`local_…`) of a desktop Session.
    #[serde(default, deserialize_with = "text_or_none")]
    pub host_session_id: Option<String>,
    /// The name Claude Code recorded for the Session.
    #[serde(default, deserialize_with = "text_or_none")]
    pub name: Option<String>,
}

/// The fields that only name or title a Session never make a file
/// unreadable: of the wrong type, they are absent.
fn text_or_none<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<String>, D::Error> {
    Ok(match Value::deserialize(deserializer)? {
        Value::String(text) => Some(text),
        _ => None,
    })
}

impl LiveProcess {
    /// The file at `path`, or `None` when it is unreadable.
    pub(crate) fn read(path: &Path) -> Option<LiveProcess> {
        serde_json::from_slice(&fs::read(path).ok()?).ok()
    }

    /// Whether the process that wrote the file still runs. A file whose pid
    /// is dead, or now belongs to a process started at another time, is
    /// stale.
    pub(crate) fn is_running(&self, os: &dyn OsFacts) -> bool {
        let Some(facts) = os.process(self.pid) else {
            return false;
        };
        self.proc_start
            .as_deref()
            .and_then(parse_proc_start)
            .is_some_and(|started| started.as_second() == facts.started_at.as_second())
    }

    /// Whether the desktop app runs this process (one of its Sessions).
    pub(crate) fn is_desktop(&self) -> bool {
        self.entrypoint.as_deref() == Some(DESKTOP_ENTRYPOINT)
    }

    /// Whether the Session is idle. Anything else (`busy`, `waiting`, a
    /// status the app doesn't know, none) counts as busy: fail closed.
    pub(crate) fn is_idle(&self) -> bool {
        self.status.as_deref() == Some("idle")
    }
}

/// The live-process files of the Claude Code processes running in
/// `config_dir`: the regular files named `<pid>.json` (links not followed).
pub(crate) fn files(config_dir: &Path) -> Vec<PathBuf> {
    fs::read_dir(claude_files::live_process_folder(config_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(claude_files::is_live_process_file)
        })
        .map(|entry| entry.path())
        .collect()
}

/// The fields the app relies on that `body` (a live-process file) lacks or
/// holds with the wrong type, for the self-check (ADR 0004).
pub(crate) fn bad_fields(body: &Value) -> Vec<&'static str> {
    let mut bad = Vec::new();
    if !body.get("pid").is_some_and(Value::is_u64) {
        bad.push("pid");
    }
    for key in ["sessionId", "procStart", "entrypoint", "status"] {
        if !body.get(key).is_some_and(Value::is_string) {
            bad.push(key);
        }
    }
    bad
}

/// `procStart` as `ps -o lstart` prints it (`Sun Oct 25 14:02:11 2026`, the
/// day space-padded). Claude Code runs it as `LC_ALL=C TZ=UTC ps …`, so the
/// time is UTC whatever the Mac's time zone.
fn parse_proc_start(text: &str) -> Option<Timestamp> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(body: Value) -> Option<LiveProcess> {
        serde_json::from_value(body).ok()
    }

    #[test]
    fn a_title_field_of_the_wrong_type_is_absent_and_the_status_still_reads() {
        let process = parse(serde_json::json!({
            "pid": 4242,
            "status": "idle",
            "entrypoint": "claude-desktop",
            "name": {"text": "a new shape"},
            "hostSessionId": 7,
            "sessionId": ["x"],
        }))
        .expect("readable");
        assert!(process.is_idle() && process.is_desktop());
        assert_eq!(
            (process.name, process.host_session_id, process.session_id),
            (None, None, None)
        );
    }

    #[test]
    fn a_status_of_the_wrong_type_makes_the_file_unreadable() {
        assert!(parse(serde_json::json!({"pid": 4242, "status": 3})).is_none());
        assert!(parse(serde_json::json!({"pid": "4242", "status": "idle"})).is_none());
    }

    #[test]
    fn only_idle_is_idle() {
        let status = |status: Value| parse(serde_json::json!({"pid": 1, "status": status}));
        assert!(status("idle".into()).is_some_and(|p| p.is_idle()));
        for other in ["busy", "waiting", "something-new"] {
            assert!(status(other.into()).is_some_and(|p| !p.is_idle()));
        }
        assert!(parse(serde_json::json!({"pid": 1})).is_some_and(|p| !p.is_idle()));
    }
}
